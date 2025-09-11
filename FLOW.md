# FLOW — Convenant end-to-end wiring

What touches what: every screen, the client function it calls, the generated
instruction it builds, and the on-chain behavior it triggers. Written from the
current state of the codebase (`convenant/`), MagicBlock devnet.

---

## 1. System topology

```
   Browser (two wallets: creator + counterparty)
        │
        ├─ base layer (devnet over MagicBlock RPC)
        │     https://rpc.magicblock.app/devnet
        │     └─ create, join, delegate*, init_permission (ER), cancel, close,
        │        expire_and_refund, swap_on_l1 (magic action), commit result
        │
        ├─ MagicBlock router  https://devnet-router.magicblock.app/
        │     └─ getDelegationStatus(covenant)  →  live ER fqdn
        │
        └─ Ephemeral Rollup (address = covenant, discovered per-delegation)
                 └─ delegate, update_offer, accept, (commit), init_permission
                    — private & fast, enforced on a TEE node
```

The covenant account is `#[ephemeral]`: on base it may show `CreatedOnL1`,
`Delegating`, or `SettledOnL1`; the negotiation state lands on the ER
(`ActiveOnEr`) and writes back through Magic Actions.

Delegation is **DLP-only** (no bundled ACL CPI). Terms live only as a
`CreateEphemeralPermissionCpi` on the ER's Permission program, scoped to the
delegated covenant PDA, and private reads are served only to parties holding a
token signed by the TEE node's challenge.

## 2. On-chain programs

| Program | Address | Role |
| --- | --- | --- |
| Convenant | `3PdS2YjHFktSmyLoFRNr5RDkdPL6cXKmZtaTAYtqQian` | our contract (to be redeployed under a **fresh keypair** later) |
| Permission (ACL / PER) | `ACLseoPoyC3cBqoUtkbjZ4aDrkurZW86v19pXz2XQnp1` | PER access-control; permission PDA owner |
| Delegation | `DELeGGvXpWV2fqJUhqcF5ZSYMS4JTLjteaAMARRSaeSh` | runs `[delegate]` on the ER (DLP only) |
| Magic | `Magic11111111111111111111111111111111111111` | commit / action scheduling |
| Magic Context | `MagicContext1111111111111111111111111111111` | magic action context |
| SPL Token | `TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA` | SPL asset transfers (Token/NFT) |
| Tokenkeg ATA | `ATokenGPvrbdgjgcEgYbswrHBDyGmVJvXHhB9zJxJsir` | ATA create CPI (client-side) |
| System | `11111111111111111111111111111111` | SOL transfers / dummy mint |

### PDAs
- Covenant: `PDA(["covenant", creator, nonce.to_le_bytes()])` — the nonce lets
  a creator open many covenants. Derived client-side via
  `covenantAddress(creator, nonce)`; on create the page uses
  `nonce = BigInt(Date.now())`.
- Vault:     `PDA(["vault", covenantKey])` — holds both locked assets.
- Permission:`PDA(["permission:", covenantKey])` (owner = Permission program),
  funded on base at init with `EphemeralPermission::size_of(members)` × rent.

### Covenant state (`state.rs`)
`version, creator, counterparty: Option<Pubkey>, counterparty_joined,
creator_amount, creator_asset, creator_mint, counterparty_amount,
counterparty_asset, counterparty_mint, creator_accepted,
counterparty_accepted, terms_hash[32], status, deadline(i64), nonce(u64),
bump, vault_bump, created_at, settled_at`.

`AssetType`: Sol=0, Token=1, Nft=2.
`CovenantStatus`: `CreatedOnL1=0, Delegating=1, ActiveOnEr=2, Settling=3, SettledOnL1=4, Reverted=5`.

### Account layout note
`Option<Pubkey>` counterparty shifts fixed offsets past the 8-byte
discriminator: `creator_accepted` at `8+149`, `counterparty_accepted` at
`8+150`, `status` at `8+183` (LiteSVM tests read these bytes directly and are
kept in sync with the built `.so`).

## 3. Lifecycle (who does what, in order)

```
create ──▶ CreatedOnL1 ──join──▶ CreatedOnL1 (counterparty_joined=true)
              │  │
              │  ├─cancel (creator)──▶ Reverted ──▶ close ──▶ account gone
              │  └─(deadline passed)─▶ expire_and_refund ─▶ SettledOnL1 ─▶ close
              │
              └─delegate──▶ Delegating ──(ER confirm)──▶ ActiveOnEr
                                                             │
              ┌─────────────────── negotiate (private) ─────┤
              │   update_offer · accept  (both must accept)  │
              └──────────────────────────────────────────────┤
                       │
            commit_and_swap ──▶ (Magic Action swap_on_l1) ──▶ SettledOnL1
                       │
         (deadline passed) ──▶ expire_and_refund ──▶ SettledOnL1
```

Contract gating per instruction:
- `initialize_covenant` — creator signs; `counterparty: Option<Pubkey>` (`None`
  = open covenant, `Some` = invite-only); creates covenant + vault PDAs; locks
  creator asset; prefunds the covenant PDA enough rent for the future
  permission; requires a future deadline.
