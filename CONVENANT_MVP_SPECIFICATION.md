# Convenant: MagicBlock-First Product and Strict MVP Specification

**Status:** Corrected implementation baseline for the MagicBlock hackathon  
**Prepared by:** Manus AI  
**Primary technology:** MagicBlock Ephemeral Rollups on Solana  
**Frontend:** Next.js App Router with TypeScript  
**MVP environment:** Solana Devnet plus MagicBlock development infrastructure

> **Correction note:** The previous version incorrectly framed Convenant as a generic Solana escrow product. That is not sufficient for a MagicBlock hackathon. This version makes **MagicBlock Ephemeral Rollups the primary execution primitive and product differentiator** and makes **Private Ephemeral Rollups the required privacy path whenever covenant terms or execution state must remain confidential**.

> **Product assumption:** Because the original product definition was not provided, this document uses Convenant to mean a real-time, two-party covenant/agreement application in which users create, accept, execute, and settle commitments. The agreement-specific business rule must be confirmed, but the MagicBlock architecture and boundaries below should remain the foundation.

## 1. Product thesis

Convenant is a real-time agreement protocol in which a covenant is not merely created on Solana and later read from a slow or centralized application. Its execution state is delegated to a MagicBlock Ephemeral Rollup so that participants can interact with the covenant quickly and repeatedly while preserving Solana composability and settlement. The base layer provides durable ownership and settlement; the Ephemeral Rollup provides the responsive execution environment; the frontend makes the transition understandable and verifiable.

The product should communicate one clear idea: **agreements can execute with application-like responsiveness while remaining connected to Solana’s verifiable state model**. If the terms or execution state must remain confidential, the same design uses a Private Ephemeral Rollup to keep selected state and logic inside a TEE-backed environment with controlled access. If the terms and execution state are intentionally public, a standard Ephemeral Rollup is sufficient.

MagicBlock documentation describes ERs as specialized SVM runtimes where one or more accounts can be locked and temporarily shifted to an auxiliary execution layer, with state delegation, sequencer execution, and synchronization back to the base layer.[1] MagicBlock describes PERs as ERs combined with Intel TDX-backed trusted execution environments for confidential state and logic.[2]

## 2. What Convenant means: a simple analogy

Imagine a traditional agreement kept in a **smart contract room**. The agreement has a rulebook, two participants, and a record of what has happened. Solana is the permanent courthouse record: it gives the agreement durable identity and preserves the final settlement. A standard MagicBlock Ephemeral Rollup is the fast meeting room attached to that courthouse. Participants can make repeated updates quickly inside the room, while the agreed result can later be committed back to the permanent record.

If the agreement’s terms or live execution state must be confidential, the meeting room becomes a **Private Ephemeral Rollup**. Authorized participants can see the sensitive information while the computation runs inside MagicBlock’s protected environment. The courthouse can receive the permitted final result or commitment without exposing every private detail. If everything is intentionally public, a standard ER is the correct and simpler choice.

In this analogy, Convenant is the rulebook and application coordinating the room, the participants, and the final record. MagicBlock is not an optional performance add-on: it is the room where the active agreement runs. Solana is not replaced; it remains the durable foundation and settlement layer. The frontend is the receptionist and dashboard, while the program is the impartial referee that rejects actions violating the agreement’s rules.

## 3. Architecture decision summary

| Decision | Strict MVP decision | Rationale |
|---|---|---|
| Execution environment | Use a MagicBlock ER for the active covenant state | MagicBlock must be visible in the core user journey, not only in documentation |
| Base layer | Solana stores the canonical program and receives committed/settled state | Preserves composability and durable settlement |
| Privacy | PER is an optional MVP mode only if private terms are central to the product | PER introduces access control, attestation, TEE trust assumptions, and extra demo risk |
| Frontend | Next.js is approved | Wallet and ER interactions belong in client components; public/read-only content can use server components |
| Contract language | Rust + Anchor-compatible Solana program, following MagicBlock ER integration guidance | Reuses Solana tooling while adding delegation/commitment behavior |
| Data model | On-chain covenant account plus explicit delegation/commitment state | Avoids a centralized database as the source of truth |
| Network | Devnet only | Prevents accidental mainnet use during the hackathon |

## 4. Strict MVP definition

