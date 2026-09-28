# Static Astro build, served by `serve` (not `astro preview`, which isn't
# meant for production use -- see Astro's own deploy docs). Multi-stage so
# the runtime image doesn't carry the Astro/Vite/Node toolchain used to
# build it, matching the repo-root Dockerfile's shape for the main app.
FROM node:22-alpine AS build
WORKDIR /app
COPY package.json package-lock.json* ./
RUN npm ci
COPY . .
# AGENTOPS_PUBLIC_API_URL is read at build time (see
# src/components/ContactForm.astro) and baked into the static output --
# passed as a build arg, not runtime env, since there is no server process
# left to read it at request time. (This site has no login/register flow
# and so no AGENTOPS_WEB_APP_URL/_BASE_PATH to configure -- every such spot
# links to the README's self-host quick start instead; see
# src/lib/external-links.ts.)
ARG AGENTOPS_PUBLIC_API_URL=https://api.agentops.dedyn.io
ENV AGENTOPS_PUBLIC_API_URL=$AGENTOPS_PUBLIC_API_URL
RUN npm run build

FROM node:22-alpine
WORKDIR /app
RUN npm install -g serve@14
COPY --from=build /app/dist ./dist
EXPOSE 4321
# No `-s`/`--single` -- that flag rewrites every path to index.html (SPA
# fallback), which is wrong for this static multi-page Astro site: it was
# making every route (e.g. /pricing/, /features/graph/) silently serve the
# homepage's HTML instead of its own generated page. `serve` finds each
# route's own `<path>/index.html` correctly without it.
CMD ["serve", "dist", "-l", "4321"]
