import {
  address,
  getProgramDerivedAddress,
  getAddressEncoder,
  createRpc,
  createSolanaRpcApi,
  createDefaultRpcTransport,
  DEFAULT_RPC_CONFIG,
  unwrapOption,
  type Address,
  type Rpc,
  type GetAccountInfoApi,
  type Instruction,
  type Option,
  type TransactionSigner,
} from "@solana/kit";
import {
  AssetType,
  CovenantStatus,
  CONVENANT_PROGRAM_ADDRESS,
  findCovenantPda,
  findVaultPda,
  fetchMaybeCovenant,
  getCreateAssociatedTokenAccountInstructionAsync,
  type Covenant,
} from "../generated/convenant";

/**
 * Convenant domain helpers: PDA/ATA derivation, account reading, enum mapping,
 * amount formatting, and lightweight localStorage persistence for the dashboard.
 * Devnet-only by design.
 */

/** Convenant program id, sourced from the regenerated client (single source). */
export const PROGRAM_ID = CONVENANT_PROGRAM_ADDRESS;

/** Legacy SPL Token program (our contract manually derives ATAs against it). */
export const TOKEN_PROGRAM_ID =
  "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA" as const;
/**
 * Owner of every derived token account. MagicBlock does not deploy the SPL
 * Associated Token Account program, so Convenant derives its own token
 * accounts under itself and creates them via
 * `create_associated_token_account`. Despite the historical name this is NOT
 * the SPL ATA program address.
 */
export const ATA_PROGRAM_ID = PROGRAM_ID;
/** System program (SOL mint sentinel). */
export const SYSTEM_PROGRAM_ID = "11111111111111111111111111111111" as const;
/** MagicBlock PER access-control (permission) program. */
export const PERMISSION_PROGRAM_ID =
  "ACLseoPoyC3cBqoUtkbjZ4aDrkurZW86v19pXz2XQnp1" as const;

/**
 * Demo SPL mints (devnet) used by the create + join UIs. Owned by
 * `scripts/mint-demo-tokens.ts` (the mint-authority keypairs live in
 * /tmp/opencode/demo-{usdc,nft}.json) so the two-browser demo never depends
 * on pre-existing balances.
 */
export const DEMO_MINTS = {
  USDC: "5P99ZytaHyjfkcx5tp1yU2ZdKRCiNrvgDNBzz4dNkuU7" as const,
  NFT: "83o191eNgqRQWnEUHpRznTkbvXooVDNbPir8kH2EFW7p" as const,
} as const;

export type RpcClient = Rpc<GetAccountInfoApi>;

/** Build a minimal read-only RPC against the given base-layer endpoint. */
export function createReadRpc(url: string): RpcClient {
  const transport = createDefaultRpcTransport({ url });
  return createRpc({
    api: createSolanaRpcApi(DEFAULT_RPC_CONFIG),
    transport,
  });
}

/** Derive the covenant PDA (nonce-seeded so a creator can make many). */
export async function covenantAddress(
  creator: Address,
  nonce: number | bigint,
): Promise<Address> {
  const [pda] = await findCovenantPda({ creator, nonce });
  return pda;
}

/** Derive the vault PDA from the covenant address. */
export async function vaultAddress(covenant: Address): Promise<Address> {
  const [pda] = await findVaultPda({ covenant });
  return pda;
}

/**
 * MagicBlock PER permission PDA gating read/write access to the covenant on
 * the ephemeral rollup: `PDA(["permission:", covenant], PERMISSION_PROGRAM_ID)`.
 */
export async function permissionAddress(covenant: Address): Promise<Address> {
  const [pda] = await getProgramDerivedAddress({
    programAddress: PERMISSION_PROGRAM_ID as Address,
    seeds: [
      new TextEncoder().encode("permission:"),
      getAddressEncoder().encode(covenant),
    ],
  });
  return pda;
}

/**
 * Token account for `owner` holding `mint`, derived exactly as the program does
 * it: `PDA([owner, token_program, mint])` under the Convenant program id.
 */
export async function ataAddress(
  owner: Address,
  mint: Address,
): Promise<Address> {
  const [pda] = await getProgramDerivedAddress({
    programAddress: ATA_PROGRAM_ID as Address,
    seeds: [
      getAddressEncoder().encode(owner),
      getAddressEncoder().encode(TOKEN_PROGRAM_ID as Address),
      getAddressEncoder().encode(mint),
    ],
  });
  return pda;
}

/**
 * Build the instruction that creates a program-derived token account.
 *
 * MagicBlock does not deploy the SPL Associated Token Account program, so this
 * program derives its own token accounts under seeds
 * `[owner, token_program, mint]` and creates them on-chain. The instruction is
 * idempotent, so it is safe to prepend ahead of a lock or a settlement.
 *
 * `owner` may be a wallet or the covenant vault PDA.
 */
