import { createClient } from "@solana/kit-client-rpc";
import { getBase64Encoder } from "@solana/kit";
import type { ClusterUrl, TransactionSigner } from "@solana/kit";

/**
 * Timeout for a single MagicBlock router / ER call. Without it a stalled
 * endpoint hangs the polling interval forever and the UI stops updating.
 */
const RPC_TIMEOUT_MS = 10_000;

/**
 * POSTs a JSON-RPC request with a bounded timeout, rejecting on a non-2xx
 * response or an empty body. Both the router and the tokenized ER return JSON,
 * so a failed call surfaces as an exception the caller can handle rather than
 * an undefined result that silently reads as "not delegated".
 */
async function postJson(
  url: string,
  body: unknown,
  headers: Record<string, string> = {},
): Promise<unknown> {
  const response = await fetch(url, {
    method: "POST",
    headers: { "content-type": "application/json", ...headers },
    body: JSON.stringify(body),
    signal: AbortSignal.timeout(RPC_TIMEOUT_MS),
  });
  if (!response.ok) {
    throw new Error(`${url} responded ${response.status}`);
  }
  const parsed = (await response.json()) as {
    result?: unknown;
    error?: { message?: string };
  };
  if (parsed.error) {
    throw new Error(parsed.error.message ?? "MagicBlock RPC error");
  }
  return parsed.result;
}

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
/** MagicBlock ephemeral vault; sponsors PER account rent on the ER. */
export const EPHEMERAL_VAULT_ID = "MagicVau1t999999999999999999999999999999999" as const;

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
  const result = await postJson(router, {
    jsonrpc: "2.0",
    id: 1,
    method: "getRoutes",
  });
  return (result as Route[]) ?? [];
}

/**
 * Normalizes a router fqdn for comparison.
 *
 * The router has been observed returning values like
 * `https://https://devnet-tee.magicblock.app/` — a scheme that has been
 * duplicated upstream — so the scheme and any path are stripped before
 * comparing, leaving just the host.
 */
export function normalizeFqdn(fqdn: string): string {
  return fqdn
    .trim()
    .replace(/^[a-z][a-z0-9+.-]*:\/\//i, "")
    .replace(/^[a-z][a-z0-9+.-]*:\/\//i, "") // tolerate a doubled scheme
    .split("/")[0]
    .toLowerCase();
}

/**
 * Finds the route serving `fqdn`, matching on host rather than the full URL so
 * a trailing slash or a duplicated scheme still resolves.
 */
export async function pickRoute(
  fqdn: string,
  router: string = MAGICBLOCK.router,
): Promise<Route | undefined> {
  const wanted = normalizeFqdn(fqdn);
  const routes = await getRoutes(router);
  return routes.find((r) => normalizeFqdn(r.fqdn) === wanted);
}

/**
 * Resolves the validator identity for an endpoint.
 *
 * Throws when the endpoint has no matching route: silently falling back to an
 * arbitrary node would pin the covenant to the wrong ER, and for a TEE
 * endpoint specifically it would disable private reads without any visible
 * symptom.
 */
export async function pickValidator(
  fqdn: string = MAGICBLOCK.teeRpc,
  router: string = MAGICBLOCK.router,
): Promise<{ identity: string; fqdn: string }> {
  const route = await pickRoute(fqdn, router);
  if (!route) {
    throw new Error(
      `No MagicBlock route serves ${fqdn}; refusing to delegate to an arbitrary node.`,
    );
  }
  return { identity: route.identity, fqdn: route.fqdn };
}

/**
 * Picks the TEE validator. Delegating a covenant to a non-TEE node would
 * disable private reads (permissions are enforced only on TEE nodes), so this
 * fails loudly rather than falling back.
 */
export async function pickTeeValidator(router: string = MAGICBLOCK.router) {
  return pickValidator(TEE_PROVIDER_ENDPOINT, router);
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
  const result = (await postJson(
    erEndpoint,
    {
      jsonrpc: "2.0",
      id: 1,
      method: "getAccountInfo",
      params: [account, { encoding: "base64" }],
    },
    { authorization: `Bearer ${token}` },
  )) as { value: PrivateAccountInfo | null } | null;
  return result?.value ?? null;
}

/**
 * Queries the MagicBlock router for the live delegation status of an account.
 * Never hardcode the ER endpoint — always discover it via the router.
 */
export async function getDelegationStatus(
  account: string,
  router: string = MAGICBLOCK.router,
): Promise<DelegationStatus> {
  const result = await postJson(router, {
    jsonrpc: "2.0",
    id: 1,
    method: "getDelegationStatus",
    params: [account],
  });
  return (result as DelegationStatus | undefined) ?? { isDelegated: false };
}

/** True when the base-layer account owner is the delegation program. */
export function accountIsDelegated(owner: string | undefined): boolean {
  return owner === DELEGATION_PROGRAM_ID;
}

/**
 * Builds a client pointed at the ER endpoint discovered for a delegated
 * account, carrying a bearer token when one is supplied.
 *
 * The tokenized TEE endpoint refuses unauthenticated `getAccountInfo` against a
 * private account, so any read or send that touches covenant state needs one.
 */
export function createErClient(
  signer: TransactionSigner,
  erEndpoint: string = MAGICBLOCK.erRpc,
  authToken?: string,
) {
  return createClient({
    url: `${erEndpoint}${authToken ? `?token=${encodeURIComponent(authToken)}` : ""}` as ClusterUrl,
    payer: signer,
  });
}

/**
 * The ER endpoint for an account, with its scheme normalised so a doubled
 * `https://https://…` from the router cannot produce an unusable URL.
 */
export function erEndpointUrl(fqdn: string): string {
  const trimmed = fqdn.trim();
  if (/^https?:\/\//i.test(trimmed)) {
    // Re-add a single scheme; the host part may itself be duplicated upstream.
    return `https://${normalizeFqdn(trimmed)}`;
  }
  return `https://${trimmed}`;
}

/**
 * Reads a covenant from a private ER account, authenticating with a TEE bearer
 * token. Returns null when the account is absent or the caller is not one of
 * the covenant's parties — the TEE denies unauthorised reads rather than
 * returning data.
 */
export async function readPrivateCovenant<T>(
  erEndpoint: string,
  token: string,
  account: string,
  decode: (bytes: Uint8Array) => T,
): Promise<T | null> {
  const info = await fetchPrivateAccount(erEndpoint, token, account);
  if (!info?.data) return null;
  // `getBase64Encoder` turns the RPC's base64 string back into raw account
  // bytes (getBase64Decoder does the opposite direction).
  //
  // The whole buffer is handed to `decode`: the Codama-generated account
  // decoders are struct codecs whose first field IS the 8-byte Anchor
  // discriminator, so stripping it here throws "enum discriminator out of
  // range" on every private read.
  return decode(new Uint8Array(getBase64Encoder().encode(info.data[0])));
}

export type ErClient = ReturnType<typeof createErClient>;