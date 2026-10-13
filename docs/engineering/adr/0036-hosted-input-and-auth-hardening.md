# 0036. Hosted input and auth hardening

Follows findings F31, F35, F37, F38 and F39. Rationale comes from the findings, the author's decisions of 2026-10-13, and the security-fix plan reviewed by the five-member plan council.

## Status

Accepted, 2026-10-13. Commits `d9f447b`, `17a0407`, `ffd9c1e`, `2232420` and `7026b45` on branch `security/hosted-hardening`.

## Context

The engineering docs found five exposures on the hosted server:

- **F37:** without `AGENTOPS_API_KEY_HASH`, routes falling back to the instance key were unauthenticated. It was live: anonymous `GET /tools` returned 200 in production on 2026-10-13.
- **F31:** hosted `/mcp` rewrote only `path`, so `notes_path` reached the handler as a literal server path.
- **F35:** `scrape_library` fetched any registered URL from the server's own network, and stored the response where the caller could read it.
- **F38:** roles did not gate `/mcp` or curation.
- **F39:** no limits on login, 2FA, signup or device approval; a 2FA challenge allowed unlimited retries.

## Decision

1. **Fail closed at start-up.** `agentops-server` refuses to start when the bind is not loopback-only and no instance key is configured. A solo loopback setup needs no key. `AGENTOPS_ALLOW_UNAUTHENTICATED` is an explicit opt-out. The wizards generate a key.
2. **Refuse path-like arguments on hosted `/mcp`.** `HOSTED_FORBIDDEN_ARGS` (currently `notes_path`) is rejected before dispatch, for every tool. Local stdio keeps the argument, since the caller is the machine's own user.
3. **Fetch user-supplied doc URLs from public addresses only.** A ureq resolver wrapper refuses non-public addresses at DNS-resolution time on every connection, which covers redirects and DNS rebinding. URLs must be http(s). Pages per scrape and concurrent background jobs are capped. `AGENTOPS_ALLOW_PRIVATE_DOC_URLS` opts out for intranet docs.
4. **No role gating for knowledge writes or curation.** Any authorized org member may record knowledge and curate (author decision): the knowledge comes from working on the codebase. The org's access mode is the only `/mcp` gate.
5. **Rate-limit unauthenticated auth routes in-process.** Fixed windows keyed per account, per address, per 2FA challenge and per user; 429 with `Retry-After`. Five wrong codes revoke a 2FA challenge.
6. **One trusted client-IP rule.** `X-Real-IP` is trusted only from a loopback or private TCP peer; the first `X-Forwarded-For` hop is never used. Proxies must set `X-Real-IP` and overwrite client values; the web auth proxy forwards the browser's address.

## Alternatives considered

- **`tower_governor` for rate limits.** Rejected: it needs the same connect-info wiring plus a custom key extractor, and its `SmartIpKeyExtractor` reads `X-Forwarded-For` first, which a client controls behind this nginx. The limits also need account and challenge keys.
- **Checking the URL text for private hosts.** Rejected: a hostname can resolve to a private address, and redirects bypass a one-time check. Resolution-time checks cover both.
- **Per-user limits on authenticated account routes.** Deferred: those need a live session, a different threat (`findings.md` F46).
- **Role-gating `/mcp` writes.** Rejected by the author (decision 4).

## Consequences

- Every network-reachable deployment must configure an instance key; Docker and Kubernetes examples and the README say so.
- Self-hosters behind their own proxy must set `X-Real-IP`, or all browser logins share one address bucket.
- Rate-limit counters live in memory and reset on restart; this relies on the single-instance deployment (`findings.md` F42).
- ureq's `Resolver` trait is in its `unversioned` module, so a ureq upgrade can break `net.rs`.
- Scraping intranet docs needs an explicit opt-out.

## Evidence

- `findings.md` F31, F35, F37, F38, F39 (resolved entries name their commits).
- `startup_auth_check` in `agentops-core/crates/agentops-server/src/lib.rs`; `BootstrapConfig::ensure_instance_key` in `agentops-core/crates/agentops-manifest/src/bootstrap.rs`.
- `HOSTED_FORBIDDEN_ARGS` in `agentops-core/crates/agentops-heavy-api/src/mcp_http.rs`.
- `PublicOnlyResolver`, `DOC_AGENT`, `validate_docs_url` in `docbrain-core/crates/docbrain-ingest/src/net.rs`; `JobSlot`, `MAX_SCRAPE_PAGES` in `docbrain-core/crates/docbrain-mcp/src/tools.rs`.
- `FixedWindowLimiter` in `agentops-core/crates/agentops-heavy-api/src/rate_limit.rs`; `client_ip` in `agentops-core/crates/agentops-heavy-api/src/client_ip.rs`; `clientIpHeaders` in `apps/web/src/lib/server/auth-proxy.ts`.
- Production nginx sets `X-Real-IP $remote_addr` for both the API and web sites (checked 2026-10-13).
