import path from "node:path";
import { defineConfig } from "vitest/config";

// No @vitejs/plugin-react here — this repo's exact dependency graph has a
// peer conflict pulling it in (@rolldown/plugin-babel vs @babel/core), and
// Vitest's default esbuild transform already handles JSX/TSX fine for the
// scope these tests actually cover (src/lib/api, src/lib/graph/traverse.ts,
// src/lib/tenant-context.tsx — logic and one hook, not full component
// rendering). Revisit if a future test genuinely needs the Babel-based
// transform's extra features.
export default defineConfig({
  resolve: {
    alias: {
      "@": path.resolve(process.cwd(), "./src"),
    },
  },
  test: {
    environment: "jsdom",
    setupFiles: ["./vitest.setup.mts"],
    include: ["src/**/*.test.{ts,tsx}"],
  },
});
