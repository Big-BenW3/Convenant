import Link from "next/link";
import { WalletButton } from "./components/wallet-button";
import { ThemeToggle } from "./components/theme-toggle";
import { GridBackground } from "./components/grid-background";

const PROTECTED_PAIRS = [
  { a: "SOL", b: "USDC", note: "Lock cash on both sides, settle timed" },
  { a: "SOL", b: "NFT", note: "Swap a floor piece for liquidity" },
  { a: "USDC", b: "USDC", note: "Timelocked conditional payment" },
  { a: "USDC", b: "NFT", note: "Buy now, deliver on expiry" },
  { a: "NFT", b: "NFT", note: "Peer-to-peer collection trade" },
  { a: "SOL", b: "SOL", note: "Deferred atomic settlement" },
];

const STEPS = [
  {
    n: "01",
    title: "Compose",
    body: "You pick one asset to offer and one to demand. Both escrow into the program before any negotiation starts.",
  },
  {
    n: "02",
    title: "Delegate",
    body: "The covenant moves onto a MagicBlock Ephemeral Rollup. Terms are private there \u2014 invisible to the public chain.",
  },
  {
    n: "03",
    title: "Agree",
    body: "Counterparty checks the terms privately. Both accept, and the rollup atomically settles both sides on the base layer in one transaction.",
  },
  {
    n: "04",
    title: "Fail-closed",
    body: "If the deadline passes without agreement, every locked asset returns to its owner. No party can lose into the void.",
  },
];

const USES = [
  { k: "P2P", v: "Trade SOL/USDC/NFT directly, no orderbook, no front-run." },
  { k: "Escrow", v: "Conditional settlement where the condition is a mutual term, not trust." },
  { k: "Negotiation", v: "Counterparty sees only a counter, never both parties' full position." },
  { k: "Settlement", v: "Atomic on base layer \u2014 or fully reverted when terms lapse." },
];

const METRICS = [
  { k: "ASSETS", v: "SOL · USDC · NFT" },
  { k: "SURFACES", v: "BASE + ER" },
  { k: "PAIRS", v: "06" },
  { k: "STATES", v: "06" },
];

