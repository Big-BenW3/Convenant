"use client";

import { useState, useCallback, useMemo } from "react";
import { useSWRConfig } from "swr";
import type { Instruction } from "@solana/kit";
import { useWallet } from "../wallet/context";
import { createErClient } from "../magicblock";

/**
 * Mirrors `useSendTransaction` but sends through a MagicBlock Ephemeral
 * Rollup RPC client instead of the base layer. Used for negotiation and
 * commit instructions that run on a delegated account.
 */
export function useSendErTransaction(erEndpoint?: string) {
  const { signer } = useWallet();
  const { mutate } = useSWRConfig();
  const [isSending, setIsSending] = useState(false);

  const erClient = useMemo(
    () => (signer && erEndpoint ? createErClient(signer, erEndpoint) : null),
    [signer, erEndpoint]
  );

  const send = useCallback(
    async ({ instructions }: { instructions: readonly Instruction[] }) => {
      if (!erClient) throw new Error("Wallet not connected or ER endpoint unavailable");
      setIsSending(true);
      try {
        const result = await erClient.sendTransaction([...instructions]);
        mutate((key: unknown) => Array.isArray(key) && key[0] === "balance");
        return result.context.signature;
      } finally {
        setIsSending(false);
      }
    },
    [erClient, mutate]
  );

  return { send, isSending };
}