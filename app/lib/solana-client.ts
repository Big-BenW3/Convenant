import { createEmptyClient } from "@solana/kit";
import { rpc } from "@solana/kit-plugin-rpc";

/**
 * Convenant runs against MagicBlock devnet only.
 *
 * The program is deployed to one MagicBlock devnet cluster, and every ER the
 * app talks to is a child of that same cluster. Pointing any part of the client
 * at mainnet or public devnet would send a devnet-only program client somewhere
 * the program does not exist — and public devnet is a *different* cluster from
 * MagicBlock devnet, so accounts would appear empty rather than erroring.
 */
export type ClusterMoniker = "devnet";

export const CLUSTERS: ClusterMoniker[] = ["devnet"];

const CLUSTER_URLS: Record<ClusterMoniker, string> = {
  devnet: "https://rpc.magicblock.app/devnet",
};

/**
 * MagicBlock devnet serves its websocket on the same host as the RPC.
 *
 * This must stay the *same cluster* as CLUSTER_URLS: public
 * `api.devnet.solana.com` is a different cluster from MagicBlock devnet, so
 * subscribing there would stream notifications for accounts the RPC cannot see.
 */
const CLUSTER_WS_URLS: Record<ClusterMoniker, string> = {
  devnet: "wss://rpc.magicblock.app/devnet",
};

export function getClusterWsConfig(cluster: ClusterMoniker) {
  return { url: CLUSTER_WS_URLS[cluster] };
}

export function getClusterUrl(cluster: ClusterMoniker) {
  return CLUSTER_URLS[cluster];
}

export function createSolanaClient(cluster: ClusterMoniker) {
  // No `rpcAirdrop()`: MagicBlock devnet rejects airdrops through the standard
  // RPC, and enabling the plugin on a shared client would advertise a faucet
  // against a cluster that has none.
  return createEmptyClient().use(
    rpc(CLUSTER_URLS[cluster], { url: CLUSTER_WS_URLS[cluster] })
  );
}

export type SolanaClient = ReturnType<typeof createSolanaClient>;
