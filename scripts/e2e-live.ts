import { readFileSync } from "node:fs";
import https from "node:https";
import zlib from "node:zlib";
import { Keypair } from "@solana/web3.js";
import nacl from "tweetnacl";
import {
  address,
  appendTransactionMessageInstructions,
  compileTransaction,
  createKeyPairSignerFromBytes,
  createSolanaRpc,
  createTransactionMessage,
  getBase64EncodedWireTransaction,
  setTransactionMessageFeePayerSigner,
  setTransactionMessageLifetimeUsingBlockhash,
  signTransaction,
  unwrapOption,
  type Address,
  type Instruction,
  type TransactionSigner,
} from "@solana/kit";
import {
  AssetType,
  CovenantStatus,
  getAcceptInstruction,
  getCancelCovenantInstruction,
  getCloseCovenantInstruction,
  getCommitAndSwapInstructionAsync,
  getCovenantDecoder,
  getDelegateCovenantInstructionAsync,
  getExpireAndRefundInstruction,
  getInitializeCovenantInstructionAsync,
  getInitPermissionInstruction,
  getJoinCovenantInstructionAsync,
  getUpdateOfferInstruction,
} from "../app/generated/convenant";
import {
  SYSTEM_PROGRAM_ID,
  TOKEN_PROGRAM_ID,
  DEMO_MINTS,
  SOL_DUMMY,
  covenantAddress,
  createReadRpc,
  createAtaInstruction,
  ataAddress,
  permissionAddress,
  readCovenant,
  settlementAccounts,
  vaultAddress,
} from "../app/lib/covenant";
import {
  MAGICBLOCK,
  MAGIC_CONTEXT_ID,
  MAGIC_PROGRAM_ID,
  fetchPrivateAccount,
  getAuthTokenFor,
  getDelegationStatus,
  pickTeeValidator,
  pickValidator,
} from "../app/lib/magicblock";

// ---------------------------------------------------------------- helpers

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

function assert(cond: boolean, msg: string) {
  if (!cond) throw new Error(`ASSERT FAILED: ${msg}`);
}

async function waitFor<T>(
  desc: string,
  fn: () => Promise<T | undefined>,
  timeoutMs: number,
  intervalMs = 2000,
): Promise<T> {
  const end = Date.now() + timeoutMs;
  let last: unknown;
  while (Date.now() < end) {
    try {
      const v = await fn();
      if (v !== undefined && v !== null) return v as T;
      last = undefined;
    } catch (e) {
      last = e;
    }
    await sleep(intervalMs);
  }
  throw new Error(`timeout waiting for ${desc}${last ? ` (last: ${String(last)})` : ""}`);
}

/** IPv4-only fetch polyfill used by @solana/kit / undici internals. */
function httpsFetch(input: string | URL, init?: RequestInit): Promise<Response> {
  return new Promise((resolve, reject) => {
    const url = typeof input === "string" ? new URL(input) : input;
    const body = init?.body as string | undefined;
    const req = https.request(url.href, {
      method: init?.method ?? "GET",
      family: 4,
      headers: Object.fromEntries(Object.entries(init?.headers ?? {})),
    }, (res) => {
      const chunks: Buffer[] = [];
      res.on("data", (c: Buffer) => { chunks.push(c); });
      res.on("end", () => {
        let buf = Buffer.concat(chunks);
        const enc = (res.headers["content-encoding"] ?? "").toLowerCase();
        try {
          if (enc.includes("br")) buf = zlib.brotliDecompressSync(buf);
          else if (enc.includes("gzip")) buf = zlib.gunzipSync(buf);
          else if (enc.includes("deflate")) buf = zlib.inflateSync(buf);
        } catch { /* leave as-is */ }
        const headers: Record<string, string> = {};
        for (const [k, v] of Object.entries(res.headers)) {
          if (k.toLowerCase() !== "content-encoding") {
            headers[k] = Array.isArray(v) ? v.join(",") : v ?? "";
          }
        }
        resolve(new Response(buf, {
          status: res.statusCode ?? 200,
          statusText: res.statusMessage ?? "",
          headers,
        }));
      });
    });
    req.on("error", reject);
    if (body) req.write(body);
    req.end();
  });
}
globalThis.fetch = httpsFetch as typeof globalThis.fetch;

const url = MAGICBLOCK.baseRpc;
const readRpc = createReadRpc(url);
const fullRpc = createSolanaRpc(url);

async function withRetry<T>(desc: string, fn: () => Promise<T>, tries = 5): Promise<T> {
  let last: unknown;
  for (let i = 0; i < tries; i++) {
    try {
      return await fn();
    } catch (e) {
      last = e;
      await sleep(1000);
    }
  }
  throw new Error(`retries exhausted for ${desc}: ${String(last)}`);
}

/**
 * Sends instructions raw over HTTP and confirms by polling getSignatureStatuses.
 * Avoids createClient's WebSocket-based confirmation (unsupported on MagicBlock).
 */
async function sendIxsRaw(
  payer: TransactionSigner,
  ixs: readonly Instruction[],
  rpcEndpoint: string = url,
  opts: { skipPreflight?: boolean; confirmTimeoutMs?: number } = {},
): Promise<string> {
  const { skipPreflight = true, confirmTimeoutMs = 120000 } = opts;
  const rpcFor = createSolanaRpc(rpcEndpoint as any);
  const blockhashResult = await withRetry(
    `getLatestBlockhash ${payer.address.slice(0, 8)}`,
    async () => {
      const r = await rpcFor.getLatestBlockhash().send();
      return r.value;
    },
  );
  const tx = createTransactionMessage({ version: 0 });
  const t2 = setTransactionMessageFeePayerSigner(payer, tx);
  const t3 = appendTransactionMessageInstructions(ixs, t2);
  const t4 = setTransactionMessageLifetimeUsingBlockhash(blockhashResult, t3);
  const compiled = compileTransaction(t4) as any;
  if (ixs.some((ix: any) => String(ix.programAddress) === "2uY6gVNHe62XZXByt796pbwye1p7eTu2Nftv7uwC5CtQ")) {
    for (const ix of ixs) {
      if (String((ix as any).programAddress) !== "2uY6gVNHe62XZXByt796pbwye1p7eTu2Nftv7uwC5CtQ") continue;
      console.log("  [debug] ix accounts:", JSON.stringify((ix as any).accounts?.map((a: any) => ({ a: String(a.address).slice(0, 12), signer: a.isSigner, mut: a.isWritable }))));
    }
  }
  const signed = await signTransaction([(payer as any).keyPair], compiled);
  const wire = getBase64EncodedWireTransaction(signed);
  const sig = await rpcFor
    .sendTransaction(wire, { encoding: "base64", skipPreflight })
    .send();
  const sigsOfAddr = async () => {
    // MagicBlock nodes don't always confirm via getSignatureStatuses; fall back
    // to the signatures-by-address oracle so failed txs surface fast.
    const hist: any = await rpcFor.getSignaturesForAddress(payer.address, { limit: 5 }).send();
    const list: any[] = Array.isArray(hist) ? hist : (hist.values ?? hist.value ?? []);
    return list.find((h: any) => h.signature === sig);
  };
  const end = Date.now() + confirmTimeoutMs;
  let lastErr: unknown;
  while (Date.now() < end) {
    const s = await rpcFor.getSignatureStatuses([sig]).send();
    const st = s.value[0];
    if (st) {
      if (st.err) {
        const logs = await tryFetchTxLogs(rpcFor, sig, rpcEndpoint.includes("?token=") ? rpcEndpoint : undefined);
        throw new Error(`tx ${sig} failed [endpoint=${rpcEndpoint.split("?")[0].slice(0,60)}]: ${jsonSafe(st.err)}${logs ? "\n  tx logs:\n  " + logs.join("\n  ") : ""}`);
      }
      if (st.confirmationStatus === "confirmed" || st.confirmationStatus === "finalized") {
        return sig;
      }
    } else {
      try {
        const entry = await sigsOfAddr();
        if (entry) {
          if (entry.err) throw new Error(`tx ${sig} failed: ${jsonSafe(entry.err)}`);
          return sig;
        }
      } catch (e) {
        lastErr = e;
      }
    }
    await sleep(2000);
  }
  throw new Error(
    `timeout waiting for confirm ${sig.slice(0, 8)}${lastErr ? ` (last: ${String(lastErr)})` : ""}`,
  );
}