- `join_covenant` — counterparty/lock side signs; invite-only covenants require
  the exact pinned key; open covenants pin whoever joins **first** (later
  joiners are rejected — creator excluded, deadline must be future, status
  must be `CreatedOnL1`, not already joined).
- `delegate_covenant` (`#[delegate]`, runs on ER) — DLP-only, pinned
  `validator` from the router; requires created + counterparty joined;
  writes `ActiveOnEr` when the ER activates.
- `init_permission` (ER-only) — creates the private PER for the covenant on the
  ER's Permission program: members = creator + counterparty, both `AUTHORITY`;
  payer = the delegated covenant PDA signing with its base seeds; idempotent
  (skips if the permission holds lamports).
- `update_offer` — ActiveOnEr only; a party may only change **their own**
  amount and only before they've accepted (`FrozenAfterAccept`); **resets
  both** acceptance flags + rehashes `terms_hash`.
- `accept` — ActiveOnEr only; sets own `*_accepted=true`.
- `commit_and_swap` (`#[commit]`, runs on ER) — by a party; requires ActiveOnEr
  + both accepted + before deadline; commits status → `Settling` and schedules
  the magic action.
- `swap_on_l1` (`#[action]`, runs on base) — the escrow, executed atomically;
  moves each party's locked asset from the vault to the opposite party (cross
  swap), sets `SettledOnL1`. Only schedulable via the covenant PDA authority.
- `expire_and_refund` — creator or joined counterparty; from CreatedOnL1 /
  Delegating / ActiveOnEr once `deadline` passed; refunds each party their
  **own** asset, sets `SettledOnL1`.
- `cancel_covenant` — creator only; `CreatedOnL1` only; refunds the vault;
  sets `Reverted` (both parties recover their locks even after joining).
- `close_covenant` — creator only; terminal states (`Reverted`/`SettledOnL1`/
  `Settling`) only; `close = signer` reclaims the covenant PDA lamports (rent +
  permission prefund).

## 4. Screen → client function → on-chain instruction

### Landing — `/` (static)
No chain calls. Marketing; links to `/create` and `/dashboard`.

### Create — `/create` (`app/create/page.tsx`)
Wizard (asset SOL/USDC/NFT, amount, counterparty optional, deadline).
- Derived in handler: `nonce = BigInt(Date.now())`,
  `covenantAddress(creator, nonce)`, `vaultAddress(covenantKey)`,
  SPL → `ataAddress(vaultKey, mint)`.
- Builds `getInitializeCovenantInstructionAsync(...)` → sends on **base** via
  `useSendTransaction().send` → `initialize_covenant`.
- SOL passes `SYSTEM_PROGRAM_ID` as the placeholder mint/token-account metas
  and uses the vault for both source/vault ATA slots (contract short-circuits
  SOL — matches the LiteSVM tests).

### Dashboard — `/dashboard` (`app/dashboard/page.tsx`)
Watch-only. For each row: `createReadRpc(getClusterUrl("devnet"))` +
`readCovenant(readRpc, key)` → `getAccountInfo` + decode. Add/refresh/remove
rows (localStorage `convenant:known-vessels`). No writes. Open covenants show
“not joined” until the counterparty is pinned.

### Covenant detail — `/covenant/[id]` (`app/covenant/[id]/page.tsx`)
Single controller. Polls `readCovenant` every 2s; while `ActiveOnEr`, polls
`getDelegationStatus(covenantKey)` every 3s to discover the ER `fqdn` and
switches later sends to that ER RPC. `covenant.counterparty` is
`Option<Address>` here — resolved with `unwrapOption` before any compare /
ellipsify.

| UI action | Client fn / builder | On-chain | Layer |
| --- | --- | --- | --- |
| Join (lock side) | `getJoinCovenantInstructionAsync` (+ `createAtaInstruction` for SPL vault ATA) | `join_covenant` | base |
| Delegate (creator, after join) | `getDelegateCovenantInstructionAsync({ payer, covenant, validator })` (route from `pickTeeValidator`) | `delegate_covenant` | ER |
| Negotiate — change amount | `getUpdateOfferInstruction` | `update_offer` | ER |
| Negotiate — accept | `getAcceptInstruction` | `accept` | ER |
| Settle (both accepted) | `getCommitAndSwapInstructionAsync` + `settlementAccounts(covenant, vault, "swap")` | `commit_and_swap` → magic action `swap_on_l1` | ER → base |
| Expire (deadline passed) | `getExpireAndRefundInstructionAsync` + `settlementAccounts(covenant, vault, "refund")` | `expire_and_refund` | base |

Settlement wiring:
- `swap`: each party receives the **other's** asset (cross).
- `refund`: each party receives their **own** asset back.
- SOL uses the vault for the ATA slots (same as the LiteSVM tests and the
  create page).

### ER transaction path
`use-send-er-transaction.ts` → `createErClient(signer, erEndpoint)` (JSON-RPC
over the discovered fqdn). Base path: `use-send-transaction.ts` → existing kit
client with the MagicBlock base RPC.

