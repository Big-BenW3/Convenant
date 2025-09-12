/**
 * Proves the security fixes are live on the deployed devnet program by
 * attempting the original refund-theft against it.
 *
 * The counterparty signs a legitimate `expire_and_refund` but names a
 * third-party wallet as the `creator` recipient. Before the fix the vault PDA
 * signed the creator's locked SOL straight to that wallet; now the program's
 * `address = covenant.creator` constraint rejects it.
 *
 *   npx tsx scripts/verify-hardening.ts
 */
import { readFileSync } from "node:fs";
import {
  address,
  appendTransactionMessageInstructions,
  compileTransaction,
  createKeyPairSignerFromBytes,
  createTransactionMessage,
  getAddressEncoder,
  getBase64EncodedWireTransaction,
  getProgramDerivedAddress,
  setTransactionMessageFeePayerSigner,
  setTransactionMessageLifetimeUsingBlockhash,
  signTransaction,
  type Address,
  type Blockhash,
  type Instruction,
  type ReadonlyUint8Array,
  type TransactionSigner,
} from "@solana/kit";
import {
  getInitializeCovenantInstructionAsync,
  getJoinCovenantInstructionAsync,
  getExpireAndRefundInstructionAsync,
  AssetType,
} from "../app/generated/convenant";

const RPC = "https://rpc.magicblock.app/devnet";
const PROGRAM = address("2uY6gVNHe62XZXByt796pbwye1p7eTu2Nftv7uwC5CtQ");
const SYSTEM = address("11111111111111111111111111111111");
const LAMPORTS = 1_000_000_000;

export async function rpcCall<T = unknown>(
  method: string,
  params: unknown[],
  attempts = 5,
): Promise<T> {
  let lastError: unknown;
  for (let attempt = 0; attempt < attempts; attempt++) {
    try {
      const res = await fetch(RPC, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ jsonrpc: "2.0", id: 1, method, params }),
      });
      const json = (await res.json()) as { result?: T; error?: { message: string } };
      if (json.error) throw new Error(`${method}: ${json.error.message}`);
      return json.result as T;
    } catch (err) {
      lastError = err;
      // The devnet RPC drops connections intermittently; back off and retry.
      await new Promise((r) => setTimeout(r, 800 * (attempt + 1)));
    }
  }
  throw new Error(`${method} failed after ${attempts} attempts: ${String(lastError)}`);
}

async function balance(owner: Address): Promise<bigint> {
  const r = await rpcCall<{ value: number }>("getBalance", [owner]);
  return BigInt(r.value);
}

function loadSigner(path: string): Promise<TransactionSigner> {
  return createKeyPairSignerFromBytes(
    new Uint8Array(JSON.parse(readFileSync(path, "utf8"))),
  );
}

async function signAndSend(
  payer: TransactionSigner,
  instructions: Instruction[],
): Promise<string> {
  const { value: blockhashInfo } = await rpcCall<{
    value: { blockhash: string; lastValidBlockHeight: number };
  }>("getLatestBlockhash", []);
  const { blockhash, lastValidBlockHeight } = blockhashInfo;

  const base = createTransactionMessage({ version: 0 });
  const withPayer = setTransactionMessageFeePayerSigner(payer, base);
  const withIxs = appendTransactionMessageInstructions(instructions, withPayer);
  const withLifetime = setTransactionMessageLifetimeUsingBlockhash(
    {
      blockhash: blockhash as Blockhash,
      lastValidBlockHeight: BigInt(lastValidBlockHeight),
    },
    withIxs,
  );
  const compiled = compileTransaction(withLifetime);
  const signed = await signTransaction([(payer as unknown as { keyPair: never }).keyPair], compiled);
  const wire = getBase64EncodedWireTransaction(signed);

  return rpcCall<string>("sendTransaction", [
    wire,
    { encoding: "base64", skipPreflight: true, maxRetries: 0 },
  ]);
}

/**
 * Poll a signature until the transaction is confirmed, then report its
 * on-chain error. Returns null if it never lands. Note the important detail:
 * a transaction that *fails on chain* still comes back with `meta` populated,
 * so callers must check `err` rather than treating "has meta" as success.
 */
async function waitForConfirmation(
  sig: string,
): Promise<{ err: unknown; logs: string } | null> {
  for (let attempt = 0; attempt < 45; attempt++) {
    const tx = await rpcCall<{ meta?: { err?: unknown; logMessages?: string[] } } | null>(
      "getTransaction",
      [sig, { encoding: "json", maxSupportedTransactionVersion: 0 }],
    );
    if (tx?.meta) {
      return {
        err: tx.meta.err ?? null,
        logs: (tx.meta.logMessages ?? []).join("\n      "),
      };
    }
    await new Promise((r) => setTimeout(r, 1000));
  }
  return null;
}

async function pda(seeds: ReadonlyUint8Array[]): Promise<Address> {
  const [key] = await getProgramDerivedAddress({ programAddress: PROGRAM, seeds });
  return key;
}