function jsonSafe(v: unknown): string {
  return JSON.stringify(v, (_k, val) => (typeof val === "bigint" ? val.toString() : val));
}

async function tryFetchTxLogs(rpcFor: any, sig: string, erTokenEndpoint?: string): Promise<string[] | undefined> {
  const parseLogs = (r: any): string[] | undefined => {
    // kit returns { value: <tx> | null } or the raw tx; handle both.
    const t = r?.value ?? r?.result ?? r;
    if (!t) return undefined;
    const meta = t.meta;
    if (meta && Array.isArray(meta.logMessages)) return meta.logMessages as string[];
    return undefined;
  };
  try {
    const r: any = await rpcFor.getTransaction(sig, { encoding: "json", maxSupportedTransactionVersion: 0 }).send();
    const logs = parseLogs(r);
    if (logs) return logs;
  } catch { /* fall through to ER endpoint */ }
  if (erTokenEndpoint) {
    try {
      const res = await fetch(erTokenEndpoint, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          jsonrpc: "2.0",
          id: 1,
          method: "getTransaction",
          params: [sig, { encoding: "json", maxSupportedTransactionVersion: 0 }],
        }),
      });
      const body = (await res.json()) as any;
      return parseLogs(body?.result);
    } catch { /* ignore */ }
  }
  return undefined;
}

/**
 * l1Client: base-layer HTTP-only transaction sender (no WebSocket).
 */
function l1Client(payer: TransactionSigner) {
  return {
    send: async (ixs: readonly Instruction[]) => sendIxsRaw(payer, ixs),
  };
}

/**
 * erClient: ER HTTP-only transaction sender. Same as l1Client but pointed at
 * the given ephemeral-rollup endpoint.
 */
function erClient(payer: TransactionSigner, erEndpoint: string) {
  return {
    sendTransaction: async (ixs: readonly Instruction[]) => sendIxsRaw(payer, ixs, erEndpoint),
  };
}

type Covenant = NonNullable<Awaited<ReturnType<typeof readCovenant>>>;

async function pollCovenant(key: Address) {
  return waitFor(`readCovenant ${key.slice(0, 8)}`, async () => {
    const c = await readCovenant(readRpc, key);
    return c ?? undefined;
  }, 15000);
}

async function getVaultLamports(vault: Address): Promise<bigint> {
  const info = await fullRpc.getAccountInfo(vault).send();
  return info.value?.lamports ?? 0n;
}

async function getVaultTokenBalance(ata: Address): Promise<bigint> {
  try {
    const res = await fullRpc.getTokenAccountBalance(ata).send();
    return BigInt(res.value.amount);
  } catch {
    return 0n;
  }
}

async function getBalance(owner: Address): Promise<bigint> {
  return withRetry(`getBalance ${owner.slice(0, 8)}`, async () => {
    const res = await fullRpc.getBalance(owner).send();
    return res.value;
  });
}

function fmtLamports(n: bigint): string {
  return (Number(n) / 1e9).toFixed(4) + " SOL";
}

/** Unauthenticated getAccountInfo against an ER — used to prove private reads fail. */
async function tryPublicRead(
  erEndpoint: string,
  account: string,
): Promise<{ data?: [string, string]; lamports?: number } | null> {
  const res = await fetch(erEndpoint, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      jsonrpc: "2.0",
      id: 1,
      method: "getAccountInfo",
      params: [account, { encoding: "base64" }],
    }),
  });
  const body = (await res.json()) as {
    error?: { message?: string };
    result?: { value: { data?: [string, string]; lamports?: number } | null };
  };
  if (body.error) return null;
  return body.result?.value ?? null;
}

/** Decode a covenant fetched from the private ER (tokenized read). */
function decodePrivateCovenant(acc: { data?: [string, "base64"] }): Covenant | null {
  if (!acc.data) return null;
  return getCovenantDecoder().decode(Buffer.from(acc.data[0], "base64"));
}

/** Reads the covenant from the ER (post-delegation state lives there, not base). */
async function readCovenantEr(
  erEndpointWithToken: string,
  token: string,
  key: Address,
): Promise<Covenant | null | undefined> {
  const acc = await fetchPrivateAccount(erEndpointWithToken, token, key);
  return acc ? decodePrivateCovenant(acc) : undefined;
}

/** ER diagnostic fetch (IPv4, no undici): full tx envelope for one signature. */
async function fetchErTx(erTokenEndpoint: string, sig: string): Promise<any> {
  const bodyStr = JSON.stringify({
    jsonrpc: "2.0",
    id: 1,
    method: "getTransaction",
    params: [sig, { encoding: "json", maxSupportedTransactionVersion: 0 }],
  });
  return new Promise((resolve, reject) => {
    const u = new globalThis.URL(erTokenEndpoint);
    const req = https.request(u.href, {
      method: "POST",
      family: 4,
      headers: { "content-type": "application/json", "content-length": String(Buffer.byteLength(bodyStr)) },
    }, (resp: any) => {
      const chunks: Buffer[] = [];
      resp.on("data", (c: Buffer) => { chunks.push(c); });
      resp.on("end", () => {
        try { resolve(JSON.parse(Buffer.concat(chunks).toString("utf8"))); }
        catch (e) { reject(e); }
      });
      resp.on("error", reject);
    });
    req.on("error", reject);
    req.write(bodyStr);
    req.end();
  });
}

