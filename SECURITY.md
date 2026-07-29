# Security

This project reads entire codebases. A tool whose job is understanding your whole
repository is, by construction, a high-value target — this document exists because
that risk profile is different from a typical CLI, not despite it.

See the full threat model and design rationale in the project plan:
`/Users/jesusglez/.claude/plans/i-m-thinking-that-now-modular-sparrow.md` (§Security).
This file is the operational summary; the plan is the reasoning behind it.

## Reporting a vulnerability

This repository is currently private and pre-release. Until it's public, report
issues directly to the maintainer rather than opening a public GitHub issue.
A public disclosure policy will be published alongside the first open-source release.

## Controls in place (Phase 1)

- **Secret redaction gate (`agentops-security`)** — every chunk of raw source text is
  scanned for credential-shaped strings before it's written into the graph store or
  any generated document. Matches are replaced with `[REDACTED:<rule>]`, never
  silently dropped, and the CLI reports a count. This is mandatory by default.
- **Known secret-bearing filenames excluded from the walk itself** — `.env*`, `*.pem`,
  `*.key`, `id_rsa*` and similar are never read into memory in the first place,
  independent of the redaction gate.
- **Zero-network-egress invariant for `agentops-scanner`** — this crate must never
  gain a *runtime* HTTP/networking dependency. Enforced mechanically via `deny.toml`'s
  `[[bans.deny]]` list (`reqwest`, `hyper-tls`, `native-tls`, `openssl`) — run
  `cargo deny check` in CI on every PR.
  - **Known, understood exception, verified empirically (not assumed from docs)**:
    `tree-sitter-language-pack` (used by `agentops-scanner` for AST extraction) has a
    *default* `download` feature that auto-fetches precompiled grammar binaries over
    the network at **runtime** on first use — left enabled, this would be a real
    runtime-egress violation. It's disabled in the workspace `Cargo.toml`
    (`default-features = false`). Instead, `.cargo/config.toml` sets `TSLP_LANGUAGES`
    to our 4 supported languages, which makes the crate's build script fetch grammar
    *source* and compile each into a `.dylib`/`.so` under the build output directory —
    at `cargo build` time, via `ureq` as a genuine build-dependency (confirmed via
    `cargo tree -e normal,build -i ureq`, which shows it only under
    `[build-dependencies]`) — then load it via `libloading` (the `dynamic-loading`
    feature, kept enabled) from that same local path at runtime. Net effect: building
    `agentops-scanner` from source requires network access; the compiled binary makes
    zero network calls when actually scanning a repository. This was verified against
    a throwaway probe crate (build the dependency graph, inspect the generated
    `registry_generated.rs`, confirm `get_parser()` actually returns a working parser)
    rather than trusted from the crate's documentation, which turned out to describe
    the default (`download`-enabled) behavior, not this project's configuration.
    `ureq` is intentionally not in the `deny.toml` ban list — banning it would break
    the build — but this distinction (build-time vs. runtime egress) is exactly why
    the invariant is scoped to runtime dependencies, not "no networking crate anywhere
    in the tree."
