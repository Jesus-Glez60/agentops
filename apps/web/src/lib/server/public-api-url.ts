import { headers } from "next/headers";

/**
 * The externally-reachable URL for the REST API `agentops connect --remote`
 * actually talks to -- a **different port** from the web app's own origin
 * in every deployment shape this app ships, not just a remapped one: even
 * the plain default `docker-compose up` exposes the web app on 3000 and
 * the API on 8420, two different ports. There is no way to derive "what
 * external port did the operator map 8420 to" from inside the container at
 * request time -- unlike the web app's own origin (which the incoming
 * request's own `Host` header already tells us), the API's public address
 * has to be told to us. `AGENTOPS_PUBLIC_API_URL` is a new, explicit,
 * runtime (not build-time -- `NEXT_PUBLIC_*` vars get baked into the
 * client bundle at `next build`, before a self-hoster's `docker-compose.yml`
 * port mapping is even known, which is why `NEXT_PUBLIC_AGENTOPS_API_URL`
 * elsewhere in this codebase has never actually worked for a Docker
 * deployment) env var an operator sets once. Falls back to a best-effort
 * guess (the web request's own hostname + the *internal* default API port,
 * 8420) when unset -- correct only for the unmapped-default case, wrong
 * for anything remapped like `thedamnserver`'s 18420. The caller is told
 * whether this was a real value or a guess, so the UI can warn rather than
 * silently hand out a broken command (caught exactly this way, live,
 * against a real remapped deployment).
 *
 * Shared by `/welcome` and `/repositories/connect/local` -- both need the
 * exact same "what's the real, externally-reachable API URL" answer to
 * build a working `agentops connect --remote` command.
 */
export async function publicApiUrl(): Promise<{ url: string; isGuess: boolean }> {
  const configured = process.env.AGENTOPS_PUBLIC_API_URL?.trim().replace(/\/$/, "");
  if (configured) {
    return { url: configured, isGuess: false };
  }
  const h = await headers();
  const proto = h.get("x-forwarded-proto") ?? "http";
  const hostHeader = h.get("x-forwarded-host") ?? h.get("host") ?? "localhost";
  const hostname = hostHeader.split(":")[0];
  return { url: `${proto}://${hostname}:8420`, isGuess: true };
}