export async function buildCreateTokenAccount(
  payer: TransactionSigner,
  owner: Address,
  mint: Address,
): Promise<Instruction> {
  return getCreateAssociatedTokenAccountInstructionAsync({
    payer,
    ata: await ataAddress(owner, mint),
    owner,
    mint,
    tokenProgram: address(TOKEN_PROGRAM_ID),
    systemProgram: address(SYSTEM_PROGRAM_ID),
  });
}

/** True when an asset+mint pair represents native SOL. */
export function isSol(asset: AssetType, mint: Address): boolean {
  return asset === AssetType.Sol || mint === SYSTEM_PROGRAM_ID;
}

/** SOL-side dummy for SPL accounts that are `unused` (must still be valid metas). */
export const SOL_DUMMY: Address = SYSTEM_PROGRAM_ID as Address;

/** Whether to build payout ATAs for a swap (cross) or a refund (own asset back). */
export type SettlementMode = "swap" | "refund";

/**
 * Account set for a commit-and-swap / encrypt-and-refund. Payout semantics:
 *  - swap: creator receives counterparty's asset, counterparty receives creator's.
 *  - refund: each party receives their own asset back.
 * SOL → system-program dummy, exactly like the LiteSVM test wiring.
 */
export type SettlementAccounts = {
  creatorVaultAta: Address;
  counterpartyVaultAta: Address;
  creatorPayoutAta: Address;
  counterpartyPayoutAta: Address;
  creatorMintAcct: Address;
  counterpartyMintAcct: Address;
  vault: Address;
};

/**
 * The two committed parties. The counterparty is required once a covenant has
 * been joined (open-join covenants get it set on first join).
 */
export type Parties = { creator: Address; counterparty: Address };

/**
 * Resolves the counterparty from its nullable field, throwing when a covenant
 * has not been joined yet. Use it before any settlement/swap wiring.
 */
export function partyAddresses(c: {
  creator: Address;
  counterparty: Option<Address>;
}): Parties {
  const counterparty = unwrapOption(c.counterparty);
  if (counterparty === null) {
    throw new Error("covenant has no counterparty yet (not joined)");
  }
  return { creator: c.creator, counterparty };
}

/**
 * Derives the SPL/ATA accounts a settle or refund instruction needs from the
 * covenant's locked terms, using the same wiring as the contract tests.
 */
export async function settlementAccounts(
  c: Pick<
    Covenant,
    | "creator"
    | "counterparty"
    | "creatorAsset"
    | "creatorMint"
    | "counterpartyAsset"
    | "counterpartyMint"
  >,
  vault: Address,
  mode: SettlementMode = "swap",
): Promise<SettlementAccounts> {
  const { creator, counterparty } = partyAddresses(c);
  const creatorSol = isSol(c.creatorAsset, c.creatorMint);
  const cpSol = isSol(c.counterpartyAsset, c.counterpartyMint);
  const [creatorVaultAta, counterpartyVaultAta, creatorPayoutAta, counterpartyPayoutAta] =
    await Promise.all([
      creatorSol ? Promise.resolve(SOL_DUMMY) : ataAddress(vault, c.creatorMint),
      cpSol ? Promise.resolve(SOL_DUMMY) : ataAddress(vault, c.counterpartyMint),
      // Swap: creator receives counterparty's asset. Refund: creator gets own back.
      mode === "swap"
        ? cpSol
          ? Promise.resolve(SOL_DUMMY)
          : ataAddress(creator, c.counterpartyMint)
        : creatorSol
          ? Promise.resolve(SOL_DUMMY)
          : ataAddress(creator, c.creatorMint),
      // Swap: counterparty receives creator's asset. Refund: counterparty gets own back.
      mode === "swap"
        ? creatorSol
          ? Promise.resolve(SOL_DUMMY)
          : ataAddress(counterparty, c.creatorMint)
        : cpSol
          ? Promise.resolve(SOL_DUMMY)
          : ataAddress(counterparty, c.counterpartyMint),
    ]);
  return {
    creatorVaultAta,
    counterpartyVaultAta,
    creatorPayoutAta,
    counterpartyPayoutAta,
    creatorMintAcct: creatorSol ? (SOL_DUMMY as Address) : c.creatorMint,
    counterpartyMintAcct: cpSol ? (SOL_DUMMY as Address) : c.counterpartyMint,
    vault,
  };
}

/**
 * Token accounts that must exist before a settlement or refund can move SPL /
 * NFT value. `release_asset` transfers into a payout account, so each side's
 * destination has to be an initialised token account or the transfer fails.
 *
 * `accounts` must be the same object passed to the settlement instruction; the
 * owner/mint pairs are re-derived from the covenant so the accounts created here
 * are exactly the ones the instruction will pass.
 *
 * Returns an empty list for SOL-only covenants, where the vault PDA holds
 * lamports directly and no token account is involved.
 */