- **Injection-aware output formatting** (`agentops-security::wrap_repo_content`,
  implemented but not yet wired into `agentops-docgen`'s output) — raw repository
  content in generated docs will be wrapped with an explicit delimiter and framing
  note distinguishing "repository content" from "instructions," since comments/
  READMEs in a scanned repo are untrusted input to whatever agent reads our output
  next.
- **Pinned Ruler version** (`agentops-ruler-bridge::RULER_VERSION`, implemented) —
  `apply()` always invokes `@intellectronica/ruler@0.3.44` explicitly, never
  `@latest` or an unversioned `npx @intellectronica/ruler`, given the precedent of
  supply-chain attacks against agentic tooling via malicious npm postinstall
  scripts. **Residual limitation, stated plainly**: this pins the *version string*
  only — it relies on the npm registry's own tarball-integrity check, not an
  additional locally-vendored checksum/lockfile independent of npm. Bumping
  `RULER_VERSION` is a deliberate code change (reviewed like any other dependency
  bump), not something that happens silently.

## Known, tracked exceptions

- **`RUSTSEC-2025-0119`** — `number_prefix` (a transitive dependency of `indicatif`,
  used for the CLI's progress-bar byte-size formatting) is unmaintained. Not a
  vulnerability, no known exploit, no safe upgrade currently available upstream.
  Ignored explicitly in `deny.toml` with this justification attached. Revisit when
  `indicatif` drops it or a maintained fork appears.
- **`CDLA-Permissive-2.0`** — `webpki-root-certs`/`webpki-roots` (transitive via
  `ureq`'s TLS stack) use this permissive-but-non-standard license. Allowed
  explicitly in `deny.toml`.

## Controls in place (Phase 1, continued)

- **`AccessMode::Advisor` structural enforcement (`agentops-mcp`)** — implemented, not
  just a plan item anymore. `Advisor` mode's `tools/list` response genuinely never
  includes `scan_repo`/`add_note`/`generate_docs` (the tool definitions aren't in the
  list the server builds — a client has no way to discover them exist). `call_tool`
  also re-checks the mode defensively at the dispatch layer, so even a
  hand-constructed `tools/call` naming a write tool directly is refused with a normal
  tool-result error, not executed. Verified against the real hand-rolled JSON-RPC/
  stdio server (not just unit tests): `tools/list` in `Advisor` mode returns exactly
  `["status", "list_gotchas", "repo_map"]`; `Full` mode adds `scan_repo`/`add_note`/
  `generate_docs`. This is the "an AI agent that structurally cannot" guarantee the
  plan called for, as opposed to a system-prompt suggestion.
- **`agentops-api` (implemented)** — the REST transport reuses `agentops-mcp`'s
  `list_tools`/`call_tool` directly (one implementation of each tool, two
  transports), so the same `AccessMode` guarantee holds over HTTP: `GET /tools` in
  Advisor mode omits the write tools, and `POST /tools/scan_repo` in Advisor mode
  returns `403 Forbidden` before any repo-scanning code runs — verified against a
  live server with `curl`, not just in-process tests.
- **API-key authentication (`agentops-security::api_key`, implemented)** — opt-in,
  applied identically to `agentops-api` and `docbrain-api` via an axum
  `middleware::from_fn_with_state` layer. Keys are 32 random bytes from the OS
  CSRNG (`getrandom`), shown to the operator once at generation time; only a
  SHA-256 hash is ever configured on the server (`AGENTOPS_API_KEY_HASH` /
  `DOCBRAIN_API_KEY_HASH`), so a compromised server config doesn't hand over a
  usable key. Verification compares hashes via `subtle::ConstantTimeEq`, not
  `==`, so response timing can't be used to guess a key byte-by-byte. `/health`
  is exempt (so uptime checks/load balancers work without a key); every other
  route returns `401 Unauthorized` on a missing or wrong `Authorization: Bearer
  <key>` header once a hash is configured. Auth is opt-in rather than
  mandatory-by-default: if no hash is configured, the server runs
  unauthenticated exactly as before, which remains fine for the CLI's
  localhost-only `serve-api`/`docbrain-serve-api` workflows (Phase 1) but MUST
  be turned on before either server is ever bound beyond `127.0.0.1` (Phase 3
  hosted heavy tier).
- **`docbrain-api`'s CORS policy is deliberately permissive** (`CorsLayer::permissive()`,
  any origin) so the local dashboard (`apps/web`, a separate origin during
  development) can call it directly from the browser. This is now a real gap
  worth naming precisely rather than the "no auth yet" framing used before
  API-key auth shipped: permissive CORS plus `Authorization`-header auth is
  still safe (browsers don't attach an `Authorization` header cross-origin
  automatically the way they do cookies, so another site can't ride a logged-in
  session), but it should still be tightened to the dashboard's actual origin
  before this server is ever bound beyond `127.0.0.1`, as defense in depth.

## Not yet implemented (tracked against the plan)

- Per-tenant/org isolation for `GraphStore` and docbrain-store queries (Phase 2/3).
- Key *distribution/rotation* tooling for the API-key auth that did ship (see
  above) — today `agentops-security::api_key::generate_api_key` is a library
  call an operator runs manually; there's no CLI command, storage, or rotation
  workflow yet. Fine for a single hosted deployment managing its own env vars,
  not sufficient for multi-tenant self-serve key issuance (Phase 3).
- GitHub repo-access credential custody (per-repo SSH keypairs encrypted at rest,
  GitHub App as the preferred path) — Phase 3.
- Independent security review of the redaction gate and zero-egress invariant, before
  any real client codebase touches this tool.
