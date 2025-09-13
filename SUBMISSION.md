# Convenant — Private Two-Party Swaps on MagicBlock Ephemeral Rollups

**Track:** MagicBlock · **Network:** Solana Devnet (MagicBlock) · **Status:** Live, deployed, reproducible

---

## 1. The problem

Two people want to trade. They do not want a middleman, and they do not want
each other to be able to read their offer and walk away with better terms.

Every mainstream escrow solves this by hiding *everything* behind a relayer —
which means the relayer learns your price, and every settlement waits on a
server. Public Solana escrow inverts the problem: terms are on-chain and
immutable, but the moment you publish an offer, your counterparty can see it
and front-run your negotiation.

**Convenant is a private escrow.** Both parties see a live, fast, shared
workspace where only they can read the terms, and settlement lands atomically
on Solana's base layer with no relayer in the middle.

---

## 2. Why MagicBlock, and not "Solana plus a database"

The privacy here is not an application-level trick. It is enforced by the
execution environment.

The covenant account is delegated to a **MagicBlock Ephemeral Rollup** pinned to
a **TEE node** (Intel TDX). Negotiation runs inside that rollup, where a
**Permission account scoped to the covenant PDA** admits exactly two members —
the creator and the counterparty. A third party calling `getAccountInfo` on
that account is refused by the node itself, not by our code.

Then, because the rollup is an SVM and not a database, the negotiated state is
committed back to Solana and a **Magic Action** settles the escrow atomically
with that commit.

MagicBlock is not a performance add-on here. It is the thing that makes a
private escrow possible without inventing a trusted operator.

---

## 3. What is deployed

| | |
|---|---|
| **Contract (Convenant)** | `2uY6gVNHe62XZXByt796pbwye1p7eTu2Nftv7uwC5CtQ` |
| Loader | `BPFLoaderUpgradeab1e11111111111111111111111111` |
| ProgramData | `CktFqFPmTCW6eD8DjAM8StzxthJma7ZQWFug4FMDVvtf` |
| Upgrade authority | `DztJxBybR7fKa9UyR8fBZMohaZYcLydyxrJr5CuJBa34` |
| Last deployed | slot `510351960` |
| Program size | 512,000 bytes |

MagicBlock programs it composes with:

| Program | Address |
|---|---|
| Permission (ACL / PER) | `ACLseoPoyC3cBqoUtkbjZ4aDrkurZW86v19pXz2XQnp1` |
| Delegation (DLP) | `DELeGGvXpWV2fqJUhqcF5ZSYMS4JTLjteaAMARRSaeSh` |
| Magic | `Magic11111111111111111111111111111111111111` |
| Magic Context | `MagicContext1111111111111111111111111111111` |
| Ephemeral Vault | `MagicVau1t999999999999999999999999999999999` |
| SPL Token | `TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA` |
| TEE validator used | `MTEWGuqxUpYZGFJQcp8tLN7x5v9BSeoFHYWQQ3n3xzo` (`devnet-tee.magicblock.app`) |

Base RPC `https://rpc.magicblock.app/devnet` · Router
`https://devnet-router.magicblock.app/` · TEE `https://devnet-tee.magicblock.app/`

Demo wallets: creator `FJTwNFddjUy3Ur9csceLsTzUbC35R389qYkXtXFK5XDU`,
counterparty `J3pXqcm3qexhx5UYVNAgCrw9RGGcnPwgTmW6UzvvwuTe`.

---

## 4. How it works

```
BASE LAYER (Solana devnet)              EPHEMERAL ROLLUP (TEE)
─────────────────────────────           ─────────────────────────
create ────► CreatedOnL1
join   ────► both assets in vault PDA
delegate ───────────────────────────►   ActiveOnEr  (TEE-enforced)
                                      init_permission  ← private gate
                                      update_offer    ← private terms
                                      accept          ← mutual consent
                                      commit_and_swap ──┐
                                                            │ commit
                                                            ▼
BASE: swap_on_l1  ◄── Magic Action executes atomically with the commit
       └─► assets cross to the opposite party, SettledOnL1
```

The vault is a PDA holding **both** locked assets. `swap_on_l1` is the only
code that moves value out of it, and both settlement paths converge there.

Escrow supports **SOL, SPL tokens, and NFTs**. Because MagicBlock does not
deploy the SPL Associated Token Account program, Convenant derives its own token
accounts under seeds `[owner, token_program, mint]` and creates them on-chain via
`create_associated_token_account`.

---

## 5. Live proof

Reproduce with:

```bash
npm run setup                 # build program + generate the TS client
E2E_FUNDER_KEYPAIR=~/.config/solana/id.json npx tsx scripts/e2e-live.ts
```

The live suite drives two funded devnet wallets through the whole lifecycle on
the TEE. From the most recent run:

```
init                create ok, vault = 0.5000 SOL
join                join ok, vault = 0.7500 SOL
delegate            validator MTEWGuqxUpYZGFJQcp8tLN7x5v9BSeoFHYWQQ3n3xzo
                    → https://devnet-tee.magicblock.app
init_permission    4Vpz4wmzU9oPRiDMjpTeGMAkzwAk8fY7RcCurrn2Tcp2qzUpfDNJYuusS8whaTW8Cmrc8sMMtrHq2m81UiXLQtaN
private read       creator=FJTwNF cp=J3pXqc creatorLock=0.5000 SOL status=2
unauthenticated    correctly denied          ← the privacy claim, negatively proven
update_offer       creator 0.40 SOL
update_offer       counterparty 0.30 SOL
over-escrow        correctly rejected (InsufficientFunds)
both accepted
commit_and_swap    executes on the ER
```