### 4.1 The one demonstrable loop

The strict MVP is complete when a user can create a covenant, delegate its active state to a MagicBlock ER, and perform the core covenant actions through that ER with visibly low-latency interaction. A second wallet must be able to participate according to the covenant’s authority rules. The resulting state must be committed or undelegated back to Solana and independently inspectable.

The live demo should use a compact agreement such as a two-party commitment with a deposit, acceptance, progress/checkpoint action, and settlement. The exact domain language may be changed later, but the flow must demonstrate **delegation, ER execution, and commitment/settlement** rather than only ordinary Solana transactions.

### 4.2 MVP state machine

| State | Execution location | Meaning | Valid next states |
|---|---|---|---|
| `CreatedOnL1` | Solana | Covenant account initialized on the base layer | `Delegating`, `Cancelled` |
| `Delegating` | Solana/MagicBlock handoff | Delegation transaction is being processed | `ActiveOnER`, `DelegationFailed` |
| `ActiveOnER` | MagicBlock ER | Covenant is live for fast interaction | `Accepted`, `Funded`, `Checkpointed`, `Settling`, `Reverted` |
| `Settling` | ER then Solana | Final state is being committed or undelegated | `SettledOnL1`, `SettlementFailed` |
| `SettledOnL1` | Solana | Final covenant result is durable on the base layer | Terminal |
| `Reverted` | Solana/ER boundary | Invalid or failed ER execution was rejected and state restored/unlocked according to protocol behavior | `ActiveOnER` only through a documented recovery path |
| `Cancelled` | Solana or ER, according to authority | Covenant ended without settlement | Terminal |

The implementation must not invent a fake “ER mode” that merely sends ordinary Solana RPC calls. The repository must contain the actual MagicBlock delegation/commitment/undelegation integration required by the selected ER path, and the demo must show the active account being processed through that path.

### 4.3 Features included and excluded

| Capability | Strict MVP | Rule |
|---|---:|---|
| Wallet connection | Yes | Two separate Devnet wallets |
| Create covenant on Solana | Yes | Durable initial account |
| Delegate covenant account to MagicBlock ER | Yes | Required differentiator |
| Fast ER interaction | Yes | At least one meaningful state transition while active on ER |
| Commit or undelegate final state to Solana | Yes | Required proof of settlement |
| Agreement detail and state history | Yes | Show L1 and ER-related signatures/identifiers where available |
| SOL escrow | Optional | Include only if it does not obscure ER integration; otherwise use non-financial checkpoint state |
| Private Ephemeral Rollup | Conditional | Required if private obligations/terms are the main product value; otherwise Phase 2 |

| Mainnet | No | Block in code and interface |
| Centralized database source of truth | No | Optional indexer only for discovery and analytics |
| Backend custody signer | No | User wallets sign user-authorized actions |
| Legal enforceability claim | No | Use a clear software/demo disclaimer |

## 5. ER and PER: exact role in Convenant

### 5.1 Ephemeral Rollups are core, not optional

An ER is the primary execution layer for Convenant’s live state. The covenant account or state cluster is delegated from Solana to the MagicBlock ER. While delegated, the program executes in the ER’s specialized SVM runtime and the application routes transactions through the MagicBlock-supported path. The ER is responsible for responsive execution and sequencing during the active session. Solana remains the durable base layer to which the final or committed state returns.

The MVP must identify which account or accounts are delegated, which instructions are permitted while delegated, how the frontend discovers the ER endpoint or router, and how the application knows that commitment or undelegation has completed. These must be tested as protocol behavior, not represented as frontend booleans.

### 5.2 Private Ephemeral Rollups are a product mode, not a synonym for ER

A PER combines ER execution with TEE-backed confidentiality. MagicBlock states that accounts are public by default and that programs can explicitly define access rules; PER is intended for sensitive logic such as confidential transfers, sealed-bid auctions, and secure identity flows.[2] Therefore, Convenant should use PER only if the covenant contains information that must remain hidden from unauthorized observers—for example, a private offer, private performance metric, private condition, or sealed commitment.

If the terms and execution state are intentionally public, the strict MVP may use a standard ER. If either the terms or execution state must remain confidential, PER is mandatory and the strict MVP must include attestation, access tokens/challenges, authorization rules, and a clear explanation of the TEE trust assumption. PER must never be advertised as zero-knowledge privacy or as removing all trust assumptions.