export async function settlementTokenAccountSetup(
  payer: TransactionSigner,
  covenant: Covenant,
  accounts: SettlementAccounts,
  mode: "swap" | "refund",
): Promise<Instruction[]> {
  const creator = unwrapOption(covenant.counterparty) ?? covenant.creator;
  const counterparty = unwrapOption(covenant.counterparty);
  const setups: Array<Promise<Instruction>> = [];

  const add = (owner: Address, mint: Address) => {
    if (isSol(AssetType.Token, mint)) return;
    setups.push(buildCreateTokenAccount(payer, owner, mint));
  };

  // The vault's holding account for whichever side offered a token.
  add(accounts.vault, covenant.creatorMint);
  add(accounts.vault, covenant.counterpartyMint);

  // Each side's payout destination. On a swap the creator receives the
  // counterparty's mint and vice versa; a refund returns each side to itself.
  if (counterparty) {
    add(creator, mode === "swap" ? covenant.counterpartyMint : covenant.creatorMint);
    add(counterparty, mode === "swap" ? covenant.creatorMint : covenant.counterpartyMint);
  }

  return Promise.all(setups);
}

/**
 * Reads the covenant account at the given address, returning the decoded
 * `Covenant` (or null if the account is absent). Pairs with a read RPC client.
 */
export async function readCovenant(
  rpc: RpcClient,
  covenantKey: Address,
): Promise<Covenant | null> {
  const maybe = await fetchMaybeCovenant(rpc, covenantKey);
  return maybe.exists ? maybe.data : null;
}

/** Human label for an asset type. */
export function assetLabel(asset: AssetType): string {
  switch (asset) {
    case AssetType.Sol:
      return "SOL";
    case AssetType.Token:
      return "TOKEN";
    case AssetType.Nft:
      return "NFT";
    default:
      return "ITEM";
  }
}

/**
 * Formats a raw covenant amount for display. `decimals` applies to token/NFT
 * amounts; SOL amounts are lamports and shown as SOL.
 */
export function formatAmount(
  amount: bigint,
  asset: AssetType,
  decimals = 9,
): string {
  if (asset !== AssetType.Sol && decimals > 0) {
    const divisor = 10n ** BigInt(decimals);
    const whole = amount / divisor;
    const frac = amount % divisor;
    if (frac === 0n) return whole.toString();
    const pad = frac.toString().padStart(decimals, "0");
    const trimmed = pad.replace(/0+$/, "");
    return trimmed === "" ? whole.toString() : `${whole}.${trimmed}`;
  }
  return amount.toString();
}

/** How to render each covenant status. */
export function statusMeta(status: CovenantStatus): {
  label: string;
  kind: "idle" | "active" | "ok" | "warn" | "err";
} {
  switch (status) {
    case CovenantStatus.CreatedOnL1:
      return { label: "CREATED · L1", kind: "idle" };
    case CovenantStatus.Delegating:
      return { label: "DELEGATING", kind: "warn" };
    case CovenantStatus.ActiveOnEr:
      return { label: "ACTIVE · ER", kind: "active" };
    case CovenantStatus.Settling:
      return { label: "SETTLING", kind: "warn" };
    case CovenantStatus.SettledOnL1:
      return { label: "SETTLED · L1", kind: "ok" };
    case CovenantStatus.Reverted:
      return { label: "REVERTED", kind: "err" };
    default:
      return { label: "UNKNOWN", kind: "idle" };
  }
}

// ---------------------------------------------------------------------
// Local vessel list (no indexer in the MVP) — persist covenant IDs in
// localStorage so the dashboard can list the connected wallet's covenants
// when they return.
// ---------------------------------------------------------------------

const VESSELS_KEY = "convenant:known-vessels";

export function loadKnownCovenants(): string[] {
  if (typeof window === "undefined") return [];
  try {
    const raw = localStorage.getItem(VESSELS_KEY);
    const parsed = raw ? (JSON.parse(raw) as string[]) : [];
    return Array.isArray(parsed) ? parsed : [];
  } catch {
    return [];
  }
}

export function rememberCovenant(idOrAddress: string) {
  if (typeof window === "undefined") return;
  const existing = loadKnownCovenants();
  if (!existing.includes(idOrAddress)) {
    existing.push(idOrAddress);
    localStorage.setItem(VESSELS_KEY, JSON.stringify(existing));
  }
}

export function forgetCovenant(idOrAddress: string) {
  if (typeof window === "undefined") return;
  const next = loadKnownCovenants().filter((id) => id !== idOrAddress);
  localStorage.setItem(VESSELS_KEY, JSON.stringify(next));
}