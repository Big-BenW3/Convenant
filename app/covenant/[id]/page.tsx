"use client";

import { useEffect, useMemo, useState } from "react";
import { useParams } from "next/navigation";
import Link from "next/link";
import { address, unwrapOption } from "@solana/kit";
import {
  AssetType,
  CovenantStatus,
  getJoinCovenantInstructionAsync,
  getDelegateCovenantInstructionAsync,
  getUpdateOfferInstruction,
  getAcceptInstruction,
  getCommitAndSwapInstructionAsync,
  getExpireAndRefundInstructionAsync,
  type Covenant,
} from "../../generated/convenant";
import { useWallet } from "../../lib/wallet/context";
import { useSendTransaction } from "../../lib/hooks/use-send-transaction";
import { useSendErTransaction } from "../../lib/hooks/use-send-er-transaction";
import {
  readCovenant,
  createReadRpc,
  vaultAddress,
  settlementAccounts,
  ataAddress,
  createAtaInstruction,
  statusMeta,
  formatAmount,
  assetLabel,
  SOL_DUMMY,
  SYSTEM_PROGRAM_ID,
  TOKEN_PROGRAM_ID,
  DEMO_MINTS,
  rememberCovenant,
} from "../../lib/covenant";
import { getClusterUrl } from "../../lib/solana-client";
import { getDelegationStatus, pickTeeValidator, MAGIC_PROGRAM_ID, MAGIC_CONTEXT_ID } from "../../lib/magicblock";
import { ellipsify } from "../../lib/explorer";
import { GridBackground } from "../../components/grid-background";
import { WalletButton } from "../../components/wallet-button";
import { ThemeToggle } from "../../components/theme-toggle";

type OpStatus = "idle" | "sending" | "done" | "error";

