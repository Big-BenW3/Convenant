import { createClient } from "@solana/kit-client-rpc";
import type { ClusterUrl, TransactionSigner } from "@solana/kit";

/**
 * MagicBlock Ephemeral Rollup adapter.
 *
 * Centralizes the MagicBlock-specific endpoint, routing, and lifecycle logic so
 * it does not leak into the product layer. Devnet-only by design (mainnet is
 * blocked for the hackathon demo).
 */

export const MAGICBLOCK = {
  /** Base-layer devnet RPC. */
  baseRpc: "https://rpc.magicblock.app/devnet",
  /** Router used to discover which ER an account is delegated to. */
  router: "https://devnet-router.magicblock.app/",
  /** Fallback ER RPC; prefer the `fqdn` returned by getDelegationStatus. */
  erRpc: "https://devnet-as.magicblock.app/",
  /** TEE ER: enforces private permissions via Query Filtering + attestation.
   * Covenant terms must only ever be delegated here. */
  teeRpc: "https://devnet-tee.magicblock.app/",
} as const;

/** TEE provider endpoint used for private reads (challenge/login auth). */
export const TEE_PROVIDER_ENDPOINT = MAGICBLOCK.teeRpc;

/** MagicBlock system programs (SDK constants). */
export const DELEGATION_PROGRAM_ID =
  "DELeGGvXpWV2fqJUhqcF5ZSYMS4JTLjteaAMARRSaeSh" as const;
export const MAGIC_PROGRAM_ID = "Magic11111111111111111111111111111111111111" as const;
export const MAGIC_CONTEXT_ID = "MagicContext1111111111111111111111111111111" as const;

export type DelegationStatus = {
  isDelegated: boolean;
  fqdn?: string;
  delegationRecord?: {
    authority: string;
    owner: string;
    delegationSlot: number;
    lamports: number;
  };
};

export type Route = {
  identity: string;
  fqdn: string;
  baseFee: number;
  blockTimeMs: number;
  countryCode: string;
};

/**
 * Lists the live Ephemeral Rollup nodes from the Magic router. The node
 * `identity` is the validator pubkey that must be passed as the `validator`
 * account when delegating a permission (PER) — the ACL program requires it.
 * The `fqdn` is the ER endpoint for that node.
 */
export async function getRoutes(
  router: string = MAGICBLOCK.router,
): Promise<Route[]> {
  const response = await fetch(router, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "getRoutes" }),
  });
  const body = (await response.json()) as {
    result?: Route[];
    error?: { message?: string };
  };
  if (body.error) {
    throw new Error(body.error.message ?? "getRoutes failed");
  }
  return body.result ?? [];
}

/** Picks a route whose fqdn matches the given endpoint (fallback: first). */
export async function pickValidator(
  fqdn: string = MAGICBLOCK.erRpc,
): Promise<{ identity: string; fqdn: string } | undefined> {
  const routes = await getRoutes();
  return (
    routes.find((r) => r.fqdn.endsWith(fqdn)) ??
    routes.find((r) => r.fqdn === fqdn) ??
    routes[0]
  );
}

/**
 * Picks the TEE validator. Delegating a covenant to a non-TEE node would
 * disable private reads (permissions are enforced only on TEE nodes).
 */
export async function pickTeeValidator() {
  return pickValidator(TEE_PROVIDER_ENDPOINT);
}

/**
 * Obtains a TEE bearer token by signing the node's challenge (Auth flow).
 * `publicKey`/`signMessage` accept a @solana/web3.js PublicKey/Keypair, or a
 * kit signer's `address`/`signMessage` when available.
 */
export async function getAuthTokenFor(
  teeEndpoint: string,
  publicKey: { toString(): string },
  signMessage: (message: Uint8Array) => Promise<Uint8Array>,
): Promise<{ token: string; expiresAt: number }> {
  const { getAuthToken } = await import("@magicblock-labs/ephemeral-rollups-sdk");
  const base = teeEndpoint.endsWith("/") ? teeEndpoint.slice(0, -1) : teeEndpoint;
  const result = await getAuthToken(base, publicKey as never, async (msg: Uint8Array) => {
    const sig = await signMessage(msg);
    return sig as Uint8Array;
  });
  return result as { token: string; expiresAt: number };
}

export type PrivateAccountInfo = {
  data?: [string, "base64"];
  lamports?: bigint;
  owner?: string;
  executable?: boolean;
};

/**
 * Reads an account from the (private) ER with a bearer token, exactly like the
 * TEE's Query Filtering expects. Returns null when the account is absent or
 * the caller is not authorized.
 */
export async function fetchPrivateAccount(
  erEndpoint: string,
  token: string,
  account: string,
): Promise<PrivateAccountInfo | null> {
  const response = await fetch(erEndpoint, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      authorization: `Bearer ${token}`,
    },
    body: JSON.stringify({
      jsonrpc: "2.0",
      id: 1,
      method: "getAccountInfo",
      params: [account, { encoding: "base64" }],
    }),
  });
  const body = (await response.json()) as {
    error?: { message?: string };
    result?: { value: PrivateAccountInfo | null } | null;
  };
  if (body.error) {
    throw new Error(body.error.message ?? "private getAccountInfo failed");
  }
  return body.result?.value ?? null;
}

/**
 * Queries the MagicBlock router for the live delegation status of an account.
 * Never hardcode the ER endpoint — always discover it via the router.
 */
export async function getDelegationStatus(
  account: string,
  router: string = MAGICBLOCK.router,
): Promise<DelegationStatus> {
  const response = await fetch(router, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      jsonrpc: "2.0",
      id: 1,
      method: "getDelegationStatus",
      params: [account],
    }),
  });
  const body = (await response.json()) as {
    result?: DelegationStatus;
    error?: { message?: string };
  };
  if (body.error) {
    throw new Error(body.error.message ?? "getDelegationStatus failed");
  }
  return (
    body.result ?? { isDelegated: false }
  );
}

/** True when the base-layer account owner is the delegation program. */
export function accountIsDelegated(owner: string | undefined): boolean {
  return owner === DELEGATION_PROGRAM_ID;
}

/**
 * Builds a RPC client pointed at the ER endpoint discovered for a delegated
 * account. Reuses the wallet signer as the fee payer, matching the kit flow.
 */
export function createErClient(
  signer: TransactionSigner,
  erEndpoint: string = MAGICBLOCK.erRpc,
) {
  return createClient({
    url: erEndpoint as ClusterUrl,
    payer: signer,
  });
}

export type ErClient = ReturnType<typeof createErClient>;