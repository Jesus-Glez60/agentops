# apps/web

The AgentOps web app (Next.js 16, React 19): Search, Knowledge Graph, Documentation Viewer, Gotchas, Libraries, Repositories (connect wizard + indexing), Settings, Profile, plus the `/login`, `/setup`, `/welcome`, `/invite/[token]` and `/cli-auth` (CLI device-login approval) flows.

It's a frontend only. Every screen reads from `agentops-server`, so run that first or the screens will look empty.

## Develop

```sh
# from the repo root: the API on :8420
cargo run --bin agentops-server

# in this directory
npm ci
npm run dev      # http://localhost:3000/suite
```

The app lives under a base path, `/suite` by default (`AGENTOPS_WEB_APP_BASE_PATH`, read in `next.config.ts`), because the marketing site owns the root in production.

| Env var | Default | Purpose |
|---|---|---|
| `AGENTOPS_HEAVY_API_URL` | `http://127.0.0.1:8420` | Where server-side code reaches `agentops-server`. |
| `AGENTOPS_PUBLIC_API_URL` | guessed from the request | The API address shown to users in connect instructions (`connect.sh`, `--remote`). Set it whenever the guess is wrong, e.g. behind a reverse proxy. |
| `AGENTOPS_WEB_APP_BASE_PATH` | `/suite` | URL prefix for the whole app. |

## Scripts

| Command | Action |
|---|---|
| `npm run dev` | Dev server. |
| `npm run build` | Production build (`output: "standalone"`, used by the root `Dockerfile`). |
| `npm run start` | Serve a production build. |
| `npm run lint` | ESLint. |
| `npm test` | Vitest. |

`public/llms.txt` and `public/llms-full.txt` are copies of the repo-root files; update them together.

See [`CLAUDE.md`](CLAUDE.md) before writing code here: this Next.js version differs from older ones, and its bundled docs in `node_modules/next/dist/docs/` are the reference.
