import type { ClusterMoniker } from "./solana-client";

export function getExplorerUrl(path: string, cluster: ClusterMoniker): string {
  const url = new URL(path, "https://explorer.solana.com");
  // Devnet-only, so the cluster is always passed explicitly; without it the
  // explorer defaults to mainnet.
  url.searchParams.set("cluster", cluster);
  return url.toString();
}

export function ellipsify(str: string, chars = 4): string {
  if (str.length <= chars * 2 + 3) return str;
  return `${str.slice(0, chars)}...${str.slice(-chars)}`;
}
