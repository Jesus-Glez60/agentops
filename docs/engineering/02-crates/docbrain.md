# Crates: docbrain

The four crates that ingest and search third-party library documentation: store, ingestion, MCP adapter, REST adapter. Read this before changing scraping, chunking or doc search; behavior is explained in [08-docbrain.md](../08-docbrain.md).

Index and field definitions: [02-crate-reference.md](../02-crate-reference.md).

## docbrain-graph

- **Path**: `docbrain-core/crates/docbrain-graph`
- **Purpose**: Entities of the documentation graph and the `DocbrainStore` port, with the SQLite adapter `SqliteDocbrainStore` (`src/sqlite.rs`).
- **Entry points**:
  - `DocbrainStore` trait (20 methods). Entities: `Library`, `DocNode` ("neuron": a token-bounded chunk), `DocEdge` ("synapse"), `ChangelogEntry`, `Job`, `RepoLibraryUsage`, `SearchHit`.
  - `NodeKind`: `Prose`, `CodeExample`. `EdgeRelation`: `Sequence`, `ParentSection`, `CrossReference`, `HasExample`. `JobStatus`: `Running`, `Succeeded`, `Failed`.
  - `content_hash`, `UpsertOutcome`.
- **Tables**: `libraries`, `doc_snapshots`, `doc_nodes`, `doc_edges`, `doc_node_vectors`, `changelog_versions`, `jobs`, `repo_library_versions`.
- **Depends on**: `agentops-embeddings`, `agentops-sqlite-support`.
- **Used by**: 10 crates, including every docbrain crate, `mcp`, `heavy-api`, `heavy-embeddings` and both binaries.
- **Invariants**:
  - Edges let retrieval walk to directly connected context instead of returning isolated top-k chunks (module doc).
  - Single-tenant by design. Multi-tenancy is done outside the trait, with one SQLite file per tenant (`agentops-heavy-api`). Decision "Multi-tenant docbrain isolation via one SQLite file per tenant, not a TenantContext".
- **Tests**: 13 (`src/sqlite.rs`).

## docbrain-ingest

- **Path**: `docbrain-core/crates/docbrain-ingest`
- **Purpose**: Use cases: library discovery, doc scraping, chunking, embedding, retrieval and changelog sync.
- **Entry points**:
  - Discovery (`src/discover.rs`): `LibraryDiscovery` port, `RegistryDiscovery` (npm/PyPI metadata first, GitHub search as fallback), `DiscoveredLibrary`, `Ecosystem` (`Npm`, `PyPi`, `Cargo`, `Go`, `NuGet`).
  - Classification (`src/classify.rs`): `classify_dependency`, deciding whether an import is a real third-party package and which registry it belongs to.
  - Scraping (`src/scrape.rs`): `scrape_docs(docs_url, max_pages, firecrawl_api_key)`. It prefers a site's own Markdown export (`llms.txt`, content negotiation) over HTML link-scraping. Optional Firecrawl fallback for JS-rendered pages.
  - Chunking (`src/chunk.rs`): `chunk_and_store`, `MAX_CHUNK_TOKENS = 500`; dedup plus synapse-edge construction.
  - `scrape_chunk_and_store`: the full ingest for one library version.
  - Retrieval (`src/retrieve.rs`): `get_docs`, `search_docs`, `get_code_examples`: token-budgeted, edge-expanded reads.
  - Changelogs (`src/changelog.rs`): `ChangelogSync` port, `GitHubReleasesSync`.
- **Depends on**: `agentops-embeddings`, `docbrain-graph`.
- **Used by**: `cli`, `mcp` (`sync_docs.rs`), `docbrain-mcp`.
- **Invariants**:
  - Explicitly outside the scanner's zero-network rule: fetching from registries, GitHub and doc sites is this crate's job (module doc).
  - Retrieval logic stays here, not in `docbrain-mcp`'s handlers (`src/retrieve.rs` module doc).
- **Decision**: "docbrain's shallow crawl now prefers a site's llms.txt over blind HTML link-scraping -- including following a meta-index one hop deeper". Firecrawl fallback added in `c2ef9a8`.
- **Known history**: gotchas 54239 (unbounded topic length), 54002 (releases pagination 422), 53990 (doc versions vs changelog versions rarely pair).
- **Tests**: 61 (`src/scrape.rs` 17, `src/chunk.rs` 12, `src/discover.rs` 11, `src/classify.rs` 8, `src/changelog.rs` 7, `src/retrieve.rs` 6).

## docbrain-mcp

- **Path**: `docbrain-core/crates/docbrain-mcp`
- **Purpose**: Inbound adapter: docbrain's tool table and a stdio MCP server. Every tool goes through the `DocbrainStore` port.
- **Entry points**: `list_tools()`, `call_tool(store, db_path, name, args)`, `run_stdio`, `handle_message`, `default_db_path()`, plus its own JSON-RPC types.
- **Tools**: 12; see `survey/mcp-inventory.md`.
- **Depends on**: `agentops-manifest`, `docbrain-graph`, `docbrain-ingest`.
- **Used by**: `cli` (`docbrain-serve`), `heavy-api`, `mcp-server`, `server`, `docbrain-api`.
- **Invariants**:
  - `default_db_path()` is `AGENTOPS_DOCBRAIN_DB`, else `<AGENTOPS_DATA_DIR>/docbrain.db`. It is the single-tenant, CLI-facing store, distinct from hosted `docbrain-tenants/` (`DOCBRAIN_DB_DIR`).
  - No access mode, and no MCP annotations on its tools (`findings.md` F7).
  - `scrape_library` reads an undeclared `_firecrawl_api_key` argument. Hosted `/mcp` fills it from the tenant's integrations vault (`agentops-heavy-api/src/mcp_http.rs`).
  - Wire types are hand-rolled and deliberately duplicated with `agentops-mcp`, not shared (`agentops-mcp/src/protocol.rs` module doc).
  - `scrape_library` can run as a background job (`background`), tracked in `jobs` and polled with `get_job_status`.
- **Tests**: 8 (`src/tools.rs` 5, `src/server.rs` 3).

> Unverified: the doc comment on `default_db_path` names "this crate's own binary" and a `docbrain-serve-api` command. The crate has no `[[bin]]` and the CLI has no such command; see `findings.md` F22.

## docbrain-api

- **Path**: `docbrain-core/crates/docbrain-api`
- **Purpose**: Inbound adapter: REST endpoints wrapping `docbrain-mcp`'s dispatch. One implementation per tool, two transports.
- **Entry points**: `build_router`, `build_router_without_health`, `build_router_without_health_and_tools`, `run`.
- **Depends on**: `agentops-security`, `docbrain-graph`, `docbrain-mcp`.
- **Used by**: `server` (mounted at `/docbrain`).
- **Invariants**:
  - `SqliteDocbrainStore` is not `Sync`, so it sits behind `Arc<Mutex<_>>` and every handler holds the lock for one call (module doc).
  - Mounted in `agentops-server` with one global store, not per tenant (gotcha 30341, "docbrain_router mounted with one global, non-tenant store on the hosted multi-tenant server"). Tenant-scoped library routes live in `agentops-heavy-api/src/libraries_http.rs`.
- **Tests**: 10 (`src/lib.rs`).
