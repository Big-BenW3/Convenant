/**
 * Demo-token bootstrapper for the two-browser demo / live e2e.
 *
 * Creates (idempotently) the Convenant demo mints owned by the funding
 * wallet, pre-creates the e2e wallets' ATAs, and mints demo balances:
 *   - USDC (6 decimals): 1,000 per wallet
 *   - NFT (0 decimals):  1 per wallet
 *
 * Mint addresses are stable: they are the pubkeys of the keypairs at
 * /tmp/opencode/demo-usdc.json and /tmp/opencode/demo-nft.json, which are
 * also hardcoded as app/lib/covenant.ts `DEMO_MINTS`.
 *
 * Uses legacy @solana/web3.js so the fresh mint keypair can be an explicit
 * signer on the CreateAccount (matching the e2e script's legacy-web3 use).
 *
 * Run: npx tsx scripts/mint-demo-tokens.ts
 */
import { readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import https from "node:https";
import {
  type AccountMeta,
  Connection,
  Keypair,
  PublicKey,
  SYSVAR_RENT_PUBKEY,
  Transaction,
  TransactionInstruction,
} from "@solana/web3.js";
import { address } from "@solana/kit";
import {
  ATA_PROGRAM_ID,
  SYSTEM_PROGRAM_ID,
  TOKEN_PROGRAM_ID,
  ataAddress,
} from "../app/lib/covenant";

const url = "https://rpc.magicblock.app/devnet";
const SPL_TOKEN = new PublicKey(TOKEN_PROGRAM_ID);
const SPL_SYSTEM = new PublicKey(SYSTEM_PROGRAM_ID);
const ATA = new PublicKey(ATA_PROGRAM_ID);
const ATA_DISC = Uint8Array.from([112, 83, 122, 159, 174, 104, 244, 19]);
const DECIMALS = { USDC: 6, NFT: 0 } as const;
const MINT_AMOUNT = { USDC: 1_000_000_000n, NFT: 1n } as const; // 1000 USDC (6dp), 1 NFT
const MINT_ACCOUNT_SIZE = 82;

const WALLETS = [
  { label: "creator", keypair: Keypair.fromSecretKey(Uint8Array.from(JSON.parse(readFileSync("/tmp/opencode/e2e-creator.json", "utf8")) as number[])) },
  { label: "counterparty", keypair: Keypair.fromSecretKey(Uint8Array.from(JSON.parse(readFileSync("/tmp/opencode/e2e-cp.json", "utf8")) as number[])) },
] as const;

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

async function confirmTx(conn: Connection, sig: string): Promise<void> {
  for (let i = 0; i < 20; i++) {
    await sleep(2000);
    const st = await conn.getSignatureStatus(sig);
    if (st?.value) {
      if (st.value.err) throw new Error(`tx ${sig} failed: ${JSON.stringify(st.value.err)}`);
      return;
    }
  }
  throw new Error(`tx ${sig} not confirmed after 40s`);
}

async function airdropIfNeeded(conn: Connection, pubkey: PublicKey, minLamports: number): Promise<void> {
  for (let round = 0; round < 5; round++) {
    const bal = await conn.getBalance(pubkey);
    if (bal >= minLamports) return;
    console.log(`  airdropping 1 SOL to ${pubkey.toBase58()} (round ${round + 1})...`);
    const sig = await conn.requestAirdrop(pubkey, 1_000_000_000); // cap: 1 SOL per request
    await confirmTx(conn, sig);
    await sleep(1000);
  }
  const finalBal = await conn.getBalance(pubkey);
  console.log(`  final balance: ${finalBal / 1e9} SOL`);
}

function splIx(programId: PublicKey, keys: AccountMeta[], data: Uint8Array): TransactionInstruction {
  return new TransactionInstruction({ programId, keys: keys.map((k) => ({ ...k, pubkey: k.pubkey })), data: Buffer.from(data) });
}

function createAtaIx(payer: PublicKey, owner: PublicKey, mint: PublicKey, ata: PublicKey): TransactionInstruction {
  return splIx(ATA, [
    { pubkey: payer, isSigner: true, isWritable: true },
    { pubkey: ata, isSigner: false, isWritable: true },
    { pubkey: owner, isSigner: false, isWritable: false },
    { pubkey: mint, isSigner: false, isWritable: false },
    { pubkey: SPL_TOKEN, isSigner: false, isWritable: false },
    { pubkey: SPL_SYSTEM, isSigner: false, isWritable: false },
  ], Buffer.from(ATA_DISC));
}

function createSystemMintIx(
  payer: PublicKey,
  mint: PublicKey,
  lamports: number,
  space: number,
  owner: PublicKey,
): TransactionInstruction {
  const data = new Uint8Array(4 + 8 + 8 + 32);
  new DataView(data.buffer).setUint32(0, 0, true); // CreateAccount
  data.set(buf(BigInt(lamports)), 4);
  data.set(buf(BigInt(space)), 12);
  data.set(owner.toBytes(), 20);
  return splIx(SPL_SYSTEM, [
    { pubkey: payer, isSigner: true, isWritable: true },
    { pubkey: mint, isSigner: true, isWritable: true },
  ], data);
}

function initMintIx(mint: PublicKey, decimals: number, mintAuthority: PublicKey): TransactionInstruction {
  const data = new Uint8Array(1 + 1 + 32 + 1);
  data[0] = 0; // InitializeMint
  data[1] = decimals;
  data.set(mintAuthority.toBytes(), 2);
  data[34] = 0; // COption::None freeze authority
  return splIx(SPL_TOKEN, [
    { pubkey: mint, isSigner: false, isWritable: true },
    { pubkey: SYSVAR_RENT_PUBKEY, isSigner: false, isWritable: false },
  ], data);
}

function mintToIx(mint: PublicKey, to: PublicKey, mintAuthority: PublicKey, amount: bigint): TransactionInstruction {
  const data = new Uint8Array(1 + 8);
  data[0] = 7; // MintTo
  data.set(buf(amount), 1);
  return splIx(SPL_TOKEN, [
    { pubkey: mint, isSigner: false, isWritable: true },
    { pubkey: to, isSigner: false, isWritable: true },
    { pubkey: mintAuthority, isSigner: true, isWritable: false },
  ], data);
}

function buf(n: bigint): Buffer {
  const b = Buffer.alloc(8);
  b.writeBigUInt64LE(n);
  return b;
}

function httpsFetch(input: string | URL, init?: RequestInit): Promise<Response> {
  return new Promise((resolve, reject) => {
    const url = typeof input === "string" ? new URL(input) : input;
    const body = init?.body as string | undefined;
    const req = https.request(url.href, {
      method: init?.method ?? "POST",
      family: 4,
      headers: Object.fromEntries(Object.entries(init?.headers ?? {})),
    }, (res) => {
      let data = "";
      res.on("data", (c: Buffer) => { data += c; });
      res.on("end", () => {
        resolve(new Response(data, {
          status: res.statusCode ?? 200,
          statusText: res.statusMessage ?? "",
          headers: Object.fromEntries(Object.entries(res.headers).map(([k, v]) => [k, Array.isArray(v) ? v.join(",") : v ?? ""])),
        }));
      });
    });
    req.on("error", reject);
    if (body) req.write(body);
    req.end();
  });
}

async function main() {
  const payer = Keypair.fromSecretKey(
    Uint8Array.from(JSON.parse(readFileSync(join(homedir(), ".config", "solana", "id.json"), "utf8")) as number[]),
  );
  const connection = new Connection(url, {
    commitment: "confirmed",
    fetch: httpsFetch as typeof globalThis.fetch,
  });
  const mintKeys: { kind: "USDC" | "NFT"; keypair: Keypair }[] = [
    { kind: "USDC", keypair: Keypair.fromSecretKey(Uint8Array.from(JSON.parse(readFileSync("/tmp/opencode/demo-usdc.json", "utf8")) as number[])) },
    { kind: "NFT", keypair: Keypair.fromSecretKey(Uint8Array.from(JSON.parse(readFileSync("/tmp/opencode/demo-nft.json", "utf8")) as number[])) },
  ];

  console.log("payer (mint authority)", payer.publicKey.toBase58());
  console.log("payer bal", (await connection.getBalance(payer.publicKey)) / 1e9, "SOL");
  await airdropIfNeeded(connection, payer.publicKey, 2_000_000_000); // need 2 SOL for multiple txs

  for (const w of WALLETS) {
    await airdropIfNeeded(connection, w.keypair.publicKey, 500_000_000);
  }

  for (const m of mintKeys) {
    const kind = m.kind;
    const mint = m.keypair.publicKey;
    const info = await connection.getAccountInfo(mint);

    if (info === null) {
      const rent = await connection.getMinimumBalanceForRentExemption(MINT_ACCOUNT_SIZE);
      const createTx = new Transaction().add(
        createSystemMintIx(payer.publicKey, mint, rent, MINT_ACCOUNT_SIZE, SPL_TOKEN),
        initMintIx(mint, DECIMALS[kind], payer.publicKey),
      );
      const sig = await connection.sendTransaction(createTx, [payer, m.keypair], { skipPreflight: true });
      await confirmTx(connection, sig);
      await sleep(1500);
      console.log(`  mint ${kind} created ${mint.toBase58()}`, sig);
    } else {
      console.log(`  mint ${kind} exists ${mint.toBase58()}`);
    }

    for (const w of WALLETS) {
      const ata = new PublicKey(await ataAddress(address(w.keypair.publicKey.toBase58()), address(mint.toBase58())));
      const ataInfo = await connection.getAccountInfo(ata);
      const have = ataInfo === null ? 0n : BigInt((await connection.getTokenAccountBalance(ata)).value?.amount ?? 0);
      if (have >= MINT_AMOUNT[kind]) {
        console.log(`  ${w.label} ${kind} already funded (${have}) at ${ata.toBase58()}`);
        continue;
      }
      const tx = new Transaction();
      if (ataInfo === null) {
        tx.add(createAtaIx(payer.publicKey, w.keypair.publicKey, mint, ata));
      }
      tx.add(mintToIx(mint, ata, payer.publicKey, MINT_AMOUNT[kind]));
      const sig = await connection.sendTransaction(tx, [payer], { skipPreflight: true });
      await confirmTx(connection, sig);
      await sleep(800);
      console.log(`  minted ${MINT_AMOUNT[kind]} ${kind} -> ${w.label} ${ata.toBase58()}`, sig);
    }
  }

  console.log("\nFinal balances:");
  for (const m of mintKeys) {
    for (const w of WALLETS) {
      const ata = new PublicKey(await ataAddress(address(w.keypair.publicKey.toBase58()), address(m.keypair.publicKey.toBase58())));
      const info = await connection.getAccountInfo(ata);
      if (info === null) {
        console.log(`  ${w.label} ${m.kind} @ ${ata.toBase58()}: no account`);
        continue;
      }
      const bal = await connection.getTokenAccountBalance(ata);
      console.log(`  ${w.label} ${m.kind} @ ${ata.toBase58()}: ${bal.value ? bal.value.amount : 0}`);
    }
  }
}

main().then(
  () => process.exit(0),
  (e) => {
    console.error(e);
    process.exitCode = 1;
  },
);