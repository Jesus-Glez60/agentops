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

## Not yet implemented (tracked against the plan)

- Per-tenant/org isolation for `GraphStore` and docbrain-store queries (Phase 2/3).
- `AccessMode::Advisor` structural enforcement (write-capable MCP tools genuinely not
  registered, not just refused at call time) — Phase 1 scope, not yet built in this
  skeleton.
- GitHub repo-access credential custody (per-repo SSH keypairs encrypted at rest,
  GitHub App as the preferred path) — Phase 3.
- Independent security review of the redaction gate and zero-egress invariant, before
  any real client codebase touches this tool.