| Primitive | Required when | Not required when |
|---|---|---|
| ER | Always for the MagicBlock MVP | Never omit from the main architecture |
| PER | Terms or execution state must be confidential | All state is intentionally public |

## 6. System architecture

```text
Next.js browser client
  ├── Public server-rendered shell and documentation
  ├── Client wallet provider
  ├── Convenant UI state machine
  ├── MagicBlock router / ER client adapter
  ├── Optional PER attestation and access-control client
  └── Solana/ER transaction confirmation and refresh
          │
          ├── Solana Devnet RPC: initialize, delegation, commitment, settlement
          └── MagicBlock ER RPC/router: active covenant execution
                    │
                    ├── Convenant program: Rust/Anchor-compatible logic
                    ├── Delegation/commitment/undelegation integration
                    ├── Optional escrow accounts
                    ├── Optional PER access-control / TEE boundary
```

### 6.1 Repository structure

```text
convenant/
├── app/
│   ├── page.tsx
│   ├── create/page.tsx
│   ├── covenants/[id]/page.tsx
│   ├── covenants/[id]/active/page.tsx
│   └── demo/page.tsx
├── components/
│   ├── wallet/
│   ├── covenant/
│   ├── magicblock/
│   ├── privacy/
│   └── transaction/
├── lib/
│   ├── solana/
│   ├── magicblock/
│   │   ├── er-client.ts
│   │   ├── delegation.ts
│   │   ├── commitment.ts
│   │   └── undelegation.ts
│   ├── per/
│   └── covenant/
├── programs/convenant/src/lib.rs
├── tests/
│   ├── program/
│   ├── er/
│   ├── per/
├── idl/
├── docs/
│   ├── architecture.md
│   ├── er-lifecycle.md
│   ├── per-threat-model.md
│   └── demo-script.md
├── Anchor.toml
├── package.json
└── README.md
```

The `lib/magicblock/` adapter is mandatory. It prevents MagicBlock-specific routing and lifecycle behavior from leaking into every screen and makes it possible to update SDK details without rewriting the product layer.

## 7. Contract and account design

### 7.1 Base covenant account

The base account is initialized on Solana and contains the durable identity, terms commitment, authority model, and lifecycle status. A recommended PDA is:

```text
covenant_pda = PDA(
  seeds = [b"covenant", creator_pubkey, covenant_nonce_u64_le],
  program_id = CONVENANT_PROGRAM_ID
)
```

The exact seeds must be centralized in one shared helper and covered by client and program tests.

| Field | Purpose |
|---|---|
| `version` | Account layout version |
| `creator` | Creator authority |
| `counterparty` | Counterparty authority |
| `nonce` | Unique creator-scoped identifier |
| `status` | L1/ER lifecycle state |
| `terms_hash` | Canonical terms commitment |
| `deadline` | Optional time boundary |
| `er_cluster_or_id` | Identifies the selected ER context if required by the integration |
| `delegation_status` | Not delegated, pending, delegated, committing, undelegated |
| `checkpoint` | Latest agreed progress or execution checkpoint |
| `created_at` | Creation timestamp |
| `settled_at` | Finalization timestamp |
| `bump` | PDA bump |

Do not store arbitrary sensitive text in the public base account. For a public MVP, bounded display text may be stored on-chain. For a private mode, store only commitments or public metadata on Solana and keep private execution state inside the PER according to MagicBlock’s privacy and access-control model.

### 7.2 Required instruction groups

| Group | Instructions | Responsibility |
|---|---|---|
| Initialization | `initialize_covenant` | Create durable L1 account and immutable terms commitment |
| Delegation | MagicBlock-compatible delegation instruction(s) | Lock/route the account into the ER and record lifecycle state |
| Active execution | `accept`, `checkpoint`, `approve`, or domain-specific action | Execute covenant logic while the account is active in the ER |
| Commitment | MagicBlock-compatible commit instruction(s) | Commit valid ER state toward Solana |
| Undelegation | MagicBlock-compatible undelegation instruction(s) | Return the account to normal L1 control after finalization |
| Settlement | `settle` or domain-specific final action | Apply the final deterministic result and close/mark terminal state |
| Optional privacy | PER authorization/access instructions | Restrict state access and prove client authorization |