async function main() {
  console.log("Verifying the deployed program rejects forged settlement recipients");
  console.log(`program ${PROGRAM}\n`);

  const creator = await loadSigner("/tmp/opencode/e2e-creator.json");
  const counterparty = await loadSigner("/tmp/opencode/e2e-cp.json");
  const attacker = await loadSigner("/tmp/opencode/counterparty.json");

  for (const [label, signer] of [
    ["creator", creator],
    ["counterparty", counterparty],
    ["attacker", attacker],
  ] as const) {
    const bal = await balance(signer.address);
    console.log(`${label.padEnd(13)} ${signer.address}  ${Number(bal) / LAMPORTS} SOL`);
    if (bal < BigInt(Math.floor(0.6 * LAMPORTS))) {
      console.log(`\nFUNDING NEEDED: ${label} has less than 0.6 SOL`);
      process.exit(1);
    }
  }

  // Nonce is a u64 little-endian seed, matching `nonce.to_le_bytes()` in Rust.
  const nonce = BigInt(Date.now());
  const nonceLe = new Uint8Array(8);
  for (let i = 0; i < 8; i++) {
    nonceLe[i] = Number((nonce >> BigInt(8 * i)) & 0xffn);
  }
  const covenant = await pda([
    new TextEncoder().encode("covenant"),
    getAddressEncoder().encode(creator.address),
    nonceLe,
  ]);
  const vault = await pda([
    new TextEncoder().encode("vault"),
    getAddressEncoder().encode(covenant),
  ]);
  const creatorLock = BigInt(Math.floor(0.3 * LAMPORTS));
  const cpLock = BigInt(Math.floor(0.2 * LAMPORTS));

  console.log(`\ncovenant ${covenant}`);
  console.log(`vault    ${vault}\n`);

  console.log("[1/3] initialize_covenant — creator locks 0.3 SOL");
  const initSig = await signAndSend(creator, [
    await getInitializeCovenantInstructionAsync({
      creator,
      covenant,
      vault,
      sourceTokenAccount: vault,
      vaultTokenAccount: vault,
      mintAccount: SYSTEM,
      tokenProgram: SYSTEM,
      systemProgram: SYSTEM,
      counterparty: counterparty.address,
      creatorAsset: AssetType.Sol,
      creatorMint: SYSTEM,
      creatorAmount: creatorLock,
      deadline: BigInt(Math.floor(Date.now() / 1000) + 120),
      nonce,
    }),
  ]);
  const initRes = await waitForConfirmation(initSig);
  if (!initRes || initRes.err) {
    console.log(`      FAILED: ${initRes?.logs ?? "(never confirmed)"}`);
    process.exit(1);
  }
  console.log(`      sig ${initSig}`);

  console.log("[2/3] join_covenant — counterparty locks 0.2 SOL");
  const joinSig = await signAndSend(counterparty, [
    await getJoinCovenantInstructionAsync({
      counterparty,
      covenant,
      vault,
      sourceTokenAccount: vault,
      vaultTokenAccount: vault,
      mintAccount: SYSTEM,
      tokenProgram: SYSTEM,
      systemProgram: SYSTEM,
      counterpartyAsset: AssetType.Sol,
      counterpartyMint: SYSTEM,
      counterpartyAmount: cpLock,
    }),
  ]);
  const joinRes = await waitForConfirmation(joinSig);
  if (!joinRes || joinRes.err) {
    console.log(`      FAILED: ${joinRes?.logs ?? "(never confirmed)"}`);
    process.exit(1);
  }
  console.log(`      sig ${joinSig}`);

  const vaultBefore = await balance(vault);
  const attackerBefore = await balance(attacker.address);
  console.log(`\n      vault holds ${Number(vaultBefore) / Number(LAMPORTS)} SOL`);

  console.log("\n[3/3] ATTACK — counterparty refunds with a forged `creator` recipient");
  let rejected = false;
  let detail = "";
  try {
    const attackSig = await signAndSend(counterparty, [
      await getExpireAndRefundInstructionAsync({
        signer: counterparty,
        covenant,
        vault,
        creator: attacker.address, // <- forged; must be rejected
        counterparty: counterparty.address,
        creatorVaultAta: vault,
        counterpartyVaultAta: vault,
        creatorPayoutAta: vault,
        counterpartyPayoutAta: vault,
        creatorMintAcct: SYSTEM,
        counterpartyMintAcct: SYSTEM,
        tokenProgram: SYSTEM,
        systemProgram: SYSTEM,
      }),
    ]);
    const res = await waitForConfirmation(attackSig);
    detail = res?.logs ?? "(no logs)";
    rejected = Boolean(res?.err);
  } catch (err) {
    rejected = true;
    detail = String(err);
  }

  const attackerAfter = await balance(attacker.address);
  const vaultAfter = await balance(vault);
  const gained = attackerAfter - attackerBefore;

  console.log(`      rejected : ${rejected}`);
  console.log(`      detail   : ${detail.split("\n").slice(-3).join(" | ").slice(0, 300)}`);
  console.log(`\nattacker gained : ${Number(gained) / Number(LAMPORTS)} SOL`);
  console.log(`vault lamports  : ${vaultBefore} -> ${vaultAfter}`);

  if (gained > 0n) {
    console.log("\nFAIL: the attacker extracted funds from the deployed program.");
    process.exit(1);
  }
  if (!rejected) {
    console.log("\nFAIL: the forged refund was not rejected.");
    process.exit(1);
  }
  console.log("\nPASS — deployed program rejected the forged refund and the escrow is intact.");
}

// Only run when invoked directly, so the helpers can be imported by other scripts.
if (process.argv[1]?.includes("verify-hardening")) {
  main().catch((err) => {
    console.error("error:", err instanceof Error ? (err.stack ?? err.message) : String(err));
    process.exit(1);
  });
}
