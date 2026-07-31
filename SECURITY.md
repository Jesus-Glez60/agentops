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
- **Dependency graph (`DependsOn` edges, implemented)** — the scanner already
  computed a file-to-file dependency graph in memory to power PageRank
  ranking, then discarded it; `agentops_scanner::resolve_dependency_edges`
  exposes exactly that resolution as data, and `agentops install`/the
  `scan_repo` MCP tool now persist it as real `DependsOn` edges, queryable
  via the new `get_dependencies` tool. Fixing this surfaced a real
  correctness bug worth documenting: `agentops-mcp`'s `scan_repo` tool had
  quietly drifted from `agentops-cli`'s `install` command into two separate
  implementations of "scan and persist" — `install` got upsert/prune
  support (fixing a duplicate-node-on-rescan bug), but `scan_repo` kept the
  old `add_node`-only behavior, meaning the actual primary way this product
  gets used (an agent calling `scan_repo` via MCP mid-session, not a human
  running the CLI) still silently duplicated every node on every rescan.
  Consolidated into one shared implementation (`agentops_mcp::scan_and_persist`)
  both callers use, specifically so this class of drift can't happen again
  by construction. Covered by a regression test that rescans through the
  real JSON-RPC dispatch path twice and asserts the symbol count doesn't
  double — a library-level test wouldn't have caught the original bug,
  since the divergence was between two call sites, not inside either
  implementation alone.
- **Direct symbol lookup (`get_symbol`/`ast_search`, implemented)** — two
  read-only MCP tools added alongside `get_dependencies`, both always
  available in `AccessMode::Advisor` and `Full` (added to `READ_ONLY_TOOLS`).
  `get_symbol` does an exact-name match against `Symbol` nodes and returns
  the full stored source; `ast_search` does a case-insensitive substring
  match on symbol names and returns name+location only, so an agent can
  narrow down to the right symbol before fetching its full source with
  `get_symbol`. Known limitation, documented rather than silently absent:
  neither tool filters by symbol kind (`function` vs `struct` vs ...) —
  the graph only persists the coarse `NodeKind::Symbol`, not the
  fine-grained kind, so that filter isn't available yet. Both tools return
  a tool-level error (not a panic or empty success) when nothing matches.
  Verified live: rebuilt `agentops-cli`, ran the real compiled binary via
  actual stdin/stdout JSON-RPC against `agentops-heavy/crates/agentops-license`
  as a target repo — `scan_repo`, `get_symbol` (exact match, real source
  returned), `ast_search` (case-insensitive substring, matched 9 real
  symbols, excluded non-matches), and `get_dependencies` (correctly reported
  none, since this target repo's imports are Rust `use` paths, not the
  relative-path style the resolver supports) all behaved correctly over the
  real protocol, not just in unit tests.
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