export default function CovenantDetailPage() {
  const params = useParams<{ id: string }>();
  const id = params?.id ?? "";

  const { signer, status: walletStatus } = useWallet();
  const { send, } = useSendTransaction();

  const [covenant, setCovenant] = useState<Covenant | null>(null);
  const [notFound, setNotFound] = useState(false);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [op, setOp] = useState<OpStatus>("idle");
  const [opName, setOpName] = useState("");
  const [nowMs, setNowMs] = useState(0);

  // Join form (counterparty only).
  const [cpAsset, setCpAsset] = useState<"SOL" | "USDC" | "NFT">("SOL");
  const [cpAmount, setCpAmount] = useState("");
  const [cpAmountBig, setCpAmountBig] = useState("0");
  // Update-offer form.
  const [newAmount, setNewAmount] = useState("");

  const readRpc = useMemo(() => createReadRpc(getClusterUrl("devnet")), []);

  const covenantKey = useMemo(() => (id ? address(id) : undefined), [id]);
  const me = useMemo(
    () => (signer ? signer.address : undefined),
    [signer]
  );

  const [erHost, setErHost] = useState<string | undefined>(undefined);
  const { send: sendEr } = useSendErTransaction(erHost);

  // Discover the live ER endpoint once the covenant is delegated.
  useEffect(() => {
    if (!covenant || covenant.status !== CovenantStatus.ActiveOnEr) return;
    let alive = true;
    const discover = async () => {
      try {
        const status = await getDelegationStatus(covenantKey!);
        if (alive && status.isDelegated && status.fqdn) {
          setErHost(`https://${status.fqdn}`);
        }
      } catch {
        /* router may not know the account yet — keep polling */
      }
    };
    void discover();
    const t = setInterval(discover, 3000);
    return () => {
      alive = false;
      clearInterval(t);
    };
  }, [covenant, covenantKey]);

  useEffect(() => {
    rememberCovenant(id);
  }, [id]);

  // Poll covenant state (fast while on the ER).
  useEffect(() => {
    if (!covenantKey) return;
    let alive = true;
    const poll = async () => {
      setNowMs(Date.now());
      const acc = await readCovenant(readRpc, covenantKey);
      if (alive) {
        if (acc) setCovenant(acc);
        else setNotFound(true);
        setLoading(false);
      }
    };
    void poll();
    const t = setInterval(() => void poll(), 2000);
    return () => {
      alive = false;
      clearInterval(t);
    };
  }, [readRpc, covenantKey]);

  const meta = useMemo(
    () => (covenant ? statusMeta(covenant.status) : undefined),
    [covenant]
  );

  const isCreator = !!me && covenant?.creator === me;
  const cpAddr = useMemo(
    () => (covenant ? unwrapOption(covenant.counterparty) : undefined),
    [covenant]
  );
  const isCounterparty = !!me && cpAddr === me;
  const isParty = isCreator || isCounterparty;
  const deadlinePassed =
    covenant != null && Number(covenant.deadline) * 1000 < nowMs;
  const bothAccepted =
    covenant != null &&
    covenant.creatorAccepted &&
    covenant.counterpartyAccepted;
  const canSettle = bothAccepted;

  if (loading) {
    return <Shell>Loading covenant…</Shell>;
  }

  if (notFound || !covenant || !covenantKey) {
    return (
      <Shell>
        <p className="text-doto mb-2 text-xs uppercase tracking-[0.3em] text-muted">
          Not found
        </p>
        <p className="text-sm text-muted">
          No covenant account at {ellipsify(id, 8)} on devnet.
        </p>
        <Link
          href="/dashboard"
          className="mt-6 inline-block border border-border px-5 py-3 text-xs uppercase tracking-[0.2em] transition hover:border-foreground"
        >
          Back to dashboard
        </Link>
      </Shell>
    );
  }

  const showJoin = isCounterparty && !covenant.counterpartyJoined;
  const showDelegate = covenant.counterpartyJoined &&
    (covenant.status === CovenantStatus.CreatedOnL1);
  const showNegotiate = covenant.status === CovenantStatus.ActiveOnEr;
  const showExpire = deadlinePassed &&
    (covenant.status === CovenantStatus.CreatedOnL1 ||
      covenant.status === CovenantStatus.Delegating ||
      covenant.status === CovenantStatus.ActiveOnEr);

  const run = async (name: string, fn: (s: NonNullable<typeof signer>) => Promise<string>) => {
    setError(null);
    setOpName(name);
    setOp("sending");
    try {
      if (!signer) throw new Error("Connect a wallet first.");
      await fn(signer);
      setOp("done");
      await new Promise((r) => setTimeout(r, 1200));
      setOp("idle");
    } catch (err) {
      setOp("error");
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  const doJoin = () =>
    run("join", async (s) => {
      const vault = await vaultAddress(covenantKey);
      const mySol = cpAsset === "SOL";
      const myMint = mySol
        ? address(SYSTEM_PROGRAM_ID)
        : address(cpAsset === "USDC" ? DEMO_MINTS.USDC : DEMO_MINTS.NFT);
      const amount = Number(cpAmountBig);
      if (!amount || amount <= 0) throw new Error("Enter an amount greater than zero.");

      const myAta = mySol ? (SOL_DUMMY as never) : await ataAddress(s.address, myMint);
      const vaultAta = mySol ? (SOL_DUMMY as never) : await ataAddress(vault, myMint);
      const createAtaIx = mySol
        ? undefined
        : createAtaInstruction(s.address, vault, myMint, vaultAta as never);

      const ix = await getJoinCovenantInstructionAsync({
        counterparty: s,
        covenant: covenantKey,
        vault,
        sourceTokenAccount: myAta as never,
        vaultTokenAccount: vaultAta as never,
        mintAccount: mySol ? (SYSTEM_PROGRAM_ID as never) : (myMint as never),
        tokenProgram: address(TOKEN_PROGRAM_ID),
        systemProgram: address(SYSTEM_PROGRAM_ID),
        counterpartyAsset:
          cpAsset === "SOL" ? AssetType.Sol : cpAsset === "USDC" ? AssetType.Token : AssetType.Nft,
        counterpartyMint: myMint as never,
        counterpartyAmount: BigInt(amount),
      });
      return send({ instructions: createAtaIx ? [createAtaIx, ix] : [ix] });
    });

  const doDelegate = () =>
    run("delegate", async (s) => {
      const route = await pickTeeValidator();
      if (!route) throw new Error("No TEE ER route available from the router (getRoutes).");
      const ix = await getDelegateCovenantInstructionAsync({
        payer: s,
        covenant: covenantKey,
        validator: address(route.identity) as never,
      });
      return send({ instructions: [ix] });
    });

  const doUpdateOffer = () =>
    run("update-offer", async (s) => {
      const amount = Number(newAmount);
      if (!amount || amount <= 0) throw new Error("Enter an amount greater than zero.");
      const ix = getUpdateOfferInstruction({
        payer: s,
        covenant: covenantKey,
        newAmount: BigInt(amount),
      });
      return sendEr({ instructions: [ix] });
    });

  const doAccept = () =>
    run("accept", async (s) => {
      const ix = getAcceptInstruction({ payer: s, covenant: covenantKey });
      return sendEr({ instructions: [ix] });
    });

  const doCommit = () =>
    run("commit-and-swap", async (s) => {
      const vault = await vaultAddress(covenantKey);
      const accts = await settlementAccounts(covenant, vault, "swap");
      const ix = await getCommitAndSwapInstructionAsync({
        payer: s,
        covenant: covenantKey,
        vault,
        creator: covenant.creator,
        counterparty: cpAddr!,
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
      return sendEr({ instructions: [ix] });
    });

  const doExpire = () =>
    run("expire", async (s) => {
      const vault = await vaultAddress(covenantKey);
      const accts = await settlementAccounts(covenant, vault, "refund");
      const ix = await getExpireAndRefundInstructionAsync({
        signer: s,
        covenant: covenantKey,
        vault,
        creator: covenant.creator,
        counterparty: cpAddr!,
        creatorVaultAta: accts.creatorVaultAta,
        counterpartyVaultAta: accts.counterpartyVaultAta,
        creatorPayoutAta: accts.creatorPayoutAta,
        counterpartyPayoutAta: accts.counterpartyPayoutAta,
        creatorMintAcct: accts.creatorMintAcct,
        counterpartyMintAcct: accts.counterpartyMintAcct,
        tokenProgram: address(TOKEN_PROGRAM_ID),
        systemProgram: address(SYSTEM_PROGRAM_ID),
      });
      return send({ instructions: [ix] });
    });

  return (
    <div className="relative min-h-screen bg-background text-foreground antialiased">
      <GridBackground />
      <div className="relative z-10 mx-auto max-w-3xl px-6">
        <header className="flex items-center justify-between border-b border-border-low py-4">
          <Link href="/" className="text-doto text-lg tracking-[0.3em]">
            CONVENANT
          </Link>
          <div className="flex items-center gap-3">
            <Link
              href="/dashboard"
              className="text-xs uppercase tracking-[0.2em] text-muted transition hover:text-foreground"
            >
              Panel
            </Link>
            <ThemeToggle />
            <WalletButton />
          </div>
        </header>

        <main className="py-10 md:py-14">
          <p className="text-doto mb-2 text-xs uppercase tracking-[0.3em] text-muted">
            Covenant
          </p>
          <h1 className="break-all font-mono text-lg">
            {ellipsify(covenantKey, 8)}
          </h1>
          {meta && (
            <span
              className={`mt-3 inline-block text-xs uppercase tracking-[0.2em] ${
                meta.kind === "active" || meta.kind === "ok"
                  ? "text-foreground"
                  : meta.kind === "err"
                    ? "text-destructive"
                    : "text-muted"
              }`}
            >
              {meta.label}
            </span>
          )}

          {/* Terms */}
          <section className="mt-8 border border-border-low p-5">
            <div className="grid grid-cols-[7rem_1fr] gap-y-3 text-sm">
              <span className="text-muted">Creator</span>
              <span className="truncate font-mono">
                {ellipsify(covenant.creator, 6)}
                {isCreator && <span className="ml-2 text-[10px] text-muted">YOU</span>}
              </span>
              <span className="text-muted">Counterparty</span>
              <span className="truncate font-mono">
                {cpAddr ? ellipsify(cpAddr, 6) : "—"}
                {isCounterparty && <span className="ml-2 text-[10px] text-muted">YOU</span>}{" "}
                {!covenant.counterpartyJoined && (
                  <span className="ml-1 text-[10px] text-muted">not joined</span>
                )}
              </span>
              <span className="text-muted">Creator locks</span>
              <span className="font-mono">
                {formatAmount(covenant.creatorAmount, covenant.creatorAsset)}{" "}
                {assetLabel(covenant.creatorAsset)}
              </span>
              <span className="text-muted">You receive →</span>
              <span className="font-mono">
                {covenant.counterpartyJoined
                  ? `${formatAmount(covenant.counterpartyAmount, covenant.counterpartyAsset)} ${assetLabel(covenant.counterpartyAsset)}`
                  : "pending join"}
              </span>
              <span className="text-muted">Deadline</span>
              <span className="font-mono tabular-nums">{formatDeadline(covenant.deadline)}</span>
              <span className="text-muted">Nonce</span>
              <span className="font-mono tabular-nums">{covenant.nonce.toString()}</span>
            </div>
            <div className="mt-4 flex gap-4 border-t border-border-low pt-3 text-[10px] uppercase tracking-[0.15em] text-muted">
              <span>creator accepts {covenant.creatorAccepted ? "YES" : "NO"}</span>
              <span>counterparty accepts {covenant.counterpartyAccepted ? "YES" : "NO"}</span>
              <span>deadline {deadlinePassed ? "PASSED" : "OPEN"}</span>
            </div>
          </section>

          {/* Join action */}
          {showJoin && (
            <section className="mt-6 border border-border-low p-5">
              <p className="mb-1 text-sm font-medium uppercase tracking-[0.12em]">
                Join — lock your side
              </p>
              <p className="mb-4 text-xs text-muted">
                You were invited. Choose the asset you lock and its amount.
              </p>
              <div className="grid grid-cols-3 gap-px bg-border-low">
                {(["SOL", "USDC", "NFT"] as const).map((a) => (
                  <button
                    key={a}
                    onClick={() => setCpAsset(a)}
                    className={`cursor-pointer p-3 text-center text-xs uppercase tracking-[0.15em] ${
                      cpAsset === a
                        ? "bg-primary text-primary-foreground"
                        : "bg-background text-muted hover:text-foreground"
                    }`}
                  >
                    {a}
                  </button>
                ))}
              </div>
              <input
                inputMode="decimal"
                value={cpAmount}
                onChange={(e) => {
                  setCpAmount(e.target.value);
                  const v = Number(e.target.value);
                  if (v > 0) setCpAmountBig(v.toString());
                }}
                placeholder="amount"
                className="mt-3 w-full border border-border-low bg-transparent p-3 font-mono text-lg outline-none placeholder:text-muted focus:border-foreground"
              />
              <button
                onClick={doJoin}
                disabled={walletStatus !== "connected" || op === "sending"}
                className="mt-4 w-full cursor-pointer bg-primary py-3 text-xs font-medium uppercase tracking-[0.2em] text-primary-foreground transition hover:bg-primary/85 disabled:opacity-40"
              >
                {op === "sending" ? "Joining…" : "Join & lock"}
              </button>
            </section>
          )}

          {/* Delegate */}
          {showDelegate && (
            <section className="mt-6 border border-border-low p-5">
              <p className="mb-1 text-sm font-medium uppercase tracking-[0.12em]">
                Move to private rollup
              </p>
              <p className="mb-4 text-xs text-muted">
                Both sides are locked. Delegate the covenant to a MagicBlock
                Ephemeral Rollup with a permission gate — terms become private
                and negotiation is sub-second.
              </p>
              <button
                onClick={doDelegate}
                disabled={walletStatus !== "connected" || op === "sending"}
                className="w-full cursor-pointer bg-primary py-3 text-xs font-medium uppercase tracking-[0.2em] text-primary-foreground transition hover:bg-primary/85 disabled:opacity-40"
              >
                {op === "sending" ? "Delegating…" : "Delegate to ER"}
              </button>
            </section>
          )}

          {/* Negotiation (ER) */}
          {showNegotiate && isParty && (
            <section className="mt-6 border border-border-low p-5">
              <p className="mb-1 text-sm font-medium uppercase tracking-[0.12em]">
                Negotiate on ER
              </p>
              <p className="mb-4 text-xs text-muted">
                Update your amount below, then each side accepts. Both accepted →
                commit swaps atomically on the base layer.
              </p>
              <div className="flex gap-2">
                <input
                  inputMode="decimal"
                  value={newAmount}
                  onChange={(e) => setNewAmount(e.target.value)}
                  placeholder={covenant.creatorAmount.toString()}
                  className="flex-1 border border-border-low bg-transparent p-3 font-mono text-sm outline-none placeholder:text-muted focus:border-foreground"
                />
                <button
                  onClick={doUpdateOffer}
                  disabled={op === "sending"}
                  className="cursor-pointer border border-border px-4 py-3 text-xs uppercase tracking-[0.2em] transition hover:border-foreground"
                >
                  Update
                </button>
              </div>
              <button
                onClick={doAccept}
                disabled={op === "sending"}
                className="mt-3 w-full cursor-pointer border border-border py-3 text-xs uppercase tracking-[0.2em] transition hover:border-foreground"
              >
                {isCreator
                  ? covenant.creatorAccepted
                    ? "Creator accepted ✓"
                    : "Accept terms as creator"
                  : covenant.counterpartyAccepted
                    ? "Counterparty accepted ✓"
                    : "Accept terms as counterparty"}
              </button>
              {canSettle && (
                <button
                  onClick={doCommit}
                  disabled={op === "sending"}
                  className="mt-3 w-full cursor-pointer bg-primary py-3 text-xs font-medium uppercase tracking-[0.2em] text-primary-foreground transition hover:bg-primary/85 disabled:opacity-40"
                >
                  {op === "sending" ? "Committing…" : "Both accepted — commit & swap"}
                </button>
              )}
            </section>
          )}

          {/* Not-party negotiation hint */}
          {showNegotiate && !isParty && (
            <section className="mt-6 border border-border-low p-5 text-center text-xs text-muted">
              This covenant is negotiating privately on an ephemeral rollup.
              Connect as {ellipsify(covenant.creator, 6)} or{" "}
              {cpAddr ? ellipsify(cpAddr, 6) : "—"} to participate.
            </section>
          )}

          {/* Expire */}
          {showExpire && (
            <section className="mt-6 border border-border-low p-5">
              <p className="mb-1 text-sm font-medium uppercase tracking-[0.12em]">
                Deadline passed
              </p>
              <p className="mb-4 text-xs text-muted">
                No agreement was sealed in time. Expire to return each locked
                asset to its owner.
              </p>
              <button
                onClick={doExpire}
                disabled={walletStatus !== "connected" || op === "sending"}
                className="w-full cursor-pointer border border-border py-3 text-xs uppercase tracking-[0.2em] transition hover:border-foreground disabled:opacity-40"
              >
                {op === "sending" ? "Expiring…" : "Expire & refund"}
              </button>
            </section>
          )}

          {/* Terminal states */}
          {covenant.status === CovenantStatus.SettledOnL1 && (
            <section className="mt-6 border border-border-low p-5 text-center text-xs uppercase tracking-[0.2em] text-muted">
              Settled — assets swapped on the base layer.
            </section>
          )}

          {op === "error" && error && (
            <p className="mt-5 border border-border-low p-3 font-mono text-xs">
              {error}
            </p>
          )}
          {op === "done" && (
            <p className="mt-5 border border-border-low p-3 font-mono text-xs text-foreground">
              {opName} submitted ✓
            </p>
          )}

          <div className="mt-8 flex gap-5 text-[11px] uppercase tracking-[0.15em] text-muted">
            <Link href="/dashboard" className="transition hover:text-foreground">
              ← Dashboard
            </Link>
            <a
              href={`https://solscan.io/account/${covenantKey}`}
              target="_blank"
              rel="noopener noreferrer"
              className="transition hover:text-foreground"
            >
              Solscan ↗
            </a>
          </div>
        </main>
      </div>
    </div>
  );
}

function formatDeadline(unix: bigint): string {
  const ms = Number(unix) * 1000;
  if (!Number.isFinite(ms)) return "—";
  return new Date(ms).toLocaleString(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

function Shell({ children }: { children: React.ReactNode }) {
  return (
    <div className="relative min-h-screen bg-background text-foreground antialiased">
      <GridBackground />
      <div className="relative z-10 mx-auto max-w-3xl px-6">
        <header className="flex items-center justify-between border-b border-border-low py-4">
          <Link href="/" className="text-doto text-lg tracking-[0.3em]">
            CONVENANT
          </Link>
          <div className="flex items-center gap-3">
            <Link
              href="/dashboard"
              className="text-xs uppercase tracking-[0.2em] text-muted transition hover:text-foreground"
            >
              Panel
            </Link>
            <ThemeToggle />
            <WalletButton />
          </div>
        </header>
        <main className="py-10 md:py-14">{children}</main>
      </div>
    </div>
  );
}