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
const crypto = require("crypto");
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

// First PM2 start on a fresh install: agentops-server needs a secrets
// master key to start at all, and an instance API key to bind a
// non-loopback address, so without them the /setup page that writes `.env`
// could never be reached (finding F45). Generate whichever is missing into
// `.env` (mode 0600) and print the raw API key once. Never in a container:
// a key written to the container's own filesystem would silently change on
// every recreate and lock away every encrypted secret, so containers keep
// failing loudly until the key is set in their environment.
function ensureFirstRunKeys(envPath, dotEnv) {
  const inContainer = process.env.AGENTOPS_CONTAINER === "1" || fs.existsSync("/.dockerenv");
  const has = (name) => Boolean(process.env[name] || dotEnv[name]);
  if (inContainer) return dotEnv;
  const lines = [];
  if (!has("AGENTOPS_SECRETS_MASTER_KEY")) {
    lines.push(`AGENTOPS_SECRETS_MASTER_KEY=${crypto.randomBytes(32).toString("hex")}`);
  }
  let rawKey = null;
  if (!has("AGENTOPS_API_KEY_HASH")) {
    // Same shape as agentops_security::api_key::generate_api_key.
    rawKey = `ao_${crypto.randomBytes(32).toString("hex")}`;
    lines.push(`AGENTOPS_API_KEY_HASH=${crypto.createHash("sha256").update(rawKey).digest("hex")}`);
  }
  if (lines.length === 0) return dotEnv;
  const existing = fs.existsSync(envPath) ? fs.readFileSync(envPath, "utf8") : "";
  const prefix = existing && !existing.endsWith("\n") ? "\n" : "";
  fs.writeFileSync(envPath, `${existing}${prefix}# Generated on first PM2 start by ecosystem.config.js\n${lines.join("\n")}\n`);
  fs.chmodSync(envPath, 0o600);
  console.log(`ecosystem.config.js: wrote ${lines.map((l) => l.split("=")[0]).join(" and ")} to ${envPath}.`);
  if (rawKey) {
    console.log(`Instance API key (shown once; store it now): ${rawKey}`);
  }
  return loadDotEnv(envPath);
}

const envPath = path.join(__dirname, ".env");
const dotEnv = ensureFirstRunKeys(envPath, loadDotEnv(envPath));
// Real process env wins over `.env`, matching dotenvy's precedence on the
// Rust CLI side.
const env = { ...dotEnv, ...process.env };

// The only variables the web app reads (`process.env.*` in apps/web). Passed
// explicitly so values set only in `.env` reach it too (finding F41), without
// handing it the server's secrets.
const WEB_ENV_KEYS = ["AGENTOPS_PUBLIC_API_URL", "AGENTOPS_WEB_APP_BASE_PATH"];
const webEnv = Object.fromEntries(WEB_ENV_KEYS.filter((k) => env[k] !== undefined).map((k) => [k, env[k]]));

module.exports = {
  apps: [
    {
      name: "agentops-server",
      script: path.join(__dirname, "target/release/agentops-server"),
      env: { ...env, AGENTOPS_ADDR: env.AGENTOPS_ADDR || "0.0.0.0:8420" },
      // Defense-in-depth for the Postgres-not-ready-on-cold-start crash
      // loop (see agentops-store-open's own retry/backoff, the real fix):
      // without this, PM2's default zero-delay restarts could in theory
      // still burn through max_restarts faster than some future, slower
      // Postgres recovery than the 3.4s one actually observed. Caps at
      // 15000ms and resets to 0ms once the process survives 30s uninterrupted
      // (verified against PM2's own docs).
      exp_backoff_restart_delay: 100,
      max_restarts: 20,
    },
    {
      name: "agentops-web",
      script: "server.js",
      cwd: path.join(__dirname, "apps/web/.next/standalone"),
      env: {
        ...webEnv,
        PORT: env.PORT || "3000",
        HOSTNAME: "0.0.0.0",
        AGENTOPS_HEAVY_API_URL: env.AGENTOPS_HEAVY_API_URL || "http://127.0.0.1:8420",
      },
    },
  ],
};