The actual instruction names and accounts must follow the MagicBlock SDK and current integration guide used by the implementation. The agent must not invent wrapper instructions that bypass the official delegation or commitment flow.

### 7.3 Contract invariants

The program must enforce signer roles, PDA derivation, account ownership, valid delegation state, valid ER/L1 lifecycle transitions, immutable terms commitments, deadline rules, terminal-state protection, and one-time settlement. If funds are included, the amount and recipient must be stored and checked on-chain, and no arbitrary frontend-supplied payout address may substitute for the covenant recipient.

For PER, access rules must be explicit and default-deny for sensitive accounts. The program must never expose sensitive state through a public account, unprotected event, or client-side fallback.

## 8. Frontend versus ER/program responsibility

### 8.1 Frontend responsibility

The Next.js client handles presentation, wallet connection, form validation for usability, terms preview, PDA derivation, MagicBlock endpoint selection from configuration, transaction construction, wallet signing, pending/confirmed/error states, account refresh, and Explorer links. It also explains whether the covenant is on L1, delegating, active on ER, committing, or settled on L1.

For PER, the client handles the attestation/challenge/access-token interaction required by the selected MagicBlock integration but must not claim access merely because a user clicked a button.

### 8.2 Program and MagicBlock responsibility

The Solana program and MagicBlock lifecycle enforce signer authority, state transitions, delegation eligibility, commitment and undelegation behavior, private-account access rules where applicable, and escrow movement if applicable. MagicBlock provides the specialized ER runtime and routing/settlement mechanisms; the Convenant program defines the application state and business rules.

### 8.3 Absolute separation rules

| Must never be decided only by the frontend | Must be enforced by program/MagicBlock path |
|---|---|
| Who may accept or settle | Signer and stored authority |
| Whether a covenant is active | Delegation and account state |
| Whether an ER update is valid | Program constraints and lifecycle rules |
| Whether a private user may access data | PER authorization/access layer |
| Whether a payout is allowed | Program state, amount, and recipient checks |

## 9. Next.js frontend decision

Next.js is approved as the frontend because its App Router supports server-rendered pages and client components.[4] Use server components for marketing, public documentation, and non-wallet shell content. Use client components for wallet providers, MagicBlock ER interaction, PER attestation, transaction signing, and live state refresh.

The MVP must pin versions. If the selected MagicBlock template requires Anchor’s TypeScript client and legacy `@solana/web3.js` v1, use that compatible stack consistently; Anchor’s documentation states that its current TypeScript client is not compatible with web3.js v2.[5] Solana’s current documentation recommends `@solana/kit` for new TypeScript applications and identifies web3.js as legacy.[6] This creates a deliberate implementation decision: follow the MagicBlock template’s verified compatibility for the hackathon, isolate it behind adapters, and plan a post-MVP migration rather than mixing incompatible SDK generations during the build.

## 10. Screens and user flows

### 10.1 Required screens

| Screen | Required behavior |
|---|---|
| Landing | Explain that Convenant runs live covenant state through MagicBlock ER; display Devnet and MagicBlock badges |
| Create | Collect bounded covenant terms, participants, optional amount, deadline, and privacy mode |
| Review | Show canonical terms, L1/ER execution plan, privacy disclosure, expected signatures, and any fee/latency disclaimer |
| Delegation | Show account-lock/delegation progress and prevent duplicate requests |
| Active ER session | Show “Active on MagicBlock ER,” live state/checkpoint updates, participant actions, and ER transaction identifiers |
| PER access | If privacy mode is enabled, show authorization/attestation state without exposing sensitive terms to unauthorized users |
| Commit/settle | Show commitment or undelegation progress and final L1 confirmation |
| Detail/history | Show complete lifecycle, state source, signatures, PDA, and Explorer links |
| Demo guide | Provide a deterministic two-wallet script and reset instructions |

### 10.2 Core user flow

The creator connects Wallet A and confirms Devnet. The creator defines terms and selects the covenant mode. The review screen explicitly states that the covenant will be initialized on Solana and delegated to a MagicBlock ER. Wallet A signs initialization and the delegation transaction(s). After confirmation, the detail page changes to `Active on ER` only after reading the actual delegated state, not after a local timer.

