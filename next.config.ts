import type { NextConfig } from "next";

// Convenant deploys from the repository root — the app, the Anchor workspace
// and the generated client all live here. `anchor/` is not part of the web
// build and nothing under it is imported by the Next.js app; vercel.json keeps
// it out of the deployment.

const nextConfig: NextConfig = {
  // `ws` is used by the devnet websocket client and must stay external.
  serverExternalPackages: ["ws"],
  // @solana/kit-plugin-payer's browser bundle has a spurious `import 'fs'`
  // from the payerFromFile export. Stub it out for the client bundle.
  turbopack: {
    resolveAlias: {
      fs: { browser: "./empty-module.js" },
    },
  },
  webpack: (config, { isServer }) => {
    if (!isServer) {
      config.resolve.fallback = { ...config.resolve.fallback, fs: false };
    }
    return config;
  },
};

export default nextConfig;
