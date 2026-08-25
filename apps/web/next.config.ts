import type { NextConfig } from "next";

const nextConfig: NextConfig = {
  // Traces only the files a deployment actually needs into .next/standalone
  // (~80% smaller than a full node_modules install) -- used by the Docker
  // image (Method 1) and the PM2 deployment (Method 2), both of which run
  // `node .next/standalone/server.js` rather than `next start`.
  output: "standalone",
};

export default nextConfig;