Wallet B opens the public covenant identifier and connects. The counterparty sees the terms permitted by the selected privacy mode. Wallet B accepts or performs the required action through the ER. The UI should feel immediate, but it must still show the signed transaction or ER execution result. The creator performs the next checkpoint or funding action. When the covenant’s condition is met, the parties trigger commitment/undelegation/settlement. The UI waits for the final Solana state and displays the base-layer proof.

If PER is enabled, the flow inserts `Attest/authorize → Access private state → Execute within PER → Commit permitted public result`.

## 11. Demo and demo-video plan

### 11.1 Live judging demo

The presenter should use two pre-funded Devnet wallets. The demo begins with the landing page showing that MagicBlock ER is the execution layer. Wallet A creates a covenant and pauses on the review screen. The presenter then shows delegation and the transition to `Active on ER`. Wallet B joins and performs the counterparty action. The presenter demonstrates at least one rapid state update or checkpoint that would be awkward to present as a normal one-off L1 transaction. Finally, the presenter commits or undelegates the state and shows the final Solana confirmation and Explorer link.

If PER is the selected MVP mode, the demo must visibly show that an unauthorized observer cannot read the sensitive state while an authorized participant can. Do not include PER only as a logo or slide; it must appear in an actual user-visible flow.

### 11.2 Recommended video storyboard

| Time | Scene | MagicBlock proof |
|---:|---|---|
| 0–10s | Product statement | Convenant is a real-time covenant application powered by MagicBlock |
| 10–25s | Create and review | User sees L1 initialization plus ER execution plan |
| 25–40s | Delegate | Account moves into active ER execution |
| 40–65s | Live two-party actions | Fast state updates occur in ER |
| 65–85s | Optional privacy | PER access control is shown if privacy is part of the MVP |
| 85–110s | Commit/undelegate | State returns to durable Solana settlement |
| 110–120s | Explorer and architecture | Final proof and MagicBlock-first diagram |

## 12. Required information, accounts, and infrastructure

Before coding, the team must obtain the exact MagicBlock hackathon track requirements, the approved ER endpoint/router configuration, current SDK and template versions, delegation/commitment/undelegation examples, supported local-development workflow, and the PER access-control and attestation requirements when confidential terms or execution state are part of the product.

The project requires a source repository, Solana Devnet RPC, MagicBlock ER access/configuration, a deployment key held only by the deployment environment, two separate demo wallets, and PER configuration when covenant terms or execution state must be confidential. No seed phrase or private key may enter the frontend, repository, issue tracker, terminal transcript, or chat.

```text
NEXT_PUBLIC_SOLANA_NETWORK=devnet
NEXT_PUBLIC_SOLANA_RPC_URL=<devnet-rpc>
NEXT_PUBLIC_MAGICBLOCK_ER_RPC_URL=<approved-er-endpoint>
NEXT_PUBLIC_MAGICBLOCK_ROUTER_URL=<approved-router-if-required>
NEXT_PUBLIC_CONVENANT_PROGRAM_ID=<program-id>
NEXT_PUBLIC_EXPLORER_CLUSTER=devnet
NEXT_PUBLIC_COVENANT_MODE=public-er
NEXT_PUBLIC_ENABLE_PER=false
```

The final variable choices must follow the current MagicBlock template. Public configuration may be exposed to the browser; deployment keys and any privileged credentials must not be prefixed with `NEXT_PUBLIC_`.

## 13. Testing strategy

### 13.1 Program and ER lifecycle tests

Tests must cover L1 initialization, valid delegation, invalid delegation, state visibility during delegation, permitted ER actions, wrong signer, stale state, invalid transition, duplicate action, commitment, undelegation, final L1 state, and recovery after failed or rejected execution. The tests must verify that the account is genuinely delegated and that the final state is not merely a frontend simulation.

### 13.2 PER tests

If PER is included, test unauthorized read/access, valid attestation, expired challenge, invalid access token, revoked or unsupported authorization, sensitive-state isolation, permitted public output, and final commitment behavior. Document exactly what remains public, what is confidential, what the TEE protects, and what trust remains in the hardware/operator environment.

### 13.3 Frontend and integration tests