export default function Home() {
  return (
    <div className="relative min-h-screen bg-background text-foreground antialiased">
      <GridBackground />

      <div className="relative z-10">
        {/* 01 — Header */}
        <header className="mx-auto flex max-w-6xl items-center justify-between border-b border-border-low px-6 py-4">
          <Link href="/" className="flex items-baseline gap-2">
            <span className="text-doto text-lg tracking-[0.3em]">CONVENANT</span>
            <span className="hidden text-[10px] uppercase tracking-[0.2em] text-muted sm:inline">
              private escrow
            </span>
          </Link>
          <nav className="hidden items-center gap-6 text-xs uppercase tracking-[0.15em] text-muted md:flex">
            <a href="#trust" className="transition-colors hover:text-foreground">
              Trust
            </a>
            <a href="#pairs" className="transition-colors hover:text-foreground">
              Pairs
            </a>
            <a href="#flow" className="transition-colors hover:text-foreground">
              Flow
            </a>
            <Link
              href="/dashboard"
              className="transition-colors hover:text-foreground"
            >
              Panel
            </Link>
          </nav>
          <div className="flex items-center gap-3">
            <ThemeToggle />
            <WalletButton />
          </div>
        </header>

        {/* 02 — Hero */}
        <section id="top" className="mx-auto max-w-6xl px-6 pt-16 pb-20 md:pt-24 md:pb-28">
          <p className="text-doto mb-6 text-xs uppercase tracking-[0.35em] text-muted">
            Solana · MagicBlock Ephemeral Rollups
          </p>
          <h1 className="max-w-4xl text-5xl font-medium leading-[0.95] tracking-tight sm:text-6xl md:text-7xl">
            Private trading
            <br />
            covenants.{" "}
            <span className="text-muted">Escrow,</span>
            <br />
            negotiated off-chain.
          </h1>
          <div className="mt-10 grid gap-6 md:grid-cols-2">
            <p className="max-w-md text-base leading-relaxed text-muted">
              Both parties lock their side into a covenant. Terms negotiate on a
              private Ephemeral Rollup — neither position is public on the
              base layer. Both accept and it settles atomically; neither does and
              it reverts to sender on expiry.
            </p>
            <div className="flex items-start gap-4 md:justify-end">
              <Link
                href="/create"
                className="rounded-none bg-primary px-6 py-3 text-sm font-medium uppercase tracking-[0.15em] text-primary-foreground transition hover:bg-primary/85"
              >
                Create covenant
              </Link>
              <Link
                href="/dashboard"
                className="rounded-none border border-border px-6 py-3 text-sm font-medium uppercase tracking-[0.15em] transition hover:border-foreground"
              >
                Dashboard
              </Link>
            </div>
          </div>
        </section>

        {/* 03 — Metric strip */}
        <section className="border-y border-border-low">
          <div className="mx-auto grid max-w-6xl grid-cols-2 divide-x divide-border-low md:grid-cols-4">
            {METRICS.map((m) => (
              <div key={m.k} className="px-6 py-6">
                <p className="text-doto text-[10px] uppercase tracking-[0.25em] text-muted">
                  {m.k}
                </p>
                <p className="mt-2 text-lg font-medium tracking-tight">{m.v}</p>
              </div>
            ))}
          </div>
        </section>

        {/* 04 — Trust model */}
        <section id="trust" className="mx-auto max-w-6xl px-6 py-20 md:py-28">
          <div className="grid gap-12 md:grid-cols-[1fr_1.4fr]">
            <div>
              <p className="text-doto mb-4 text-xs uppercase tracking-[0.3em] text-muted">
                04 / Trust
              </p>
              <h2 className="text-3xl font-medium tracking-tight md:text-4xl">
                No counterparty risk.
              </h2>
              <p className="mt-4 max-w-sm text-sm leading-relaxed text-muted">
                The covenant program is the only party with authority over locked
                assets. Humans never hold each other&apos;s funds — the program
                does, and it only ever releases on mutual acceptance or full
                revert.
              </p>
            </div>
            <div className="space-y-4">
              {STEPS.map((s) => (
                <div
                  key={s.n}
                  className="grid gap-3 border border-border-low p-5 sm:grid-cols-[3rem_1fr]"
                >
                  <span className="text-doto text-xs uppercase tracking-[0.2em] text-muted">
                    {s.n}
                  </span>
                  <div>
                    <h3 className="text-sm font-medium uppercase tracking-[0.12em]">
                      {s.title}
                    </h3>
                    <p className="mt-1 text-sm leading-relaxed text-muted">
                      {s.body}
                    </p>
                  </div>
                </div>
              ))}
            </div>
          </div>
        </section>

        {/* 05 — Protected pairs */}
        <section id="pairs" className="border-t border-border-low bg-card/40">
          <div className="mx-auto max-w-6xl px-6 py-20 md:py-28">
            <div className="mb-12 flex flex-wrap items-end justify-between gap-4">
              <div>
                <p className="text-doto mb-4 text-xs uppercase tracking-[0.3em] text-muted">
                  05 / Pairs
                </p>
                <h2 className="text-3xl font-medium tracking-tight md:text-4xl">
                  Every protected pair.
                </h2>
              </div>
              <p className="text-sm text-muted">
                Lock one side, demand the other. Settlement only on mutual term.
              </p>
            </div>
            <div className="grid gap-px bg-border-low sm:grid-cols-2 lg:grid-cols-3">
              {PROTECTED_PAIRS.map((p) => (
                <div key={p.a + p.b} className="bg-background p-6">
                  <p className="text-doto text-2xl tracking-wide">
                    {p.a} / {p.b}
                  </p>
                  <p className="mt-2 text-sm text-muted">{p.note}</p>
                </div>
              ))}
            </div>
          </div>
        </section>

        {/* 06 — Private by design */}
        <section className="mx-auto max-w-6xl px-6 py-20 md:py-28">
          <div className="grid gap-4 sm:grid-cols-2">
            <div className="border border-border-low p-8">
              <p className="text-doto mb-3 text-xs uppercase tracking-[0.25em] text-muted">
                Base layer
              </p>
              <p className="text-2xl font-medium tracking-tight">Settlement</p>
              <p className="mt-3 text-sm leading-relaxed text-muted">
                The covenant account exists on the public chain and the atomic
                swap lands there. Trust is inherited from Solana itself.
              </p>
            </div>
            <div className="border border-border-low p-8">
              <p className="text-doto mb-3 text-xs uppercase tracking-[0.25em] text-muted">
                Ephemeral rollup
              </p>
              <p className="text-2xl font-medium tracking-tight">Negotiation</p>
              <p className="mt-3 text-sm leading-relaxed text-muted">
                Once delegated, terms live inside a MagicBlock ER. Neither offer
                nor counter is public, and state persists only while the
                negotiation is live.
              </p>
            </div>
          </div>
        </section>

        {/* 07 — Use cases */}
        <section id="flow" className="border-t border-border-low">
          <div className="mx-auto max-w-6xl px-6 py-20 md:py-28">
            <p className="text-doto mb-4 text-xs uppercase tracking-[0.3em] text-muted">
              07 / Use
            </p>
            <h2 className="mb-12 text-3xl font-medium tracking-tight md:text-4xl">
              Built for the demo.
            </h2>
            <div className="grid gap-px bg-border-low sm:grid-cols-2">
              {USES.map((u) => (
                <div key={u.k} className="bg-background p-6">
                  <p className="text-doto mb-2 text-xs uppercase tracking-[0.2em] text-muted">
                    {u.k}
                  </p>
                  <p className="text-base leading-relaxed">{u.v}</p>
                </div>
              ))}
            </div>
          </div>
        </section>

        {/* 08 — Footer CTA */}
        <section id="cta" className="mx-auto max-w-6xl px-6 pb-20 md:pb-28">
          <div className="border border-border-low p-8 md:p-12">
            <p className="text-doto mb-3 text-xs uppercase tracking-[0.3em] text-muted">
              08 / Start
            </p>
            <h2 className="max-w-2xl text-3xl font-medium tracking-tight md:text-5xl">
              Lock both sides, then negotiate in the dark.
            </h2>
            <div className="mt-8 flex flex-wrap gap-3">
              <Link
                href="/create"
                className="rounded-none bg-primary px-6 py-3 text-sm font-medium uppercase tracking-[0.15em] text-primary-foreground transition hover:bg-primary/85"
              >
                Create covenant
              </Link>
              <Link
                href="/dashboard"
                className="rounded-none border border-border px-6 py-3 text-sm font-medium uppercase tracking-[0.15em] transition hover:border-foreground"
              >
                Open dashboard
              </Link>
            </div>
          </div>
          <footer className="mt-10 flex flex-wrap items-center justify-between gap-3 border-t border-border-low pt-6 text-[10px] uppercase tracking-[0.2em] text-muted">
            <span className="text-doto">CONVENANT</span>
            <span>Private negotiation · public settlement</span>
            <span>SOL · USDC · NFT</span>
          </footer>
        </section>
      </div>
    </div>
  );
}