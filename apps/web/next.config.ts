import type { NextConfig } from "next";

// Single source of truth for the app's basePath, shared with the backend
// (agentops-heavy-api's `web_app_base_path()`, which reads the identical
// env var name) instead of each side hardcoding "/suite" independently --
// see that function's doc comment for the full rationale. Defaults to
// "/suite" (today's committed default) when unset.
const BASE_PATH = process.env.AGENTOPS_WEB_APP_BASE_PATH || "/suite";

const nextConfig: NextConfig = {
  // Traces only the files a deployment actually needs into .next/standalone
  // (~80% smaller than a full node_modules install) -- used by the Docker
  // image (Method 1) and the PM2 deployment (Method 2), both of which run
  // `node .next/standalone/server.js` rather than `next start`.
  output: "standalone",
  // The app now lives under BASE_PATH so the site root is free for a
  // separate marketing app. next/link, next/router, and redirect() calls
  // are rewritten automatically -- src/lib/base-path.ts covers the few raw
  // window.location/URL/fetch call sites basePath doesn't touch, and reads
  // this exact same env var (via the `env` field below, which inlines it
  // into both server and client bundles under its own name, same as
  // `NEXT_PUBLIC_*` vars but without requiring that prefix).
  basePath: BASE_PATH,
  env: {
    AGENTOPS_WEB_APP_BASE_PATH: BASE_PATH,
  },
};

export default nextConfig;