Unit-test PDA derivation, terms hashing, state mapping, ER lifecycle mapping, endpoint selection, permission-aware action visibility, SOL/lamport conversions if escrow exists, PER access state, and Explorer links. Component-test delegation progress, active ER state, commit/settle progress, stale refresh, wallet rejection, RPC failure, and “status unknown” recovery.

Use a local validator or controlled Devnet fixtures. Do not make automated tests depend on a shared public wallet balance or mutable external state. Every successful integration test must re-fetch chain/ER state after confirmation.

## 14. Agent coding rules

The coding agent must start by reading the MagicBlock template README, ER quickstart, delegation/commitment/undelegation guidance, the PER quickstart and access-control guides when privacy is required, Anchor configuration, package manifest, and current hackathon instructions. It must record the exact SDK versions and choose a single compatible client stack.

The agent must implement the smallest vertical slice first: initialize on L1, delegate to ER, execute one meaningful action in ER, and commit/undelegate. It must not build a polished dashboard before proving that this lifecycle works. Every lifecycle transition must have a program test, a client integration test, and a visible UI state.

The agent must not fake MagicBlock integration with a local boolean, ordinary L1 RPC calls renamed as ER calls, hardcoded successful transaction signatures, or mocked state in the production path. Mocks are permitted only in isolated UI tests and must be clearly named.

The agent must keep MagicBlock integration behind typed adapters. It must centralize PDA seeds, program IDs, endpoint configuration, and lifecycle mapping. It must regenerate and version the IDL whenever the program changes. It must never mix incompatible web3.js versions, silently cast protocol objects to `any`, or swallow wallet/RPC errors.

The agent must not add PER casually. If the terms or execution state must be confidential, PER is mandatory and the agent must document the privacy boundary, authorization flow, attestation flow, and TEE assumption. If all information is intentionally public, the agent must use a standard ER and document that decision. No admin bypass, hidden reset, arbitrary payout recipient, private key, seed phrase, or mainnet fallback may be added for demo convenience.

## 15. Definition of done

The MagicBlock MVP is complete when a new developer can configure Devnet and the approved MagicBlock environment, start the Next.js app, connect two wallets, create a covenant on Solana, delegate the relevant state to an ER, perform at least one real covenant action through the ER, and commit or undelegate the resulting state back to Solana. The application must visibly distinguish L1, delegation, active ER, commitment, and settled states.

If PER is part of the MVP, the demo must prove the selected privacy boundary. The final state must be inspectable, the test suite must cover invalid lifecycle transitions, and the README must explain the exact MagicBlock setup.

## 16. Post-MVP roadmap

The first post-MVP increment should improve the ER lifecycle with richer checkpoints, resumable sessions, and indexing. The next should add PER if it was deferred, including formal access-control design and a threat model. Later work may include SPL assets, multi-party covenants, privacy-preserving documents, formal audits, compliance controls, and mainnet readiness. Each addition requires a new security review and must not silently expand the strict MVP.

## 17. Decisions required immediately

| Decision | Required answer |
|---|---|
| Product behavior | What real-world covenant or agreement does Convenant represent? |
| ER state | Which account or cluster is delegated, and what action occurs while active? |
| ER demo | What is the visibly fast interaction that proves ER value? |
| Privacy | Are covenant terms or execution state confidential? If yes, PER is required |
| Settlement | What exact state is committed or undelegated back to Solana? |
| Financial behavior | Is SOL/SPL escrow essential or distracting for the hackathon MVP? |
| Hackathon rubric | Which MagicBlock primitives must appear for eligibility or scoring? |

## References

[1]: https://docs.magicblock.gg/pages/get-started/introduction/ephemeral-rollup "MagicBlock Documentation, Ephemeral Rollup"

[2]: https://docs.magicblock.gg/pages/private-ephemeral-rollups-pers/introduction/onchain-privacy "MagicBlock Documentation, Onchain Privacy and Private Ephemeral Rollups"

[4]: https://nextjs.org/docs/app "Next.js Documentation, App Router"

[5]: https://www.anchor-lang.com/docs/clients/typescript "Anchor Documentation, TypeScript Client"

[6]: https://solana.com/docs/clients/official/javascript "Solana Documentation, JavaScript/TypeScript SDK"