- **Docbrain ingestion engine — `scrape_library`/`sync_changelogs`/
  `resolve_library`/`ingest_local_files` (implemented)** — this is the piece
  that actually closes the gap between `discover_library` (which only finds
  a docs/repo URL) and real, queryable `doc_nodes` content; before this,
  nothing populated doc content at all. All four join `docbrain-ingest`'s
  existing zero-trust-of-the-network posture: this crate is explicitly
  **not** under the zero-egress invariant `agentops-scanner` enforces (its
  entire job is fetching from the network), so the controls here are about
  handling untrusted response bodies safely, not about avoiding the network
  call itself.
  - `scrape_library` (`docbrain_ingest::scrape_docs`) fetches a docs page via
    `ureq` and parses it with `scraper`/`html5ever` — a real HTML parser, not
    a regex-based scraper, so malformed/adversarial markup on a third-party
    docs site can't corrupt extraction the way ad hoc string scraping could.
    The shallow-crawl option (`max_pages`) is bounded and same-origin +
    same-path-prefix only (`same_scope_links`), so a single tool call can
    never turn into an unbounded spider even against a docs site with
    thousands of internal links — verified via a live scrape of a real
    docs.rs page (`docs_page_end_to_end`/`scrape_library_persists_real_content...`
    tests), not a mocked HTML fixture only.
  - `sync_changelogs` hits GitHub's public releases API — read-only, no
    token required, so no credential to leak. `parse_owner_repo` only
    extracts an `owner/repo` pair from a URL already stored on a library the
    tenant is authorized to see (`authorized_library_id`'s guarantee) and
    only ever calls out to GitHub, never an arbitrary caller-supplied host.
  - `ingest_local_files` reads from the **local filesystem**, not the
    network — the caller-supplied `paths` are read directly via
    `std::fs::read_to_string`. This tool is meant for a trusted local caller
    (an agent or CLI user with legitimate filesystem access on the machine
    running docbrain-mcp/docbrain-api), the same trust boundary
    `agentops-scanner` already operates under for repo scanning — it is not
    exposed with any additional path-traversal restriction beyond normal
    filesystem permissions, and should not be exposed to an untrusted remote
    caller without adding one (not a concern yet since `docbrain-api`
    doesn't expose per-tool argument validation beyond what each tool does
    internally, and every tool call already requires whatever
    `DOCBRAIN_API_KEY_HASH` auth is configured).
  - All four went through the same tenant-scoping discipline as the rest of
    `docbrain-graph`: every write routes through `authorized_library_id`, so
    e.g. `scrape_library` can't be pointed at a library the caller can't
    already see, and `ingest_local_files` registers a *new* library as
    private to the caller's own org (or public) exactly like
    `discover_library` already does — no new bypass of Docbrain-4's
    isolation guarantee was introduced by adding a second way to create a
    library.
  - Known scope cut, stated plainly: none of these run as background jobs.
    A `scrape_library`/`sync_changelogs` call blocks until the fetch (and,
    for a multi-page scrape, all follow-up fetches) completes — there is no
    `get_job_status` tool and no job queue. This is fine for a single docs
    page or a shallow few-page crawl (seconds), not for a large multi-page
    ingestion running in the background while an agent keeps working; that
    would need a real async job table (`jobs` in `docbrain-graph` + a
    background thread/task) and hasn't been built.

- **Semantic search over docbrain content — `search_docs`/`index_docs`
  (implemented, `agentops-heavy-mcp`)** — the paid-tier semantic layer now
  covers docbrain, not just codebrain. `agentops_embeddings::collect_doc_index_items`
  reads real `DocNode`s out of a `DocbrainStore` (a cross-workspace path
  dependency from `agentops-heavy` into the light-tier `docbrain-graph`
  crate, same pattern already established for `agentops-graph`) and shares
  the *same* Qdrant collection and BGE-M3/reranker model instance as the
  existing codebrain index, rather than standing up a second ~2GB model
  load in the same process. Two things had to be gotten right for that
  sharing to be safe, both covered by live tests against a real Qdrant
  instance:
  - **Id-space collision.** `DocNode.id` and `agentops-graph` node `id` are
    independent autoincrement sequences from two unrelated SQLite databases
    — nothing stops them from producing the same number, and both get cast
    to `u64` as the Qdrant point id. Without namespacing, indexing a
    docbrain doc and a codebrain symbol that happened to share an id would
    silently overwrite one with the other. Fixed by reserving the top bit
    of the `u64` id space for docbrain items (`DOC_ID_NAMESPACE_BIT`) —
    real source ids are always non-negative `i64`s, so the two ranges
    provably never overlap. Covered by a dedicated unit test plus a live
    round-trip test that indexes both kinds and confirms no data loss.
  - **Result cross-contamination.** A docbrain query must never surface a
    codebrain symbol, or vice versa, even when their content is
    semantically similar (the live test uses a symbol and a doc section
    that describe literally the same thing on purpose, to make sure a
    naive shared-corpus search would fail this). Fixed with a `kind` field
    on every indexed point (`"doc"` vs `"symbol"/"gotcha"/"decision"`) and
    an optional `kind` filter threaded through `SemanticIndex::search_scoped`/
    `vector_search_scoped` — `search`/`vector_search` stay as unfiltered
    convenience wrappers so no existing caller's behavior changed.
  - Same license gate as the rest of `agentops-heavy-mcp`: this binary
    refuses to start at all without `AGENTOPS_LICENSE_KEY`, so `search_docs`/
    `index_docs` are unavailable exactly like `semantic_search`/`semantic_index`
    without one — no separate gate was needed since the whole process is
    already gated.
  - Not yet exposed over `agentops-heavy-api` (REST) — only the MCP stdio
    server has these two tools today. `agentops-heavy-api`'s existing
    `/search/index`/`/search` endpoints are codebrain-only; a docbrain
    equivalent for the dashboard hasn't been added.

- **API-key CLI tooling (implemented)** — `agentops api-key generate` (in
  `agentops-cli`) wraps `agentops_security::api_key::generate_api_key`,
  printing the raw key once and the hash to configure. Closes the "manual
  library call only" gap noted in an earlier revision of this doc. Still not
  sufficient for multi-tenant self-serve key issuance/rotation at scale —
  that's a hosted-dashboard feature, not a CLI one, and isn't built.
- **`agentops-heavy/crates/agentops-repo-access` (implemented, both repo-access
  paths now have code)**:
  - *SSH deploy keys* — per-repo Ed25519 keypairs, encrypted (OpenSSH's own
    bcrypt-pbkdf+AES-256-CTR) before ever leaving `generate_deploy_keypair`;
    decrypted only into a `0600` temp file for the lifetime of one clone/fetch
    subprocess, zeroed and deleted on `Drop` (including panic unwind). SSH
    host key pinned against `GITHUB_KNOWN_HOSTS`, fetched live from
    `https://api.github.com/meta` rather than hand-typed, and verified live
    against the real `github.com` SSH endpoint (host-key verification passed
    end to end) — not trust-on-first-use. Round-tripped through the system's
    real `ssh-keygen` as an independent oracle to confirm the OpenSSH
    encoding is genuinely correct.
  - *Passphrase sourcing* — no longer a bare caller-supplied parameter.
    `secrets::SecretsProvider` is now the policy boundary; every passphrase
    is derived via `HMAC-SHA256(master_key, tenant || repo_id)`, scoped so a
    derivation for one repo doesn't help with any other repo. The shipped
    `EnvSecretsProvider` is real and tested — genuinely correct for a single
    self-hosted deployment — but the master key sits in that deployment's
    process env, so it is **not sufficient for a multi-tenant hosted
    product**: a KMS/Vault-backed `SecretsProvider` implementation, where the
    master key never leaves a hardware/cloud boundary, is still the real
    prerequisite before any actual paying customer's deploy key is generated.
    That implementation isn't written.
  - *GitHub App (`agentops-github-app`, implemented)* — the plan's
    *recommended primary* path (avoids private-key custody on our side
    entirely; SSH deploy keys are the documented fallback/self-hosted path).
    RS256 App-JWT signing and installation-token exchange are implemented.
    JWT signing verified two independent ways against a real generated RSA
    keypair (`jsonwebtoken`'s own decode, and a from-scratch signature check
    via the `rsa` crate's PKCS1v15 verifier). The installation-token HTTP
    exchange is verified against a real HTTP transaction (`wiremock`)
    matching GitHub's documented shape — **not against GitHub's live API**,
    since no App has actually been registered on github.com yet. That
    registration is a manual, external step only a human can do; until it
    happens, this path is code-complete but operationally unverified.
  - *Connection storage + API (`store.rs`, `agentops-heavy-api`, implemented)*
    — SQLite store scoped by `(tenant, id)` at the schema's primary-key level
    (composite key, not an app-level filter someone could forget), same
    pattern as `docbrain-graph`'s `TenantContext`. `agentops-heavy-api`
    wires it to a REST surface (`POST /repos/connect`, `GET /repos`,
    `POST /repos/{id}/verify`, `GET /repos/github-app/install-url`) behind
    the same API-key middleware as `agentops-api`/`docbrain-api`. Response
    DTOs are hand-built, never a direct serialize of the store row, so the
    encrypted private key can't leak into a response even by future
    accident — verified by an explicit test asserting no response body ever
    contains key material, plus a live end-to-end curl run (connect → list
    → verify against a real refused connection, correctly recorded as
    `failed` with the actual git/ssh error text) and a real dashboard page
    (`apps/web/src/app/repos/connect`) exercised against a running instance.
  - **Still open**: the dashboard has no GitHub-App-installation *callback*
    handling (recording which installation ID belongs to which tenant once
    someone completes the install flow) — today the dashboard can only send
    someone to the install URL, not react to their coming back. Real repo
    sync/ingestion using either credential type (this is credential
    *custody*, not a clone-and-index pipeline) is separate, unbuilt work.
- **`agentops-heavy/crates/agentops-embeddings` (implemented)** — semantic
  search over the neuron graph, BGE-M3 dense embeddings generated locally
  via ONNX (`fastembed`) and indexed into Qdrant. Deliberately no Python
  runtime and no external embedding API — repo content never leaves the
  process to get embedded, unlike a hosted-embedding-API design would
  require. This crate is heavy-tier-only and does make real network calls
  (downloading the ~2GB BGE-M3 model from Hugging Face Hub on first use,
  then talking to Qdrant) — that's fine under the heavy tier's threat model
  (it already talks to Postgres/GitHub/etc.) but is explicitly **not**
  subject to the light-tier scanner's zero-runtime-network-egress
  invariant; don't assume that invariant extends here. Verified live
  against a real Qdrant instance and the real downloaded model: a query
  with zero keyword overlap with its target text still ranks the
  semantically related item first, both in a synthetic test (SSH-security
  query vs. an unrelated bread recipe) and against this repo's own real
  graph (a business-language query — "is it safe to generate a real
  customer's deploy key today" — correctly surfaced the exact KMS-gap
  decision node recorded earlier).
  Now wired to `agentops-heavy-api`'s `POST /search/index` /
  `GET /search`, gated behind a valid license (`agentops_license::require_valid_license_from_env`,
  reading `AGENTOPS_LICENSE_KEY`) — semantic search is a paid-tier feature,
  and a missing/invalid license or unset `AGENTOPS_QDRANT_URL` disables the
  routes (`402 Payment Required`) rather than the server refusing to start.
  Fixing the endpoint surfaced a real, subtle bug worth documenting: a
  handler that opened a `SqliteGraphStore` and then `.await`ed a Qdrant call
  produced a `!Send` future — `&dyn GraphStore` isn't provably `Sync`
  (`rusqlite::Connection` is intentionally `!Sync` upstream), and `&T: Send`
  requires `T: Sync`. The trait itself can't require `Sync` without breaking
  `SqliteGraphStore`, so the real fix is structural: never hold a
  `&dyn GraphStore` across an `.await` — `collect_index_items` (sync) and
  `SemanticIndex::index_items` (async) are deliberately separate functions
  to make that mistake hard to reintroduce. Verified with a regression test
  that goes through the real HTTP router (a library-level test wouldn't
  have caught this — the bug only manifests via axum's `Handler` trait
  bound).
  Also now exposed as a real MCP server, `agentops-heavy-mcp`
  (`semantic_search`/`semantic_index` tools, stdio JSON-RPC, same hand-rolled
  protocol as `agentops-mcp`) — this is the piece that actually matters for
  agent use in Claude Code, as opposed to the REST endpoint (dashboard) or
  the CLI example. No `AccessMode`/advisor-mode split here, unlike
  `agentops-mcp`: every tool is read/index-only, so there's no write
  capability to structurally gate — the gate that applies is licensing, and
  the binary refuses to start at all without a valid one (an MCP server
  with zero tools isn't useful to hand an agent, so "start degraded" doesn't
  make sense here the way it does for the REST server, which has other,
  unrelated routes worth keeping up). Verified for real: signed a real
  license with the offline production key, ran the actual compiled binary,
  and drove it over real stdin/stdout with the exact JSON-RPC framing an
  MCP client sends — `initialize`, `tools/list`, `semantic_index`, then
  `semantic_search` with a query that shares no words with its target text,
  correctly ranked first.
  Reranking added: `search()` is now a real two-stage pipeline — a wide
  embedding-based recall pass (`vector_search`, still available standalone)
  narrowed down by a `bge-reranker-v2-m3` cross-encoder pass (via
  `fastembed`'s `TextRerank`, the same model the original codebrain/docbrain
  used, there via `sentence_transformers.CrossEncoder`). A cross-encoder
  scores the query and a candidate document together in one forward pass
  rather than comparing independently-computed vectors, which is why it's
  slower but more accurate — too slow to run over a whole corpus, which is
  the actual reason this is two stages and not just a better single model.
  Verified live that reranking isn't a silent no-op: a query's reranked
  score is provably not the same number as its raw cosine-similarity score
  (different scale entirely — cosine is bounded, the cross-encoder's isn't),
  confirming the second stage genuinely ran and reordered/rescored results
  rather than passing the first stage through unchanged.
- Independent security review of the redaction gate and zero-egress invariant, before
  any real client codebase touches this tool.