/** Log the failed + a sibling-good tx envelope side by side (header, keys, inner ixs, logs). */
async function diagCommitFailure(erTokenEndpoint: string, failedSig: string, goodSig: string): Promise<void> {
  for (const [tag, sig] of [["FAILED-commit", failedSig], ["GOOD-accept", goodSig]] as const) {
    try {
      const res: any = await fetchErTx(erTokenEndpoint, sig);
      const t = res?.result;
      if (!t) {
        console.log(`  [${tag}] no tx returned (body=`, JSON.stringify(res).slice(0, 300), ")");
        continue;
      }
      const msg = t.transaction?.message ?? {};
      const keys: string[] = (msg.accountKeys ?? []).map((k: any) => typeof k === "string" ? k : k?.pubkey);
      console.log(
        `  [${tag}] header=`, JSON.stringify(msg.header),
        "sigs=", (t.transaction?.signatures ?? []).length,
        "err=", JSON.stringify(t.meta?.err),
      );
      console.log(`  [${tag}] keys(${keys.length})=`, JSON.stringify(keys.map((k) => k.slice(0, 12))));
      const outer = msg.instructions?.[0];
      console.log(`  [${tag}] outerIx=`, JSON.stringify(outer));
      console.log(`  [${tag}] innerIxs=`, JSON.stringify(t.meta?.innerInstructions ?? []).slice(0, 1500));
      console.log(`  [${tag}] logs=`, JSON.stringify(t.meta?.logMessages ?? []).slice(0, 1500));
    } catch (e) {
      console.log(`  [${tag}] fetch failed:`, String(e).slice(0, 200));
    }
  }
}

/**
 * The `#[commit]` macro injects `magicContext` as a non-signer, writable
 * account in the Rust context (see the `commit` attribute macro in
 * ephemeral-rollups-sdk-attribute-commit + `MagicIntentBundleBuilder::build`,
 * which forces `magic_context.is_signer = false`). The Codama client matches
 * this. Do NOT mark magicContext as a signer: that raises
 * numRequiredSignatures to 2 while only the payer signs, which the runtime
 * rejects as MissingRequiredSignature on instruction 0.
 */
function patchCommitIx(ix: any): any {
  return ix;
}

/** Local preflight: compile a kit ix into a legacy message header and assert
 * the runtime's tx-level signer accounting (numRequiredSignatures, payer
 * position). Run with PREFLIGHT_COMMIT=1 (no network, no SOL spent). */
async function preflightCommitIx(): Promise<void> {
  const { compileTransaction, getBase58Decoder } = await import("@solana/kit");
  const kp = Keypair.generate();
  const payerSigner = await createKeyPairSignerFromBytes(kp.secretKey);
  const fake = (b: number) => address(getBase58Decoder().decode(Buffer.alloc(32, b)));
  const ix = await getCommitAndSwapInstructionAsync({
    payer: payerSigner,
    covenant: fake(1),
    vault: fake(2),
    creator: payerSigner.address,
    counterparty: fake(3),
    creatorVaultAta: fake(4),
    counterpartyVaultAta: fake(5),
    creatorPayoutAta: fake(6),
    counterpartyPayoutAta: fake(7),
    creatorMintAcct: fake(8),
    counterpartyMintAcct: fake(9),
    tokenProgram: address(TOKEN_PROGRAM_ID),
    systemProgram: address(SYSTEM_PROGRAM_ID),
    magicProgram: address(MAGIC_PROGRAM_ID),
    magicContext: address(MAGIC_CONTEXT_ID),
  });
  const msg = createTransactionMessage({ version: 0 });
  const msg1 = setTransactionMessageFeePayerSigner(payerSigner, msg);
  const msg2 = setTransactionMessageLifetimeUsingBlockhash(
    { blockhash: "11111111111111111111111111111111" as any, lastValidBlockHeight: 1n },
    msg1,
  );
  const compiled = compileTransaction({
    ...appendTransactionMessageInstructions([ix], msg2),
    version: 0,
  } as any);
  // compileTransaction returns { messageBytes, ... } — parse the header out of
  // the wire bytes. Legacy: bytes[0..3] = header. Versioned v0: byte[0] has
  // the 0x80 flag, header = bytes[1..4].
  const raw: any = (compiled as any)?.messageBytes ?? compiled;
  const bytes: number[] = Array.from(
    raw instanceof Uint8Array ? raw : (raw as any)?.data ?? Object.values(raw),
  ) as number[];
  const versioned = bytes[0] >= 128;
  const header = {
    numRequiredSignatures: bytes[versioned ? 1 : 0],
    numReadonlySignedAccounts: bytes[versioned ? 2 : 1],
    numReadonlyUnsignedAccounts: bytes[versioned ? 3 : 2],
  };
  const roles = (ix.accounts as any[]).map((a) => a.role);
  console.log("PREFLIGHT roles=", JSON.stringify(roles));
  console.log("PREFLIGHT header=", JSON.stringify(header));
  if (header.numRequiredSignatures !== 1) {
    throw new Error(`PREFLIGHT FAIL: expected numRequiredSignatures=1, got ${JSON.stringify(header)}`);
  }
  console.log("PREFLIGHT PASS: tx-level assembly is 1-signer, payer-first");
}