### Private reads (TEE)
`getAuthTokenFor(endpoint, pubkey, signMessage)` runs the challenge/login flow
(curl-style: `/auth/challenge?pubkey=…` → sign → `/auth/login` → bearer token,
30-day session) and `fetchPrivateAccount(endpoint, token, account)` reads the
account with `authorization: Bearer`. Unauthenticated `getAccountInfo` against
a private account is refused by the TEE. The UI currently polls the **base**
read for state; tokenized reads are exercised in the live e2e.

## 5. live e2e (`scripts/e2e-live.ts`, runs via `npx tsx`)

Two funded wallets (`/tmp/opencode/e2e-creator.json`,
`/tmp/opencode/e2e-cp.json`):

- **Flow A (SOL↔SOL, invite-only, TEE)**: pinned init (creator 0.5 SOL) →
  join (cp 0.25 SOL) → delegate to the router's TEE node (fqdn must contain
  `tee`) → wait `ActiveOnEr` → `init_permission` on the ER → cp obtains a TEE
  bearer token (tweetnacl `sign.detached(msg, web3Keypair.secretKey)`) →
  tokenized private read decodes the covenant, unauthenticated read is denied
  → cp raises to 0.3 SOL → both accept → `commit_and_swap` (magicProgram +
  magicContext) → wait `SettledOnL1` + vault drained.
- **Flow B (open covenant)**: no pinned counterparty → first-come join pins
  cp → a second joiner is rejected → creator cancels → `Reverted` + vault
  drained + cp recovered → close → account gone.
- **Flow B2 (delegated expiry refund)**: open covenant whose deadline is 60s
  in the future → first-come join → delegate to TEE → wait for the clock to
  pass the deadline → `expire_and_refund` (creator signs, on base) →
  `SettledOnL1` + vault drained + both parties recovered → close.

Signing notes: legacy `@solana/web3.js` 1.98 `Keypair` has no public `sign`;
the TEE challenge is signed with tweetnacl using `Keypair.secretKey`.

## 6. Current state (as of this wiring)

- Contract: 13 convenant + 7 vault LiteSVM tests green; SBF + IDL built;
  codama client regenerated (12 instructions); CI dry-runs anchor build.
- Frontend: `tsc`, `eslint` (0 errors), and `next build` all green; routes
  `/`, `/create`, `/dashboard`, `/covenant/[id]`, `/icon.svg`.
- **The convenant program on devnet is still the OLD binary at the old
  address.** The Phase 2 contract is to be deployed under a **fresh program
  keypair** (deferred); until then the live e2e cannot run.
- Wallet `~/.config/solana/id.json` + `DztJxBybR7fKa9UyR8fBZMohaZYcLydyxrJr5CuJBa34`
  funded for the deploy; `/tmp/opencode/e2e-{creator,cp}.json` funded devnet
  wallets.

## 7. Known gaps + suggestions (prioritized)

1. **Deploy under a fresh program id** (decided, deferred) — new keypair,
   `anchor deploy` to devnet, update `PROGRAM_ID` const + generated program
   defaults, then run the live e2e (Flows A + B + B2) end to end.
2. **Open-join UX** — the detail page's join UX assumes an invited
   counterparty; surface the "open covenant — first to join locks it" affordance.
3. **Private reads in the UI** — wire the TEE tokenized read into the detail
   page (base poll is fine for the demo but the tokenized proof is stronger).
4. **Magic action re-validation** — `swap_on_l1` should re-check deadline +
   both-accepted on base at execution time to close the commit→action race.
5. **Vault ATA cleanup** — after settle/refund, close vault ATAs to reclaim
   rent.
6. **Demo tokens** — provide a devnet mint/airdrop helper for USDC/NFT so the
   two-browser demo doesn't rely on pre-existing balances.
7. **Config** — hardcoded program IDs / USDC mint in the client → move to
   env/constants; commit sponsorship + fee vault for ER sends not wired.

## 8. File map

- `convenant/app/page.tsx` — landing
- `convenant/app/create/page.tsx` — create wizard (initialize, nonce)
- `convenant/app/dashboard/page.tsx` — watch-only list (Option counterparty)
- `convenant/app/covenant/[id]/page.tsx` — lifecycle controller (join / delegate(ttee)/ negotiate / settle / expire / cancel / close)
- `convenant/app/lib/covenant.ts` — PDAs (nonce), ATAs, read, settlement accounts, formatting, localStorage
- `convenant/app/lib/magicblock.ts` — router/ER clients, `getDelegationStatus`, TEE picker + auth token + private reads
- `convenant/app/lib/hooks/use-send-transaction.ts` — base sends
- `convenant/app/lib/hooks/use-send-er-transaction.ts` — ER sends
- `convenant/app/generated/convenant/instructions/*` — codama builders (12 instructions)
- `convenant/anchor/programs/convenant/src/{lib,state,token,errors}.rs` + `tests.rs` — contract + LiteSVM tests
- `convenant/scripts/e2e-live.ts` — two-wallet live e2e (Flow A / B / B2)
- `CONVENANT_MVP_SPECIFICATION.md`, `plan.md` — spec + original design