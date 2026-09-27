import type { NextConfig } from "next";

const nextConfig: NextConfig = {
  // Traces only the files a deployment actually needs into .next/standalone
  // (~80% smaller than a full node_modules install) -- used by the Docker
  // image (Method 1) and the PM2 deployment (Method 2), both of which run
  // `node .next/standalone/server.js` rather than `next start`.
  output: "standalone",
  // The app now lives under /suite so the site root is free for a separate
  // marketing app. next/link, next/router, and redirect() calls are rewritten
  // automatically -- keep this in sync with BASE_PATH in src/lib/base-path.ts,
  // which covers the few raw window.location/URL call sites basePath doesn't touch.
  basePath: "/suite",
};

export default nextConfig;