The unauthenticated read being **refused by the TEE** is the single most
important line above: it is the difference between this being private escrow and
this being a claim about privacy.

### Settlement is not yet landing on the base layer — stated plainly

`commit_and_swap` executes successfully on the rollup, but the swap itself does
not complete: the vault still holds 0.75 SOL and the covenant never reaches
`SettledOnL1`. This is the one thing in the product that is not proven live.

Two separate bugs were found and one was fixed:

1. **The commit never undelegated** *(fixed and deployed)*. A plain
   `commit` writes the rollup state down but leaves the covenant owned by the
   delegation program, so `swap_on_l1` — which validates the covenant with
   `Account<'info, Covenant>` and therefore requires program ownership — could
   never succeed. Switching to `commit_and_undelegate` returns ownership to the
   program; confirmed on devnet, the account owner is now the program rather
   than `DELeGGvX...`.

2. **The committed bytes are corrupt** *(open)*. After the commit the account
   carries a valid Anchor discriminator and length but fails to decode as a
   `Covenant` ("Enum discriminator out of range"), and the amounts at the
   documented offsets are nonsense. So the second-stage action still has no
   valid covenant to read. This is the remaining work.

Settlement logic is verified independently: **26 LiteSVM tests** drive
`swap_on_l1` end to end across SOL/USDC/NFT combinations and assert exact
balance movements. What is unproven is the ER→base handoff.

We are flagging this rather than papering over it.

## 6. The blocker we solved

`commit_and_swap` failed on the rollup with `MissingRequiredSignature` on every
attempt. Two independent causes, both fixed:

1. **The escrow authority's signature was being dropped.** The covenant PDA is
   the action's `escrow_authority`, but a PDA that is not itself a transaction
   signer arrives with `AccountInfo::is_signer == false`. The MagicBlock SDK
   builds its CPI account metas straight from that flag, so
   `invoke_signed(seeds)` had nothing to sign and the runtime rejected the CPI.
   Fix: promote a copy of the account info with `is_signer = true` for both the
   commit list and the action's escrow authority.

2. **Base-layer accounts were declared writable on the rollup.** The vault and
   the token accounts are *not* delegated to the ER, so passing them as writable
   failed with `InvalidWritableAccount`. `commit_and_swap` only needs their
   addresses to build the action's account list; `swap_on_l1` is what touches
   them, on base. Fix: declare them read-only.

With both applied the commit transaction executes on the ER.

---

## 7. Security

A review found the escrow could be drained and it was exploitable on devnet. All
of it is fixed, deployed, and covered by tests.

**Recipient pinning.** `expire_and_refund`, `cancel_covenant`, `swap_on_l1` and
`commit_and_swap` all declared their payout accounts as unconstrained
`UncheckedAccount`s. Because `release_asset` signs with the vault PDA, any party
could name an attacker-controlled recipient and have the escrow pay it:
a counterparty could claim the creator's lock, a creator could claim the
counterparty's lock, and anyone could drain `swap_on_l1` into two wallets of
their choosing. Every recipient is now constrained with an `address` check
against the parties recorded on the covenant.

Proven against the deployed program, not just in simulation
(`npx tsx scripts/verify-hardening.ts`):

```
vault holds 0.5 SOL
ATTACK — counterparty refunds with a forged `creator` recipient
  rejected : true
  Program ... failed: custom program error: 0x1787     (6023 CreatorMismatch)
attacker gained : 0 SOL
vault lamports  : 500000000 -> 500000000
PASS — deployed program rejected the forged refund and the escrow is intact.
```

**Solvency bound.** `update_offer` accepted any amount and validated *after*
writing. A party could negotiate an offer the vault could not settle. The two
committed offers are now bounded by `escrow_total` — the sum both parties
actually locked, recorded on-chain at join, because the vault balance is not
readable from the rollup.

**Test coverage.** 26 LiteSVM tests, of which **16 are negative** and assert an
exact Anchor error code rather than merely `is_err`: forged refund/cancel/swap
recipients, wrong escrow authority, premature and third-party refunds,
non-creator cancel, join after deadline, insolvent re-pricing, zero and
post-accept re-pricing, non-party negotiation, settlement without mutual
acceptance, and settlement of an unjoined covenant.

---

## 8. Running it

```bash
npm install
npm run setup          # anchor build + codama client generation
npm run dev            # http://localhost:3000

npm run anchor-test    # 26 LiteSVM tests
npm run build          # production build
bash scripts/deploy-devnet.sh   # redeploy (idempotent; clears stale buffers)
```

Two browsers, two devnet wallets, `/create` then `/covenant/<id>`.

---

## 9. Stack

Next.js 16 · React 19 · TypeScript · Tailwind v4 · `@solana/kit` 6 · Codama ·
wallet-standard · `@magicblock-labs/ephemeral-rollups-sdk` 0.17.0 · Anchor 1.1.2
(Rust/SBF) · LiteSVM.

## 10. Honest limitations

- **Settlement does not complete on the base layer.** The commit executes and
  now correctly undelegates, but the committed account data fails to decode, so
  the swap action has nothing valid to read and the vault is untouched. The
  logic is proven by LiteSVM; the ER→base payload is not. This is the main
  thing left to finish.
- **Devnet only.** The client is hard-locked to MagicBlock devnet — there is no
  mainnet option, deliberately.
- **Demo tokens.** `scripts/mint-demo-tokens.ts` mints stand-in USDC and an NFT
  on devnet so the two-browser demo never depends on pre-existing balances.
- **Not audited.** Written for a hackathon.