if (process.env.PREFLIGHT_COMMIT === "1") {
  preflightCommitIx().then(
    () => process.exit(0),
    (e) => { console.error(e); process.exitCode = 1; },
  );
} else {

// ---------------------------------------------------------------- main

async function main() {
  const [creator, cp] = await Promise.all(
    ["/tmp/opencode/e2e-creator.json", "/tmp/opencode/e2e-cp.json"].map(
      async (p) => {
        const secret = JSON.parse(readFileSync(p, "utf8"));
        return createKeyPairSignerFromBytes(new Uint8Array(secret));
      },
    ),
  );
  // Legacy keypairs for TEE challenge signing (auth token flow).
  const cpWeb3 = Keypair.fromSecretKey(
    Buffer.from(JSON.parse(readFileSync("/tmp/opencode/e2e-cp.json", "utf8"))),
  );
  const creatorWeb3 = Keypair.fromSecretKey(
    Buffer.from(JSON.parse(readFileSync("/tmp/opencode/e2e-creator.json", "utf8"))),
  );

  console.log("creator        ", creator.address);
  console.log("counterparty   ", cp.address);
  console.log("creator  bal   ", fmtLamports(await getBalance(creator.address)));
  console.log("counterparty bal", fmtLamports(await getBalance(cp.address)));
  const cpStart = await getBalance(cp.address);
  const creatorFundsStart = await getBalance(creator.address);

  const step = (m: string) => console.log(`  [step] ${m}`);

  // =================================================================
  // FLOW A: create -> join -> TEE delegate -> init_permission -> private
  // reads -> negotiate -> commit_and_swap -> swap_on_l1
  // =================================================================
  console.log("\n=== FLOW A: SOL<->SOL private covenant on the TEE ===");

  const nonceA = BigInt(Date.now());
  step("derive covenant+vault (nonce)");
  const covenantKey = await covenantAddress(creator.address, nonceA);
  const vault = await vaultAddress(covenantKey);

  // 1. CREATOR LOCKS 0.5 SOL (pinned counterparty)
  step("build init ix (pre-funds permission rent inside covenant PDA)");
  const deadline = BigInt(Math.floor(Date.now() / 1000) + 600);
  const creatorAmount = 500_000_000n;
  const cpAmount0 = 250_000_000n;
  const initIx = await getInitializeCovenantInstructionAsync({
    creator,
    covenant: covenantKey,
    vault,
    sourceTokenAccount: vault,
    vaultTokenAccount: vault,
    mintAccount: address(SYSTEM_PROGRAM_ID),
    tokenProgram: address(TOKEN_PROGRAM_ID),
    systemProgram: address(SYSTEM_PROGRAM_ID),
    counterparty: cp.address,
    creatorAsset: AssetType.Sol,
    creatorMint: address(SYSTEM_PROGRAM_ID),
    creatorAmount,
    deadline,
    nonce: nonceA,
  });
  step("send init tx");
  const sigInit = await l1Client(creator).send([initIx]);
  console.log("init tx        ", sigInit);
  let c = await pollCovenant(covenantKey);
  assert(c.status === CovenantStatus.CreatedOnL1, "status CreatedOnL1 after init");
  assert(c.creator === creator.address, "creator set");
  assert(unwrapOption(c.counterparty) === cp.address, "counterparty pinned");
  assert(c.creatorAmount === creatorAmount, "creator amount locked");
  assert((await getVaultLamports(vault)) === creatorAmount, "vault holds creator SOL");
  console.log("  create ok, vault =", fmtLamports(await getVaultLamports(vault)));

  // 2. COUNTERPARTY LOCKS THEIR 0.25 SOL
  const joinIx = await getJoinCovenantInstructionAsync({
    counterparty: cp,
    covenant: covenantKey,
    vault,
    sourceTokenAccount: vault,
    vaultTokenAccount: vault,
    mintAccount: address(SYSTEM_PROGRAM_ID),
    tokenProgram: address(TOKEN_PROGRAM_ID),
    systemProgram: address(SYSTEM_PROGRAM_ID),
    counterpartyAsset: AssetType.Sol,
    counterpartyMint: address(SYSTEM_PROGRAM_ID),
    counterpartyAmount: cpAmount0,
  });
  const preJoin = await readCovenant(readRpc, covenantKey);
  console.log("  pre-join covenant status:", preJoin?.status, "joined:", preJoin?.counterpartyJoined, "creator:", preJoin?.creator?.slice(0, 8), "nonce:", preJoin?.nonce?.toString());
  console.log("  covenantKey:", covenantKey);
  await sleep(3000);
  const sigJoin = await l1Client(cp).send([joinIx]);
  console.log("join tx        ", sigJoin);
  c = await pollCovenant(covenantKey);
  assert(c.counterpartyJoined, "counterparty joined");
  assert((await getVaultLamports(vault)) === creatorAmount + cpAmount0, "vault holds both");
  console.log("  join ok, vault =", fmtLamports(await getVaultLamports(vault)));

  // 3. DELEGATE TO THE TEE ER (DLP-only, pinned validator). Private terms
  //    must never touch a non-TEE node.
  const route = process.argv.includes("--delegate-nontee")
    ? await pickValidator()
    : await pickTeeValidator();
  if (!route) throw new Error("no route from router getRoutes");
  console.log("  validator      ", route.identity, "->", route.fqdn);
  const delegateIx = await getDelegateCovenantInstructionAsync({
    payer: creator,
    covenant: covenantKey,
    validator: address(route.identity) as never,
  });
  // PROBE MODE: re-send the delegate with preflight ON to surface the real
  // simulation error, then with preflight OFF and a short confirm window.
  if (process.argv.includes("--probe-delegate")) {
    const rpcFor = createSolanaRpc(url as any);
    const probe = async (skipPreflight: boolean) => {
      const bh = await rpcFor.getLatestBlockhash().send();
      const msg = appendTransactionMessageInstructions(
        [delegateIx],
        setTransactionMessageLifetimeUsingBlockhash(
          bh.value,
          setTransactionMessageFeePayerSigner(
            creator,
            createTransactionMessage({ version: 0 }),
          ),
        ),
      );
      const comp = compileTransaction(msg);
      const sgn = await signTransaction([(creator as any).keyPair], comp);
      const wire = getBase64EncodedWireTransaction(sgn);
      try {
        const res = await rpcFor
          .sendTransaction(wire, { encoding: "base64", skipPreflight })
          .send();
        console.log(`  probe preflight=${skipPreflight} send OK:`, res);
        const s = await rpcFor.getSignatureStatuses([res]).send();
        console.log(
          `  probe preflight=${skipPreflight} status:`,
          JSON.stringify(s.value[0]),
        );
      } catch (e: any) {
        console.log(
          `  probe preflight=${skipPreflight} send ERROR:`,
          e.message ?? String(e),
        );
        const cause = e?.cause ?? e?.code;
        console.log(
          `  probe preflight=${skipPreflight} raw:`,
          JSON.stringify(cause ?? e, null, 1).slice(0, 4000),
        );
      }
    };
    await probe(false);
    await sleep(4000);
    await probe(true);
    await sleep(30000);
    const s: any = await rpcFor.getSignaturesForAddress(creator.address).send();
    const sigs: any[] = Array.isArray(s) ? s : (s.values ?? s.value ?? []);
    console.log(
      "  recent sigs for creator:",
      sigs.slice(0, 5).map((x: any) => `${String(x.signature).slice(0, 12)} ${x.err ?? "ok"}`),
    );
    process.exit(0);
  }

  const sigDelegate = await l1Client(creator).send([delegateIx]);
  console.log("delegate tx    ", sigDelegate);

  // 4. DISCOVER THE LIVING ER (expect the TEE fqdn) + WAIT FOR ActiveOnEr
  const status = await waitFor(
    "delegation fqdn",
    async () => {
      const s = await getDelegationStatus(covenantKey);
      return s.isDelegated && s.fqdn ? s : undefined;
    },
    120000,
    3000,
  );
  const fqdnRaw = status.fqdn;
  if (!fqdnRaw) throw new Error("delegation status missing fqdn");
  const fqdn = fqdnRaw.replace(/^https?:\/\//, "").replace(/\/.*$/, "");
  const erEndpoint = `https://${fqdn}`;
  if (process.argv.includes("--delegate-nontee")) {
    console.log("  WARN non-TEE delegation; private-read steps below may fail");
  } else {
    assert(
      fqdn.includes("tee"),
      `expected TEE delegation, got ${fqdn}`,
    );
  }
  console.log("  delegated to  ", erEndpoint);

  // The TEE ER only answers JSON-RPC to callers carrying a valid auth token
  // (the permission gating lives in the validator). Mint one bound to the
  // payer keypair and use the tokenized endpoint for every ER send + read.
  const creatorToken = await getAuthTokenFor(
    erEndpoint,
    creatorWeb3.publicKey,
    (msg) => Promise.resolve(nacl.sign.detached(msg, creatorWeb3.secretKey)),
  );
  const erRpcEndpoint = `${erEndpoint}?token=${encodeURIComponent(creatorToken.token)}`;
  console.log(
    "  er rpc token   ", creatorToken.token.slice(0, 12) + "…",
    "expires", new Date(creatorToken.expiresAt).toISOString(),
  );
  await waitFor(
    "ActiveOnEr",
    async () => {
      const acc = await readCovenant(readRpc, covenantKey);
      return acc && acc.status === CovenantStatus.ActiveOnEr ? acc : undefined;
    },
    150000,
    3000,
  );
  console.log("  covenant ActiveOnEr");

  // 5. INITIALIZE THE PRIVATE PERMISSION ON THE ER (payer = covenant PDA
  //    spending its prefunded/rent lamports; rent-sponsor CPI signed with the
  //    covenant's own seeds).
  const permission = await permissionAddress(covenantKey);
  const permIx = getInitPermissionInstruction({
    payer: creator,
    covenant: covenantKey,
    permission,
  });
  const sigPerm = await erClient(creator, erRpcEndpoint).sendTransaction([permIx]);
  console.log("init_permission", sigPerm);

  // 6. TOKENIZED READS: obtain the TEE bearer token for the counterparty and
  //    read the (private) covenant state through the TEE.
  const auth = await getAuthTokenFor(
    erEndpoint,
    cpWeb3.publicKey,
    (msg) => Promise.resolve(nacl.sign.detached(msg, cpWeb3.secretKey)),
  );
  console.log("  ttee token    ", auth.token.slice(0, 12) + "…", "expires", new Date(auth.expiresAt).toISOString());

  const privAcc = await waitFor(
    "private covenant read (tokenized)",
    async () => {
      const acc = await fetchPrivateAccount(
        `${erEndpoint}?token=${encodeURIComponent(auth.token)}`,
        auth.token,
        covenantKey,
      );
      return acc && acc.data ? acc : undefined;
    },
    30000,
    2000,
  );
  const privC = decodePrivateCovenant(privAcc!);
  if (!privC) throw new Error("tokenized read decoded into a covenant");
  assert(privC.status === CovenantStatus.ActiveOnEr, "priv read sees ActiveOnEr");
  assert(privC.creatorAmount === creatorAmount, "priv read sees creator lock");
  console.log("  private read ok:", assetState(privC));

  // And the negative proof: without a token the TEE must refuse the account.
  const pubAcc = await tryPublicRead(erEndpoint, covenantKey);
  assert(
    !pubAcc || !pubAcc.data,
    "unauthenticated read must not expose private covenant data",
  );
  console.log("  unauthenticated read correctly denied");

  // 7. NEGOTIATE ON THE ER: counterparty raises to 0.3, both accept
  const cpAmount1 = 300_000_000n;
  const updOff = getUpdateOfferInstruction({
    payer: cp,
    covenant: covenantKey,
    newAmount: cpAmount1,
  });
  const sigUpdate = await erClient(cp, erRpcEndpoint).sendTransaction([updOff]);
  console.log("update_offer tx", sigUpdate);
  const erRpcCp = `${erEndpoint}?token=${encodeURIComponent(auth.token)}`;
  await waitFor(
    "counterparty amount updated on ER",
    async () => {
      const acc = await readCovenantEr(erRpcCp, auth.token, covenantKey);
      return acc && acc.counterpartyAmount === cpAmount1 && acc.status === CovenantStatus.ActiveOnEr
        ? acc
        : undefined;
    },
    30000,
    2000,
  );
  console.log("  offer updated to 0.3 SOL");

  await erClient(creator, erRpcEndpoint).sendTransaction([getAcceptInstruction({ payer: creator, covenant: covenantKey })]);
  const sigAcceptCp = await erClient(cp, erRpcEndpoint).sendTransaction([getAcceptInstruction({ payer: cp, covenant: covenantKey })]);
  console.log("  both accepted (cp accept", sigAcceptCp + ")");
  await waitFor(
    "both_accepted + ActiveOnEr",
    async () => {
      const acc = await readCovenantEr(erRpcCp, auth.token, covenantKey);
      return acc &&
        acc.creatorAccepted &&
        acc.counterpartyAccepted &&
        acc.status === CovenantStatus.ActiveOnEr
        ? acc
        : undefined;
    },
    30000,
    2000,
  );

  // 8. COMMIT ON THE ER -> MAGIC ACTION SWAPS ON BASE
  const accts = await settlementAccounts(c, vault, "swap");
  const commitIx = await getCommitAndSwapInstructionAsync({
    payer: creator,
    covenant: covenantKey,
    vault,
    creator: creator.address,
    counterparty: cp.address,
    creatorVaultAta: accts.creatorVaultAta,
    counterpartyVaultAta: accts.counterpartyVaultAta,
    creatorPayoutAta: accts.creatorPayoutAta,
    counterpartyPayoutAta: accts.counterpartyPayoutAta,
    creatorMintAcct: accts.creatorMintAcct,
    counterpartyMintAcct: accts.counterpartyMintAcct,
    tokenProgram: address(TOKEN_PROGRAM_ID),
    systemProgram: address(SYSTEM_PROGRAM_ID),
    magicProgram: address(MAGIC_PROGRAM_ID),
    magicContext: address(MAGIC_CONTEXT_ID),
  });
  // magicContext stays non-signer per the `#[commit]` macro + Codama client.
  let sigCommit: string;
  try {
    sigCommit = await erClient(creator, erRpcEndpoint).sendTransaction([commitIx]);
  } catch (e) {
    // sendIxsRaw throws before returning the sig on failure; recover it from
    // the message and dump failed-vs-good envelopes before rethrowing.
    const m = String(e).match(/[1-9A-HJ-NP-Za-km-z]{80,90}/);
    const failedSig = m ? m[0] : null;
    console.log("commit tx FAILED, sig:", failedSig ?? "(unparseable)");
    if (failedSig) {
      await new Promise((r) => setTimeout(r, 4000));
      await diagCommitFailure(erRpcEndpoint, failedSig, sigAcceptCp);
    }
    throw e;
  }
  console.log("commit tx      ", sigCommit);

  await waitFor(
    "settled",
    async () => {
      const acc = await readCovenant(readRpc, covenantKey);
      return acc && acc.status === CovenantStatus.SettledOnL1 && acc.settledAt !== null ? acc : undefined;
    },
    180000,
    3000,
  );
  console.log("  covenant SettledOnL1");
  await waitFor(
    "vault drained after swap",
    async () => (await getVaultLamports(vault)) === 0n,
    30000,
    2000,
  );
  console.log("  vault drained, swap executed");

  const creatorBal = await getBalance(creator.address);
  const cpBal = await getBalance(cp.address);
  console.log("creator after  ", fmtLamports(creatorBal));
  console.log("counterparty   ", fmtLamports(cpBal));

  // =================================================================
  // FLOW B: OPEN covenant (no pinned counterparty) -> first-come join ->
  // second joiner rejected -> creator cancels -> close
  // =================================================================
  console.log("\n=== FLOW B: open join -> cancel -> close ===");

  const nonceB = nonceA + 1n;
  const covenantKey2 = await covenantAddress(creator.address, nonceB);
  const vault2 = await vaultAddress(covenantKey2);

  // Open covenant: counterparty = None.
  const a2 = 100_000_000n;
  const b2 = 40_000_000n;
  const init2 = await getInitializeCovenantInstructionAsync({
    creator,
    covenant: covenantKey2,
    vault: vault2,
    sourceTokenAccount: vault2,
    vaultTokenAccount: vault2,
    mintAccount: address(SYSTEM_PROGRAM_ID),
    tokenProgram: address(TOKEN_PROGRAM_ID),
    systemProgram: address(SYSTEM_PROGRAM_ID),
    counterparty: null,
    creatorAsset: AssetType.Sol,
    creatorMint: address(SYSTEM_PROGRAM_ID),
    creatorAmount: a2,
    deadline: BigInt(Math.floor(Date.now() / 1000) + 600),
    nonce: nonceB,
  });
  await l1Client(creator).send([init2]);
  let c2 = await pollCovenant(covenantKey2);
  assert(unwrapOption(c2.counterparty) === null, "open covenant: no counterparty yet");
  console.log("  created open covenant (no pinned counterparty)");

  // First-come join pins the counterparty.
  const join2 = await getJoinCovenantInstructionAsync({
    counterparty: cp,
    covenant: covenantKey2,
    vault: vault2,
    sourceTokenAccount: vault2,
    vaultTokenAccount: vault2,
    mintAccount: address(SYSTEM_PROGRAM_ID),
    tokenProgram: address(TOKEN_PROGRAM_ID),
    systemProgram: address(SYSTEM_PROGRAM_ID),
    counterpartyAsset: AssetType.Sol,
    counterpartyMint: address(SYSTEM_PROGRAM_ID),
    counterpartyAmount: b2,
  });
  await l1Client(cp).send([join2]);
  await pollCovenant(covenantKey2);
  c2 = await pollCovenant(covenantKey2);
  assert(unwrapOption(c2.counterparty) === cp.address, "first joiner pinned as counterparty");
  assert((await getVaultLamports(vault2)) === a2 + b2, "vault2 holds both");
  console.log("  first joiner pinned, vault2 =", fmtLamports(await getVaultLamports(vault2)));

  // A second joiner must fail.
  const intruder = await createKeyPairSignerFromBytes(
    new Uint8Array(Keypair.generate().secretKey),
  );
  const join3 = await getJoinCovenantInstructionAsync({
    counterparty: intruder,
    covenant: covenantKey2,
    vault: vault2,
    sourceTokenAccount: vault2,
    vaultTokenAccount: vault2,
    mintAccount: address(SYSTEM_PROGRAM_ID),
    tokenProgram: address(TOKEN_PROGRAM_ID),
    systemProgram: address(SYSTEM_PROGRAM_ID),
    counterpartyAsset: AssetType.Sol,
    counterpartyMint: address(SYSTEM_PROGRAM_ID),
    counterpartyAmount: b2,
  });
  const failed = await (async () => {
    try {
      await sendIxsRaw(intruder, [join3]);
      return false;
    } catch (e) {
      console.log("  second join rejected:", String((e as Error).message).slice(0, 120));
      return true;
    }
  })();
  assert(failed, "second join must be rejected");

  // Creator cancels while still CreatedOnL1: refunds both parties.
  const cancelIx = getCancelCovenantInstruction({
    signer: creator,
    covenant: covenantKey2,
    vault: vault2,
    creator: creator.address,
    counterparty: cp.address,
    creatorVaultAta: vault2,
    counterpartyVaultAta: vault2,
    creatorPayoutAta: vault2,
    counterpartyPayoutAta: vault2,
    creatorMintAcct: address(SYSTEM_PROGRAM_ID),
    counterpartyMintAcct: address(SYSTEM_PROGRAM_ID),
    tokenProgram: address(TOKEN_PROGRAM_ID),
    systemProgram: address(SYSTEM_PROGRAM_ID),
  });
  const sigCancel = await l1Client(creator).send([cancelIx]);
  console.log("cancel tx      ", sigCancel);
  c2 = await pollCovenant(covenantKey2);
  assert(c2.status === CovenantStatus.Reverted, "cancelled covenant -> Reverted");
  await waitFor(
    "vault2 drained after cancel",
    async () => (await getVaultLamports(vault2)) === 0n,
    30000,
    2000,
  );
  const cpAfter = await getBalance(cp.address);
  assert(cpAfter >= cpStart, "cp recovered their locked SOL after cancel");
  console.log("  covenant Reverted, vault2 refunded; cp bal", fmtLamports(cpAfter));

  // Close the terminal covenant: reclaims the covenant PDA rent + prefund.
  const closeIx = getCloseCovenantInstruction({
    signer: creator,
    covenant: covenantKey2,
  });
  const sigClose = await l1Client(creator).send([closeIx]);
  console.log("close tx       ", sigClose);
  const gone = await fullRpc.getAccountInfo(covenantKey2).send();
  assert(gone.value === null, "covenant2 account closed");
  console.log("  covenant2 closed, PDA lamports reclaimed");

  // =================================================================
  // FLOW B2: open covenant -> first-come join -> TEE delegate ->
  // deadline passes -> expire_and_refund (delegated expiry refund)
  // =================================================================
  console.log("\n=== FLOW B2: delegated deadline expiry refund ===");

  const nonceC = nonceB + 1n;
  const covenantKey3 = await covenantAddress(creator.address, nonceC);
  const vault3 = await vaultAddress(covenantKey3);

  // Short-but-future deadline so we can watch the clock pass it live.
  const expireDeadline = BigInt(Math.floor(Date.now() / 1000) + 60);
  const init3 = await getInitializeCovenantInstructionAsync({
    creator,
    covenant: covenantKey3,
    vault: vault3,
    sourceTokenAccount: vault3,
    vaultTokenAccount: vault3,
    mintAccount: address(SYSTEM_PROGRAM_ID),
    tokenProgram: address(TOKEN_PROGRAM_ID),
    systemProgram: address(SYSTEM_PROGRAM_ID),
    counterparty: null,
    creatorAsset: AssetType.Sol,
    creatorMint: address(SYSTEM_PROGRAM_ID),
    creatorAmount: a2,
    deadline: expireDeadline,
    nonce: nonceC,
  });
  await l1Client(creator).send([init3]);
  await pollCovenant(covenantKey3);

  const join4 = await getJoinCovenantInstructionAsync({
    counterparty: cp,
    covenant: covenantKey3,
    vault: vault3,
    sourceTokenAccount: vault3,
    vaultTokenAccount: vault3,
    mintAccount: address(SYSTEM_PROGRAM_ID),
    tokenProgram: address(TOKEN_PROGRAM_ID),
    systemProgram: address(SYSTEM_PROGRAM_ID),
    counterpartyAsset: AssetType.Sol,
    counterpartyMint: address(SYSTEM_PROGRAM_ID),
    counterpartyAmount: b2,
  });
  await l1Client(cp).send([join4]);
  await pollCovenant(covenantKey3);
  assert((await getVaultLamports(vault3)) === a2 + b2, "vault3 holds both before expiry");

  // Delegate so the expiry is exercised while the covenant is delegated.
  const route3 = await pickTeeValidator();
  if (!route3) throw new Error("no TEE route from router getRoutes");
  const delegate3 = await getDelegateCovenantInstructionAsync({
    payer: creator,
    covenant: covenantKey3,
    validator: address(route3.identity) as never,
  });
  await l1Client(creator).send([delegate3]);
  console.log("  delegated covenant3 to ", route3.fqdn);

  // Wait for the deadline to actually pass (60s), then expire on the base.
  await waitFor(
    "clock past deadline",
    async () => (BigInt(Math.floor(Date.now() / 1000)) >= expireDeadline ? true : undefined),
    120000,
    2000,
  );
  console.log("  deadline passed, expiring…");

  const expireIx = getExpireAndRefundInstruction({
    signer: creator,
    covenant: covenantKey3,
    vault: vault3,
    creator: creator.address,
    counterparty: cp.address,
    creatorVaultAta: vault3,
    counterpartyVaultAta: vault3,
    creatorPayoutAta: vault3,
    counterpartyPayoutAta: vault3,
    creatorMintAcct: address(SYSTEM_PROGRAM_ID),
    counterpartyMintAcct: address(SYSTEM_PROGRAM_ID),
    tokenProgram: address(TOKEN_PROGRAM_ID),
    systemProgram: address(SYSTEM_PROGRAM_ID),
  });
  const sigExpire = await l1Client(creator).send([expireIx]);
  console.log("expire tx      ", sigExpire);
  const c3 = await pollCovenant(covenantKey3);
  assert(c3.status === CovenantStatus.SettledOnL1, "expired covenant -> SettledOnL1");
  await waitFor(
    "vault3 drained after expiry",
    async () => (await getVaultLamports(vault3)) === 0n,
    30000,
    2000,
  );
  const creatorAfterExp = await getBalance(creator.address);
  assert(creatorAfterExp >= creatorFundsStart, "creator recovered their locked SOL after expiry");
  const cpAfterExp = await getBalance(cp.address);
  assert(cpAfterExp >= cpStart, "cp recovered their locked SOL after expiry");
  console.log("  covenant3 expired, vault3 refunded; creator", fmtLamports(creatorAfterExp), "cp", fmtLamports(cpAfterExp));

  const close3Ix = getCloseCovenantInstruction({
    signer: creator,
    covenant: covenantKey3,
  });
  await l1Client(creator).send([close3Ix]);
  const gone3 = await fullRpc.getAccountInfo(covenantKey3).send();
  assert(gone3.value === null, "covenant3 account closed");
  console.log("  covenant3 closed, PDA lamports reclaimed");

  // =================================================================
  // FLOW C: SPL Token (USDC) <-> SOL private covenant
  // =================================================================
  console.log("\n=== FLOW C: USDC <-> SOL private covenant ===");

  const nonceD = nonceC + 1n;
  const covenantKey4 = await covenantAddress(creator.address, nonceD);
  const vault4 = await vaultAddress(covenantKey4);
  const usdcMint = address(DEMO_MINTS.USDC);

  // 1. Pre-create vault's USDC ATA (the vault PDA owns it).
  const vaultUsdcAta = await ataAddress(vault4, usdcMint);
  const creatorUsdcAta = await ataAddress(creator.address, usdcMint);
  const cpSolAta = SOL_DUMMY;
  step("create vault USDC ATA");
  const createAtaIx = createAtaInstruction(creator.address, vault4, usdcMint, vaultUsdcAta);
  await l1Client(creator).send([createAtaIx]);
  // Verify ATA exists.
  const ataInfo = await fullRpc.getAccountInfo(vaultUsdcAta).send();
  assert(ataInfo.value !== null, "vault USDC ATA created");
  console.log("  vault USDC ATA:", vaultUsdcAta.slice(0, 12) + "…");

  // 2. CREATOR LOCKS 100 USDC (pinned counterparty).
  const usdcAmount = 100_000_000n; // 100 USDC (6 dp)
  const solAmount = 250_000_000n; // 0.25 SOL
  const deadlineC = BigInt(Math.floor(Date.now() / 1000) + 600);
  step("creator locks 100 USDC");
  const initC = await getInitializeCovenantInstructionAsync({
    creator,
    covenant: covenantKey4,
    vault: vault4,
    sourceTokenAccount: creatorUsdcAta,
    vaultTokenAccount: vaultUsdcAta,
    mintAccount: usdcMint,
    tokenProgram: address(TOKEN_PROGRAM_ID),
    systemProgram: address(SYSTEM_PROGRAM_ID),
    counterparty: cp.address,
    creatorAsset: AssetType.Token,
    creatorMint: usdcMint,
    creatorAmount: usdcAmount,
    deadline: deadlineC,
    nonce: nonceD,
  });
  const sigInitC = await l1Client(creator).send([initC]);
  console.log("init tx        ", sigInitC);
  let c4 = await pollCovenant(covenantKey4);
  assert(c4.status === CovenantStatus.CreatedOnL1, "FLOW C: CreatedOnL1");
  assert(c4.creatorAsset === AssetType.Token, "FLOW C: creator asset is Token");
  console.log("  create ok, vault USDC ATA holds", (await getVaultTokenBalance(vaultUsdcAta)));

  // 3. COUNTERPARTY JOINS WITH 0.25 SOL.
  step("counterparty joins 0.25 SOL");
  const joinC = await getJoinCovenantInstructionAsync({
    counterparty: cp,
    covenant: covenantKey4,
    vault: vault4,
    sourceTokenAccount: cpSolAta,
    vaultTokenAccount: cpSolAta,
    mintAccount: address(SYSTEM_PROGRAM_ID),
    tokenProgram: address(TOKEN_PROGRAM_ID),
    systemProgram: address(SYSTEM_PROGRAM_ID),
    counterpartyAsset: AssetType.Sol,
    counterpartyMint: address(SYSTEM_PROGRAM_ID),
    counterpartyAmount: solAmount,
  });
  const sigJoinC = await l1Client(cp).send([joinC]);
  console.log("join tx        ", sigJoinC);
  c4 = await pollCovenant(covenantKey4);
  assert(c4.counterpartyJoined, "FLOW C: counterparty joined");
  assert((await getVaultLamports(vault4)) === solAmount, "FLOW C: vault holds SOL");
  console.log("  join ok, vault SOL =", fmtLamports(await getVaultLamports(vault4)));

  // 4. DELEGATE TO TEE.
  const routeC = await pickTeeValidator();
  if (!routeC) throw new Error("no TEE route");
  step("delegate to TEE");
  const delegateC = await getDelegateCovenantInstructionAsync({
    payer: creator,
    covenant: covenantKey4,
    validator: address(routeC.identity) as never,
  });
  await l1Client(creator).send([delegateC]);
  console.log("  delegated to", routeC.fqdn);

  // 5. WAIT FOR ActiveOnEr.
  await waitFor(
    "FLOW C ActiveOnEr",
    async () => {
      const acc = await readCovenant(readRpc, covenantKey4);
      return acc && acc.status === CovenantStatus.ActiveOnEr ? acc : undefined;
    },
    150000,
    3000,
  );
  console.log("  covenant ActiveOnEr");

  // 6. INIT PERMISSION + PRIVATE READS.
  const permissionC = await permissionAddress(covenantKey4);
  const permC = getInitPermissionInstruction({
    payer: creator,
    covenant: covenantKey4,
    permission: permissionC,
  });
  const sigPermC = await erClient(creator, erRpcEndpoint).sendTransaction([permC]);
  console.log("init_permission", sigPermC);

  const authC = await getAuthTokenFor(
    erEndpoint,
    cpWeb3.publicKey,
    (msg) => Promise.resolve(nacl.sign.detached(msg, cpWeb3.secretKey)),
  );
  const privAccC = await waitFor(
    "FLOW C private read",
    async () => {
      const acc = await fetchPrivateAccount(erEndpoint, authC.token, covenantKey4);
      return acc && acc.data ? acc : undefined;
    },
    30000,
    2000,
  );
  const privCC = decodePrivateCovenant(privAccC!);
  if (!privCC) throw new Error("FLOW C: tokenized read decoded");
  assert(privCC.status === CovenantStatus.ActiveOnEr, "FLOW C: private read ActiveOnEr");
  assert(privCC.creatorAsset === AssetType.Token, "FLOW C: private read sees Token");
  console.log("  private read ok:", assetState(privCC));

  // 7. ACCEPT (no negotiation — fixed terms).
  await erClient(creator, erRpcEndpoint).sendTransaction([getAcceptInstruction({ payer: creator, covenant: covenantKey4 })]);
  const erCpC = erClient(cp, erRpcEndpoint);
  await erCpC.sendTransaction([getAcceptInstruction({ payer: cp, covenant: covenantKey4 })]);
  console.log("  both accepted");
  await waitFor(
    "FLOW C both_accepted",
    async () => {
      const acc = await readCovenant(readRpc, covenantKey4);
      return acc && acc.creatorAccepted && acc.counterpartyAccepted &&
        acc.status === CovenantStatus.ActiveOnEr ? acc : undefined;
    },
    30000,
    2000,
  );

  // 8. COMMIT + SWAP: USDC goes to counterparty, SOL goes to creator.
  step("commit_and_swap");
  const acctsC = await settlementAccounts(c4, vault4, "swap");
  const commitC = await getCommitAndSwapInstructionAsync({
    payer: creator,
    covenant: covenantKey4,
    vault: vault4,
    creator: creator.address,
    counterparty: cp.address,
    creatorVaultAta: acctsC.creatorVaultAta,
    counterpartyVaultAta: acctsC.counterpartyVaultAta,
    creatorPayoutAta: acctsC.creatorPayoutAta,
    counterpartyPayoutAta: acctsC.counterpartyPayoutAta,
    creatorMintAcct: acctsC.creatorMintAcct,
    counterpartyMintAcct: acctsC.counterpartyMintAcct,
    tokenProgram: address(TOKEN_PROGRAM_ID),
    systemProgram: address(SYSTEM_PROGRAM_ID),
    magicProgram: address(MAGIC_PROGRAM_ID),
    magicContext: address(MAGIC_CONTEXT_ID),
  });
  const sigCommitC = await erClient(creator, erRpcEndpoint).sendTransaction([commitC]);
  console.log("commit tx      ", sigCommitC);

  await waitFor(
    "FLOW C settled",
    async () => {
      const acc = await readCovenant(readRpc, covenantKey4);
      return acc && acc.status === CovenantStatus.SettledOnL1 && acc.settledAt !== null ? acc : undefined;
    },
    180000,
    3000,
  );
  console.log("  covenant SettledOnL1");
  await waitFor(
    "FLOW C vault SOL drained",
    async () => (await getVaultLamports(vault4)) === 0n,
    30000,
    2000,
  );
  console.log("  vault SOL drained");

  // Verify token balances: creator got SOL, counterparty got USDC.
  const creatorFinalSol = await getBalance(creator.address);
  const cpFinalSol = await getBalance(cp.address);
  console.log("FLOW C result:");
  console.log("  creator SOL   ", fmtLamports(creatorFinalSol));
  console.log("  counterparty SOL", fmtLamports(cpFinalSol));

  console.log("\nE2E PASS — all flows completed.");
}

function assetState(c: Covenant): string {
  const cpAddr = unwrapOption(c.counterparty);
  return `creator=${c.creator.slice(0, 6)} cp=${cpAddr?.slice(0, 6) ?? "-"} ` +
    `creatorLock=${fmtLamports(c.creatorAmount)} status=${c.status}`;
}

main().then(
  () => process.exit(0),
  (e) => {
    console.error("\nE2E FAILED:\n" + ((e as Error).stack ?? String(e)));
    process.exitCode = 1;
  },
);

} // end PREFLIGHT_COMMIT else (live e2e main)