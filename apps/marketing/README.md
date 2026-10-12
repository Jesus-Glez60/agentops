# apps/marketing

The public AgentOps site (Astro + one React island for the knowledge-graph hero), live at https://agentops.dedyn.io. Pages: home (`index.astro`), `pricing.astro`, and one page per feature (`features/[id].astro`, generated from `src/data/features`).

There's deliberately no login/register flow here. Calls to action link to the self-host quick start in the main README (`src/lib/external-links.ts`). The only backend call is the contact form (`src/components/ContactForm.astro`), which posts to `POST /contact-leads` on the API.

## Develop

```sh
npm ci
npm run dev        # http://localhost:4321
```

| Command | Action |
|---|---|
| `npm run dev` | Dev server on `:4321`. |
| `npm run build` | Static build to `./dist/`. |
| `npm run preview` | Preview the build locally. |
| `npm test` | Builds first (`pretest`), then runs Vitest route smoke tests (`src/lib/route-smoke.test.ts`). |

## Config

`AGENTOPS_PUBLIC_API_URL` is read **at build time** and baked into the static output (default `https://api.agentops.dedyn.io`); it's where the contact form posts. `site` in `astro.config.mjs` sets canonical/Open Graph URLs.

## Deploy

The `Dockerfile` here is a multi-stage build that serves `dist/` with `serve` on `:4321` (pass `--build-arg AGENTOPS_PUBLIC_API_URL=...` for a different API). It runs as the `marketing` service in the root `docker-compose.yml`, and as `deploy/k8s/marketing-*.yaml` on Kubernetes. The bundled Caddy/Ingress config routes the site root here and `/suite/*` to the web app.
