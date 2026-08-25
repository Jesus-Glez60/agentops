# Single image bundling both the Rust backend (agentops-server) and the
# Next.js frontend, supervised together by pm2-runtime -- see the
# deployment plan's Method 1 research note for why (the purist Docker
# model is one process per container; this app's app+api pairing is the
# documented exception PM2's own docs carve out for exactly this case).
#
# Two build stages (Rust, Node) feed a single Node-based runtime stage,
# since the frontend needs a Node runtime regardless and there's no
# distroless option that has both.

# --- Stage 1: Rust build ---
FROM rust:1-trixie AS rust-builder
WORKDIR /build

# rusqlite's `bundled` feature and tree-sitter-language-pack's build-time
# grammar compilation (see .cargo/config.toml's TSLP_LANGUAGES -- grammar
# *source* is fetched and compiled to .so at BUILD time, not downloaded at
# runtime) both need a C toolchain -- included by default here, but kept
# explicit in case a future base image drops it.
#
# Debian trixie, not bookworm: `ort` (ONNX Runtime, pulled in by
# fastembed for embeddings) downloads a prebuilt onnxruntime library at
# build time that references libstdc++ symbols (e.g. __cxa_call_terminate,
# basic_string::_M_replace_cold) only present in GCC 13+'s libstdc++ --
# bookworm ships GCC 12 and fails to link. Verified empirically: switching
# rust:1-bookworm -> rust:1-trixie (GCC 14) fixed the link error. The
# runtime stage below must stay on the same Debian release for glibc/
# libstdc++ ABI compatibility with this binary.
RUN apt-get update && apt-get install -y --no-install-recommends build-essential && rm -rf /var/lib/apt/lists/*

# Workspace-wide: agentops-server pulls in most of the 31-crate workspace,
# so there's no meaningful subset to copy instead of everything.
COPY Cargo.toml Cargo.lock ./
COPY .cargo ./.cargo
COPY agentops-core ./agentops-core
COPY docbrain-core ./docbrain-core

RUN cargo build --release --bin agentops-server

# --- Stage 2: Next.js build ---
FROM node:22-trixie-slim AS web-builder
WORKDIR /build/apps/web

COPY apps/web/package.json apps/web/package-lock.json ./
RUN npm ci

COPY apps/web ./
RUN npm run build

# --- Stage 3: runtime ---
FROM node:22-trixie-slim AS runtime
WORKDIR /app

RUN npm install -g pm2

# Mirrors the bare-metal (PM2/classic) repo-relative layout on purpose --
# target/release/agentops-server, apps/web/.next/standalone/server.js --
# so ecosystem.config.js's default paths work unmodified in both the
# Docker image and a plain checkout, instead of needing an image-specific
# override.
COPY --from=rust-builder /build/target/release/agentops-server ./target/release/agentops-server
COPY --from=web-builder /build/apps/web/.next/standalone ./apps/web/.next/standalone
COPY --from=web-builder /build/apps/web/.next/static ./apps/web/.next/standalone/.next/static
COPY --from=web-builder /build/apps/web/public ./apps/web/.next/standalone/public
COPY ecosystem.config.js ./

ENV NODE_ENV=production
ENV HOSTNAME=0.0.0.0
ENV PORT=3000
ENV AGENTOPS_ADDR=0.0.0.0:8420
ENV AGENTOPS_HEAVY_API_URL=http://127.0.0.1:8420

EXPOSE 3000 8420

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s \
  CMD node -e "fetch('http://127.0.0.1:8420/health').then(r=>process.exit(r.ok?0:1)).catch(()=>process.exit(1))"

ENTRYPOINT ["pm2-runtime", "ecosystem.config.js"]
