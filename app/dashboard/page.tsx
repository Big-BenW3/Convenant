"use client";

import { useEffect, useMemo, useRef, useState } from "react";
import Link from "next/link";
import { address, unwrapOption } from "@solana/kit";
import type { Covenant } from "../generated/convenant";
import { useWallet } from "../lib/wallet/context";
import { useCluster } from "../components/cluster-context";
import {
  readCovenant,
  createReadRpc,
  statusMeta,
  formatAmount,
  assetLabel,
  loadKnownCovenants,
  rememberCovenant,
  forgetCovenant,
} from "../lib/covenant";
import { getClusterUrl } from "../lib/solana-client";
import { ellipsify } from "../lib/explorer";
import { GridBackground } from "../components/grid-background";
import { WalletButton } from "../components/wallet-button";
import { ThemeToggle } from "../components/theme-toggle";

type Row = {
  key: string;
  covenantAddr: string;
  covenant?: Covenant;
  error?: string;
};

const KIND_STYLE: Record<
  NonNullable<ReturnType<typeof statusMeta>["kind"]>,
  string
> = {
  idle: "text-muted",
  active: "text-foreground",
  ok: "text-foreground",
  warn: "text-foreground",
  err: "text-destructive",
};

export default function DashboardPage() {
  const { signer, status } = useWallet();
  const { cluster, getExplorerUrl } = useCluster();
  // Memoized: building a client per render would hand `refreshFor` a new
  // identity every time and defeat the effect dependency below.
  const readRpc = useMemo(() => createReadRpc(getClusterUrl(cluster)), [cluster]);

  // Guards against out-of-order refreshes: two overlapping calls (mount effect
  // plus the refresh button) could otherwise resolve backwards and leave stale
  // rows on screen.
  const refreshSeq = useRef(0);

  const [rows, setRows] = useState<Row[]>([]);
  const [loading, setLoading] = useState(false);
  const [input, setInput] = useState("");

  const connected = status === "connected" && !!signer;

  const refreshFor = async (ids: string[]) => {
    const seq = ++refreshSeq.current;
    setLoading(true);
    try {
      const next: Row[] = await Promise.all(
        ids.map(async (id): Promise<Row> => {
          try {
            const covenantAddr = address(id);
            const maybe = await readCovenant(readRpc, covenantAddr);
            return maybe
              ? { key: id, covenantAddr: id, covenant: maybe }
              : { key: id, covenantAddr: id };
          } catch (err) {
            return {
              key: id,
              covenantAddr: id,
              error: err instanceof Error ? err.message : String(err),
            };
          }
        })
      );
      if (seq === refreshSeq.current) setRows(next);
    } finally {
      if (seq === refreshSeq.current) setLoading(false);
    }
  };

  // Load persisted covenants on mount / wallet connect.
  useEffect(() => {
    const ids = loadKnownCovenants();
    if (ids.length > 0) {
      const t = setTimeout(() => void refreshFor(ids), 0);
      return () => clearTimeout(t);
    }
  }, [connected, readRpc]);

  const addCovenant = () => {
    const id = input.trim();
    if (!id) return;
    rememberCovenant(id);
    setInput("");
    void refreshFor([...loadKnownCovenants(), id]);
  };

  const removeCovenant = (id: string) => {
    forgetCovenant(id);
    setRows((r) => r.filter((row) => row.key !== id));
  };

  return (
    <div className="relative min-h-screen bg-background text-foreground antialiased">
      <GridBackground />
      <div className="relative z-10 mx-auto max-w-4xl px-6">
        <header className="flex items-center justify-between border-b border-border-low py-4">
          <Link href="/" className="text-doto text-lg tracking-[0.3em]">
            CONVENANT
          </Link>
          <div className="flex items-center gap-3">
            <ThemeToggle />
            <WalletButton />
          </div>
        </header>

        <main className="py-12 md:py-16">
          <p className="text-doto mb-2 text-xs uppercase tracking-[0.3em] text-muted">
            Panel
          </p>
          <h1 className="text-3xl font-medium tracking-tight md:text-4xl">
            Covenant dashboard
          </h1>
          <p className="mt-3 max-w-xl text-sm leading-relaxed text-muted">
            Track every covenant you have created or joined. Create locks to your
            local panel automatically; add any covenant address to watch it.
          </p>

          {/* Add + refresh */}
          <div className="mt-8 flex flex-col gap-3 sm:flex-row">
            <input
              value={input}
              onChange={(e) => setInput(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && addCovenant()}
              placeholder="paste covenant address"
              className="flex-1 border border-border-low bg-transparent p-3 font-mono text-sm outline-none placeholder:text-muted focus:border-foreground"
            />
            <button
              onClick={addCovenant}
              className="cursor-pointer border border-border px-4 py-3 text-xs uppercase tracking-[0.2em] transition hover:border-foreground"
            >
              Add
            </button>
            <button
              onClick={() => void refreshFor(loadKnownCovenants())}
              disabled={loading}
              className="cursor-pointer bg-primary px-4 py-3 text-xs font-medium uppercase tracking-[0.2em] text-primary-foreground transition hover:bg-primary/85 disabled:opacity-40"
            >
              {loading ? "Refreshing…" : "Refresh"}
            </button>
          </div>

          {/* Empty state */}
          {rows.length === 0 && !loading && (
            <div className="mt-10 border border-border-low p-10 text-center">
              <p className="text-doto text-xs uppercase tracking-[0.3em] text-muted">
                No covenants
              </p>
              <p className="mt-3 text-sm text-muted">
                Create one first, or add an existing address above.
              </p>
              <Link
                href="/create"
                className="mt-6 inline-block border border-border px-5 py-3 text-xs uppercase tracking-[0.2em] transition hover:border-foreground"
              >
                Create covenant
              </Link>
            </div>
          )}

          {/* List */}
          <div className="mt-10 space-y-px bg-border-low">
            {rows.map((row) => {
              const c = row.covenant;
              const meta = c ? statusMeta(c.status) : undefined;
              return (
                <div key={row.key} className="bg-background p-5">
                  <div className="flex flex-wrap items-start justify-between gap-3">
                    <div className="flex items-center gap-3">
                      <span
                        className={`h-2 w-2 rounded-none ${
                          meta?.kind === "ok" || meta?.kind === "active"
                            ? "bg-foreground"
                            : meta?.kind === "err"
                              ? "bg-destructive"
                              : "bg-muted"
                        }`}
                      />
                      <div>
                        <p className="flex items-center gap-2 font-mono text-sm">
                          {ellipsify(row.covenantAddr, 6)}
                          {!c && (
                            <span className="text-[10px] uppercase tracking-[0.2em] text-muted">
                              no account
                            </span>
                          )}
                        </p>
                        {meta && c && (
                          <p className="mt-1 text-xs uppercase tracking-[0.2em] text-muted">
                            {meta.label}
                          </p>
                        )}
                      </div>
                    </div>

                    <div className="flex items-center gap-4">
                      <span
                        className={`font-mono text-xs uppercase tracking-[0.15em] ${
                          meta ? KIND_STYLE[meta.kind] : "text-muted"
                        }`}
                      >
                        {meta ? meta.label : row.error ? "ERROR" : "UNKNOWN"}
                      </span>
                      <button
                        onClick={() => removeCovenant(row.key)}
                        className="cursor-pointer text-muted transition hover:text-foreground"
                        aria-label="Remove"
                      >
                        ×
                      </button>
                    </div>
                  </div>

                  {c && (
                    <div className="mt-4 grid gap-4 border-t border-border-low pt-4 text-sm sm:grid-cols-2 lg:grid-cols-3">
                      <div>
                        <p className="text-[10px] uppercase tracking-[0.2em] text-muted">
                          Locked
                        </p>
                        <p className="mt-1 font-mono">
                          {formatAmount(c.creatorAmount, c.creatorAsset)}{" "}
                          <span className="text-muted">
                            {assetLabel(c.creatorAsset)}
                          </span>
                        </p>
                      </div>
                      <div>
                        <p className="text-[10px] uppercase tracking-[0.2em] text-muted">
                          Counterparty
                        </p>
                        <p className="mt-1 font-mono">
                          {c.counterpartyJoined
                            ? ellipsify(unwrapOption(c.counterparty) ?? "not joined", 6)
                            : "not joined"}
                        </p>
                      </div>
                      <div>
                        <p className="text-[10px] uppercase tracking-[0.2em] text-muted">
                          Deadline
                        </p>
                        <p className="mt-1 font-mono tabular-nums">
                          {formatDeadline(c.deadline)}
                        </p>
                      </div>
                    </div>
                  )}

                  {row.error && (
                    <p className="mt-3 font-mono text-xs text-destructive">
                      {row.error}
                    </p>
                  )}

                  <div className="mt-4 flex gap-4 text-[11px] uppercase tracking-[0.15em] text-muted">
                    <a
                      href={getExplorerUrl(`/account/${row.covenantAddr}`)}
                      target="_blank"
                      rel="noopener noreferrer"
                      className="transition hover:text-foreground"
                    >
                      Explorer ↗
                    </a>
                    <button
                      onClick={() => void navigator.clipboard.writeText(row.covenantAddr)}
                      className="cursor-pointer transition hover:text-foreground"
                    >
                      Copy
                    </button>
                  </div>
                </div>
              );
            })}
          </div>
        </main>
      </div>
    </div>
  );
}

function formatDeadline(unix: bigint): string {
  const ms = Number(unix) * 1000;
  if (!Number.isFinite(ms)) return "—";
  return new Date(ms).toLocaleDateString(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}