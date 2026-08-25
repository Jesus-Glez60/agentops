// Method 1 (Docker, via pm2-runtime) and Method 2 (PM2 bare-metal) both run
// this file unmodified -- see the Dockerfile's comment on why its COPY
// destinations mirror this repo's own relative layout instead of using an
// image-specific path override.
//
// Prerequisites this file does NOT build for you:
//   cargo build --release --bin agentops-server
//   (cd apps/web && npm ci && npm run build)   # needs output: "standalone" in next.config.ts
//
// Config (.env / AGENTOPS_* env vars) is read once at process start by
// both apps -- there's no hot-reload, so a config change (e.g. via the
// /setup page's POST /bootstrap/config) needs `pm2 restart ecosystem.config.js`
// to take effect.
const fs = require("fs");
const path = require("path");

// PM2 doesn't read `.env` itself (unlike `dotenvy` on the Rust CLI side --
// see agentops-cli/src/main.rs) -- parse it here so a `.env` written by
// `agentops init` or the `/setup` page's POST /bootstrap/config actually
// reaches both processes on `pm2 restart`, without adding an npm
// dependency just for this one file.
function loadDotEnv(envPath) {
  if (!fs.existsSync(envPath)) return {};
  const vars = {};
  for (const line of fs.readFileSync(envPath, "utf8").split("\n")) {
    const trimmed = line.trim();
    if (!trimmed || trimmed.startsWith("#")) continue;
    const eq = trimmed.indexOf("=");
    if (eq === -1) continue;
    vars[trimmed.slice(0, eq).trim()] = trimmed.slice(eq + 1).trim();
  }
  return vars;
}

const dotEnv = loadDotEnv(path.join(__dirname, ".env"));
// Real process env wins over `.env`, matching dotenvy's precedence on the
// Rust CLI side.
const env = { ...dotEnv, ...process.env };

module.exports = {
  apps: [
    {
      name: "agentops-server",
      script: path.join(__dirname, "target/release/agentops-server"),
      env: { ...env, AGENTOPS_ADDR: env.AGENTOPS_ADDR || "0.0.0.0:8420" },
    },
    {
      name: "agentops-web",
      script: "server.js",
      cwd: path.join(__dirname, "apps/web/.next/standalone"),
      env: {
        PORT: env.PORT || "3000",
        HOSTNAME: "0.0.0.0",
        AGENTOPS_HEAVY_API_URL: env.AGENTOPS_HEAVY_API_URL || "http://127.0.0.1:8420",
      },
    },
  ],
};
