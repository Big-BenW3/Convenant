"use client";

import { useState } from "react";
import Link from "next/link";
import { address } from "@solana/kit";
import {
  getInitializeCovenantInstructionAsync,
  AssetType,
} from "../generated/convenant";
import { useWallet } from "../lib/wallet/context";
import { useSendTransaction } from "../lib/hooks/use-send-transaction";
import {
  covenantAddress,
  vaultAddress,
  ataAddress,
  buildCreateTokenAccount,
  SYSTEM_PROGRAM_ID,
  DEMO_MINTS,
} from "../lib/covenant";
import { GridBackground } from "../components/grid-background";
import { WalletButton } from "../components/wallet-button";
import { ThemeToggle } from "../components/theme-toggle";

type CreatorAsset = "SOL" | "USDC" | "NFT";

const SPL_MINTS: Record<Exclude<CreatorAsset, "SOL">, ReturnType<typeof address>> = {
  USDC: address(DEMO_MINTS.USDC),
  NFT: address(DEMO_MINTS.NFT),
};

export default function CreatePage() {
  const { signer, status } = useWallet();
  const { send, isSending } = useSendTransaction();

  const [asset, setAsset] = useState<CreatorAsset>("SOL");
  const [counterparty, setCounterparty] = useState("");
  const [amount, setAmount] = useState("");
  const [amountBig, setAmountBig] = useState("0");
  const [days, setDays] = useState("7");

  const [error, setError] = useState<string | null>(null);
  const [confirmed, setConfirmed] = useState<{ address: string; sig: string } | null>(
    null
  );

  const connected = status === "connected" && !!signer;

  const mintLabel = asset === "SOL" ? SYSTEM_PROGRAM_ID : SPL_MINTS[asset];

  const handleCreate = async () => {
    setError(null);
    setConfirmed(null);
    if (!signer) {
      setError("Connect a wallet first.");
      return;
    }
    const nonce = BigInt(Date.now());
    const created = await covenantAddress(signer.address, nonce);
    const vault = await vaultAddress(created);
    if (!counterparty) {
      setError("Enter a counterparty address.");
      return;
    }
    const amountVal = BigInt(amountBig || "0");
    if (amountVal <= 0n) {
      setError("Enter an amount greater than zero.");
      return;
    }
    const daysNum = Number(days);
    if (!Number.isFinite(daysNum) || daysNum <= 0) {
      setError("Deadline must be a positive number of days.");
      return;
    }
    const deadline =
      Math.floor(Date.now() / 1000) + Math.floor(daysNum * 24 * 60 * 60);

    try {
      const creatorMint =
        asset === "SOL" ? address(SYSTEM_PROGRAM_ID) : SPL_MINTS[asset];

      const [sourceTokenAccount, vaultTokenAccount, mintAccount] =
        asset === "SOL"
          ? [vault, vault, address(SYSTEM_PROGRAM_ID)]
          : [
              await ataAddress(signer.address, SPL_MINTS[asset]),
              await ataAddress(vault, SPL_MINTS[asset]),
              SPL_MINTS[asset],
            ];

      // The creator's own token account and the vault's must both exist before
      // `initialize_covenant` locks the asset into the vault. Creation is
      // idempotent, so pre-creating an existing account is a no-op.
      const setupIxs =
        asset === "SOL"
          ? []
          : [
              await buildCreateTokenAccount(
                signer,
                signer.address,
                SPL_MINTS[asset],
              ),
              await buildCreateTokenAccount(signer, vault, SPL_MINTS[asset]),
            ];

      const creatorAsset =
        asset === "SOL"
          ? AssetType.Sol
          : asset === "USDC"
            ? AssetType.Token
            : AssetType.Nft;

      const instruction = await getInitializeCovenantInstructionAsync({
        creator: signer,
        covenant: created,
        vault,
        sourceTokenAccount,
        vaultTokenAccount,
        mintAccount,
        tokenProgram: address("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA"),
        systemProgram: address(SYSTEM_PROGRAM_ID),
        counterparty: address(counterparty),
        creatorAsset,
        creatorMint,
        creatorAmount: amountVal,
        deadline: BigInt(deadline),
        nonce,
      });

      const sig = await send({ instructions: [...setupIxs, instruction] });
      setConfirmed({ address: created, sig });
    } catch (err) {
      console.error(err);
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  return (
    <div className="relative min-h-screen bg-background text-foreground antialiased">
      <GridBackground />
      <div className="relative z-10 mx-auto max-w-3xl px-6">
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
            New covenant
          </p>
          <h1 className="text-3xl font-medium tracking-tight md:text-4xl">
            Compose the offer
          </h1>
          <p className="mt-3 max-w-xl text-sm leading-relaxed text-muted">
            Choose the asset you lock, who you trade with, and the deadline. Both
            sides are escrowed before negotiation starts.
          </p>

          <div className="mt-10 space-y-8">
            {/* Asset type */}
            <section>
              <p className="mb-3 text-xs uppercase tracking-[0.2em] text-muted">
                Offer
              </p>
              <div className="grid grid-cols-3 gap-px bg-border-low">
                {(["SOL", "USDC", "NFT"] as const).map((a) => (
                  <button
                    key={a}
                    onClick={() => setAsset(a)}
                    className={`cursor-pointer p-4 text-center text-sm font-medium uppercase tracking-[0.15em] transition ${
                      asset === a
                        ? "bg-primary text-primary-foreground"
                        : "bg-background text-muted hover:text-foreground"
                    }`}
                  >
                    {a}
                  </button>
                ))}
              </div>
              {asset !== "SOL" && (
                <p className="mt-2 font-mono text-[11px] text-muted">
                  mint {mintLabel}
                </p>
              )}
            </section>

            {/* Amount */}
            <section>
              <p className="mb-3 text-xs uppercase tracking-[0.2em] text-muted">
                Amount {asset}
              </p>
              <div className="flex gap-px bg-border-low">
                <div className="flex-1 bg-background">
                  <input
                    inputMode="decimal"
                    value={amount}
                    onChange={(e) => {
                      setAmount(e.target.value);
                      try {
                        const v = Number(e.target.value);
                        if (v > 0) setAmountBig(v.toString());
                      } catch {
                        /* noop */
                      }
                    }}
                    placeholder="0.0"
                    className="w-full bg-transparent p-4 font-mono text-xl tabular-nums outline-none placeholder:text-muted"
                  />
                </div>
                <div className="flex items-center bg-background px-4 text-xs uppercase tracking-[0.2em] text-muted">
                  {asset}
                </div>
              </div>
              {asset === "SOL" && (
                <p className="mt-2 font-mono text-[11px] text-muted">
                  {`${amountBig || 0} lamports`}
                </p>
              )}
            </section>

            {/* Counterparty */}
            <section>
              <p className="mb-3 text-xs uppercase tracking-[0.2em] text-muted">
                Counterparty
              </p>
              <input
                value={counterparty}
                onChange={(e) => setCounterparty(e.target.value)}
                placeholder="wallet address"
                className="w-full bg-transparent border border-border-low p-4 font-mono text-sm outline-none placeholder:text-muted focus:border-foreground"
              />
            </section>

            {/* Deadline */}
            <section>
              <p className="mb-3 text-xs uppercase tracking-[0.2em] text-muted">
                Deadline
              </p>
              <div className="grid grid-cols-2 gap-px bg-border-low">
                <div className="flex items-center bg-background px-4">
                  <input
                    inputMode="numeric"
                    value={days}
                    onChange={(e) => setDays(e.target.value)}
                    className="w-full bg-transparent p-4 font-mono text-xl tabular-nums outline-none placeholder:text-muted"
                  />
                </div>
                <div className="flex items-center bg-background px-4 text-xs uppercase tracking-[0.2em] text-muted">
                  days
                </div>
              </div>
            </section>

            <button
              onClick={handleCreate}
              disabled={!connected || isSending}
              className="w-full cursor-pointer rounded-none bg-primary py-4 text-sm font-medium uppercase tracking-[0.2em] text-primary-foreground transition hover:bg-primary/85 disabled:cursor-not-allowed disabled:opacity-40"
            >
              {!connected
                ? "Connect wallet to create"
                : isSending
                  ? "Creating…"
                  : "Lock offer & create"}
            </button>

            {error && (
              <p className="border border-border-low p-3 font-mono text-xs text-foreground">
                {error}
              </p>
            )}

            {confirmed && (
              <div className="border border-border-low p-5">
                <p className="text-doto mb-2 text-xs uppercase tracking-[0.2em] text-muted">
                  Submitted
                </p>
                <p className="truncate font-mono text-xs">covenant {confirmed.address}</p>
                <p className="truncate font-mono text-xs text-muted">sig {confirmed.sig}</p>
                <Link
                  href="/dashboard"
                  className="mt-4 inline-block text-xs uppercase tracking-[0.2em] underline underline-offset-4"
                >
                  Track in dashboard
                </Link>
              </div>
            )}
          </div>
        </main>
      </div>
    </div>
  );
}