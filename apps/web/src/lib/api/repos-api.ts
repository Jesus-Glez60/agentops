// Typed client for the Repositories screen -- same /api/heavy/* proxy
// pattern as team-api.ts/profile-api.ts (see profile-api.ts's doc comment
// for why: session token must stay server-side).

async function heavyFetch<T>(path: string, init: RequestInit = {}): Promise<T> {
  const res = await fetch(`/api/heavy${path}`, {
    ...init,
    headers: { ...(init.body ? { "Content-Type": "application/json" } : {}), ...init.headers },
    cache: "no-store",
  });
  const data = await res.json().catch(() => null);
  if (!res.ok) {
    const message = data && typeof data === "object" && typeof data.error === "string" ? data.error : `request to ${path} failed with ${res.status}`;
    throw new Error(message);
  }
  return data as T;
}

export const REPOS_SWR_KEY = "/repos";

export interface RepoCounts {
  symbols: number;
  files: number;
  gotchas: number;
  /** The subset of `gotchas` nobody's curated yet (`!curated`). Gotchas are
   * permanent knowledge, never "resolved away" -- this is what drives the
   * "needs curation" stat and health warning, not the total. */
  gotchas_needing_curation: number;
  decisions: number;
}

/** `"pending" | "active" | "failed: <reason>"` -- the backend encodes the failure reason directly into the status string rather than a separate field, see `parseRepoStatus`. */
export interface RepoConnection {
  id: string;
  tenant: string;
  repo_url: string;
  method: string;
  /** Always present for an SSH-method connection -- this is a *public* key meant to be pasted into GitHub, never a secret, so (unlike an API key) it's safe to keep returning on every fetch rather than a reveal-once flow. */
  public_key_openssh: string | null;
  status: string;
  created_at: string;
  /** Dashboard-unification (Initiative 1) additive fields -- present only
   * for an `Active` connection whose store opened; `undefined`/`false`
   * otherwise, same "never fabricated zeros" contract the Rust
   * `RepoSummary.counts` has. There is deliberately no `last_scanned_at`
   * here (unlike the retired manifest-based `RepoSummary`) -- the
   * tenant-scoped backend has no equivalent timestamp to expose yet, so
   * `repoHealth` treats it as unknown/skipped rather than fabricating one. */
  counts?: RepoCounts | null;
  branch?: string | null;
  path_missing?: boolean;
  /** User-selected branch override -- `null`/absent means "no override, index whatever the default branch is." Distinct from `branch` above (the live-read actual checked-out branch): this is the persisted intent. */
  tracked_branch?: string | null;
}

export interface ReposResponse {
  connections: RepoConnection[];
  /** Server-computed from the caller's own capabilities -- whether to even show a "Connect repository" action, so this client never reimplements that logic. */
  can_connect: boolean;
}

export function getRepos(): Promise<ReposResponse> {
  return heavyFetch<ReposResponse>(REPOS_SWR_KEY);
}

export interface ConnectRepoInput {
  repo_id: string;
  repo_url: string;
}

export interface ConnectRepoResponse {
  connection: RepoConnection;
  instructions: string;
}

export function connectRepo(input: ConnectRepoInput): Promise<ConnectRepoResponse> {
  return heavyFetch<ConnectRepoResponse>("/repos/connect", { method: "POST", body: JSON.stringify(input) });
}

export interface VerifyRepoResponse {
  status: "active" | "failed";
  reason?: string;
}

export function verifyRepo(id: string): Promise<VerifyRepoResponse> {
  return heavyFetch<VerifyRepoResponse>(`/repos/${encodeURIComponent(id)}/verify`, { method: "POST" });
}

export type ParsedRepoStatus = { kind: "pending" } | { kind: "active" } | { kind: "failed"; reason: string };

/** The backend's `status` field is `"pending"`, `"active"`, or `"failed: <reason>"` (see `ConnectionView` in agentops-heavy-api) -- this is the one place that string gets parsed into something a component can switch on. */
export function parseRepoStatus(status: string): ParsedRepoStatus {
  if (status === "pending") return { kind: "pending" };
  if (status === "active") return { kind: "active" };
  const reason = status.startsWith("failed: ") ? status.slice("failed: ".length) : status;
  return { kind: "failed", reason };
}

// --- GitHub App install flow -----------------------------------------

export function getGithubAppInstallUrl(): Promise<{ install_url: string }> {
  return heavyFetch("/repos/github-app/install-url");
}

export interface InstallationRepo {
  full_name: string;
  clone_url: string;
  default_branch: string;
  language: string | null;
  size: number;
}

export function getInstallationRepos(installationId: string): Promise<{ repositories: InstallationRepo[] }> {
  return heavyFetch(`/repos/github-app/installations/${encodeURIComponent(installationId)}/repos`);
}

export interface ConnectFromInstallationResponse {
  connections: { connection: RepoConnection; job_id: string | null }[];
}

export function connectFromInstallation(installationId: string, repoFullNames: string[]): Promise<ConnectFromInstallationResponse> {
  return heavyFetch(`/repos/github-app/installations/${encodeURIComponent(installationId)}/connect`, {
    method: "POST",
    body: JSON.stringify({ repo_full_names: repoFullNames }),
  });
}

/** Shared SWR key for "is the GitHub App connected for this tenant" -- read by both the Team & Access manage card and the Profile read-only indicator, and revalidated by both after a successful install. */
export const GITHUB_APP_INSTALLATIONS_SWR_KEY = "/repos/github-app/installations";

export interface GithubAppInstallation {
  id: string;
  account_login: string;
  installed_at: string;
  /** GitHub's own installation-management page -- org vs personal account URL shape resolved server-side (`agentops_github_app::installation_html_url`). */
  manage_url: string;
}

export function getGithubAppInstallations(): Promise<{ installations: GithubAppInstallation[] }> {
  return heavyFetch(GITHUB_APP_INSTALLATIONS_SWR_KEY);
}

// --- Indexing progress --------------------------------------------------

/** The 9 stages in the exact fixed order the backend always creates them in (`STAGE_ORDER` in `indexing_store.rs`) -- mirrored here as display labels for the wizard's progress screen. */
export const INDEXING_STAGE_LABELS: Record<string, string> = {
  connection_verified: "Connection verified",
  repository_cloned: "Repository cloned",
  files_discovered: "Files discovered",
  symbols_extracted: "Symbols extracted",
  dependencies_mapped: "Dependencies mapped",
  knowledge_nodes_created: "Knowledge nodes created",
  embeddings_generated: "Embeddings generated",
  documentation_generated: "Documentation generated",
  index_ready: "Index ready",
};

export interface IndexingStage {
  stage: string;
  seq: number;
  status: "pending" | "active" | "done" | "failed";
  progress_current: number | null;
  progress_total: number | null;
  error: string | null;
  started_at: string | null;
  finished_at: string | null;
}

export interface IndexingJobSummary {
  id: string;
  kind: "initial" | "reindex";
  status: "running" | "succeeded" | "failed";
  current_stage: string | null;
  created_at: string;
  finished_at: string | null;
}

export interface IndexingStatusResponse {
  job: IndexingJobSummary;
  stages: IndexingStage[];
  overall_percent: number;
}

export function startIndexing(connectionId: string, kind?: "initial" | "reindex"): Promise<{ job_id: string }> {
  return heavyFetch(`/repos/${encodeURIComponent(connectionId)}/index`, { method: "POST", body: JSON.stringify(kind ? { kind } : {}) });
}

export function getIndexingStatus(connectionId: string, jobId?: string): Promise<IndexingStatusResponse> {
  const q = jobId ? `?job_id=${encodeURIComponent(jobId)}` : "";
  return heavyFetch(`/repos/${encodeURIComponent(connectionId)}/index/status${q}`);
}

export function retryIndexing(connectionId: string, jobId: string): Promise<{ job_id: string }> {
  return heavyFetch(`/repos/${encodeURIComponent(connectionId)}/index/retry?job_id=${encodeURIComponent(jobId)}`, { method: "POST" });
}

/** SSH-method connections only -- 404s for a GitHub App connection (nothing to regenerate). */
export function regenerateDeployKey(connectionId: string): Promise<{ connection: RepoConnection }> {
  return heavyFetch(`/repos/${encodeURIComponent(connectionId)}/regenerate-key`, { method: "POST" });
}

/** Every branch on the connection's remote -- SSH via `git ls-remote`, GitHub App via GitHub's REST API server-side (see `agentops-heavy-api::indexing::list_branches`). */
export function listBranches(connectionId: string): Promise<string[]> {
  return heavyFetch<{ branches: string[] }>(`/repos/${encodeURIComponent(connectionId)}/branches`).then((r) => r.branches);
}

/** Sets (or, with `null`, clears) the connection's tracked-branch override and immediately spawns a reindex job against it -- same response shape as `startIndexing`. */
export function setBranch(connectionId: string, branch: string | null): Promise<{ job_id: string }> {
  return heavyFetch(`/repos/${encodeURIComponent(connectionId)}/branch`, { method: "PATCH", body: JSON.stringify({ branch }) });
}

// --- Dashboard unification (Initiative 1) ---------------------------------
//
// Migrated off the retired `agentops-api.ts` client (which called
// `AGENTOPS_API_URL` directly from the browser, bypassing this file's
// `/api/heavy/*` proxy and agentops-manifest's local-only scan registry --
// see agentops-heavy-api's `dashboard.rs` module doc comment for the
// backend side of this migration). Every `repo`/`connectionId` string below
// is what the backend returns as `SearchResult.repo`/`NodeDetail.repo`/
// `GotchaSummary.repo`/`ActivityEvent.repo` -- which is always exactly a
// `RepoConnection.id` (the tenant-scoped backend derives it from the
// connection's own checkout directory name, which *is* the connection id --
// see `checkout_path`'s doc comment), so these opaque strings compose
// directly with `RepoConnection.id` with no separate name-resolution step.

export interface ActivityEvent {
  repo: string;
  /** SQLite `CURRENT_TIMESTAMP`-formatted, lexicographically sortable. */
  started_at: string;
  files_added: number;
  files_changed: number;
  files_removed: number;
  symbols_added: number;
  symbols_changed: number;
  symbols_removed: number;
}

export function getActivity(): Promise<ActivityEvent[]> {
  return heavyFetch<{ activity: ActivityEvent[] }>("/activity").then((r) => r.activity);
}

// Rust's `NodeKind` serializes as its bare (PascalCase) variant name, e.g.
// `NodeKind::Symbol` -> `"Symbol"` -- no #[serde(rename_all)] involved.
// `DocSection` (agentops-graph/src/lib.rs) was missing here entirely --
// every `Record<NodeKind, ...>` lookup table below was therefore silently
// incomplete for it, and the graph endpoints do return real DocSection
// nodes (`subgraph.rs` seeds an "overview" DocSection unconditionally).
// Indexing an incomplete Record with an unlisted key returns `undefined`
// at runtime with no compile-time warning (the object literal type-checked
// fine against the old, incomplete union) -- confirmed live as the actual
// cause of the /graph page's "element type is invalid" crash: `GraphNode`
// used the missing key's `undefined` result directly as a JSX component.
export type NodeKind = "Symbol" | "File" | "Gotcha" | "Decision" | "Definition" | "Note" | "DocSection";

// Same bare-variant-name convention as NodeKind. Curation only ever
// reorders a gotcha's prominence -- there is no "closed"/hidden state.
export type NodeProminence = "Full" | "Reduced";

export interface SearchResult {
  repo: string;
  id: number;
  kind: NodeKind;
  name: string | null;
  path: string | null;
  container: string | null;
  start_line: number | null;
  end_line: number | null;
  /** A truncated preview of the node's content -- the detail panel fetches the full `content` separately via `getNodeDetail`. */
  snippet: string | null;
  /** 0–1, derived from `search_similar`'s distance -- see `search.rs`'s doc comment for the exact formula. */
  similarity: number;
  curated: boolean;
  prominence: NodeProminence;
  curation_reason: string | null;
}

export interface ConnectedNode {
  id: number;
  kind: NodeKind;
  name: string | null;
  path: string | null;
  /** e.g. `"affects"` for an outgoing edge, `"← affects"` for an incoming one. */
  relation: string;
}

export interface NodeDetail {
  id: number;
  kind: NodeKind;
  repo: string;
  path: string | null;
  name: string | null;
  container: string | null;
  start_line: number | null;
  end_line: number | null;
  content: string | null;
  connected: ConnectedNode[];
  curated: boolean;
  prominence: NodeProminence;
  curation_reason: string | null;
}

export interface SearchOptions {
  /** Connection ids/URLs to search; omitted/empty means every one of the caller's own connections. */
  repos?: string[];
  /** Kind filter; omitted/empty means all kinds. */
  kinds?: NodeKind[];
  topK?: number;
}

export function search(query: string, options: SearchOptions = {}): Promise<SearchResult[]> {
  const params = new URLSearchParams({ q: query });
  if (options.repos?.length) params.set("repos", options.repos.join(","));
  if (options.kinds?.length) params.set("kind", options.kinds.map((k) => k.toLowerCase()).join(","));
  if (options.topK) params.set("top_k", String(options.topK));
  return heavyFetch<{ results: SearchResult[] }>(`/local-search?${params.toString()}`).then((r) => r.results);
}

export function getNodeDetail(connectionId: string, id: number): Promise<NodeDetail> {
  return heavyFetch<NodeDetail>(`/repos/${encodeURIComponent(connectionId)}/nodes/${id}`);
}

/** Shared SWR key for the gotchas page's list -- one cache entry, revalidated the same optimistic-mutate way REPOS_SWR_KEY already is. */
export const GOTCHAS_SWR_KEY = "/gotchas";

export interface GotchaSummary {
  repo: string;
  id: number;
  name: string | null;
  path: string | null;
  container: string | null;
  start_line: number | null;
  end_line: number | null;
  snippet: string | null;
  curated: boolean;
  prominence: NodeProminence;
  curation_reason: string | null;
}

/** `needs_curation` (nobody's looked at it), `kept` (curated, still Full prominence), or `reduced` (curated down, always paired with a reason). */
export type GotchaBucket = "needs_curation" | "kept" | "reduced";

export function getGotchas(bucket?: GotchaBucket): Promise<GotchaSummary[]> {
  const params = bucket ? `?bucket=${bucket}` : "";
  return heavyFetch<{ gotchas: GotchaSummary[] }>(`/gotchas${params}`).then((r) => r.gotchas);
}

/** `reason` is required (non-empty) when `prominence` is `"Reduced"`, ignored otherwise -- enforced server-side too. */
export function setCuration(connectionId: string, id: number, prominence: NodeProminence, reason: string | null): Promise<{ id: number; prominence: NodeProminence }> {
  return heavyFetch(`/repos/${encodeURIComponent(connectionId)}/nodes/${id}/curation`, {
    method: "POST",
    body: JSON.stringify({ prominence: prominence.toLowerCase(), reason }),
  });
}

/**
 * The Knowledge Graph screen's 4 tabs, mapped onto real `EdgeRelation`s
 * (there's no `Calls`/`Contains` edge in the graph) -- see
 * `agentops-api/src/subgraph.rs`'s `mode_filter`:
 * `local` = DependsOn+Documents+Affects both directions, `dep_chain` =
 * DependsOn outgoing only, `impact` = DependsOn incoming only, `knowledge`
 * = Affects both directions.
 */
export type GraphMode = "local" | "dep_chain" | "impact" | "knowledge";

export interface SubgraphNode {
  id: number;
  kind: NodeKind;
  name: string | null;
  path: string | null;
  curated: boolean;
  prominence: NodeProminence;
  /** BFS distance from the seed node; 0 for the seed itself. */
  depth: number;
}

export interface SubgraphEdge {
  id: number;
  src_id: number;
  dst_id: number;
  relation: "DependsOn" | "Documents" | "Affects" | "References";
  /** "depends on" / "documents" / "affects" / "references" -- see search.rs's relation_label. No "← " prefix: direction is conveyed structurally via src_id/dst_id. */
  label: string;
}

export interface SubgraphResponse {
  seed_id: number;
  mode: GraphMode;
  depth: number;
  nodes: SubgraphNode[];
  edges: SubgraphEdge[];
  /** `true` if the backend's NODE_CAP (150) was hit -- the response is a partial subgraph, not the full neighborhood. */
  truncated: boolean;
}

export interface SubgraphOptions {
  /** 1-4; server defaults to 2 and clamps regardless of what's sent. */
  depth?: number;
  /** Empty/omitted = no filter. */
  kinds?: NodeKind[];
}

export function getSubgraph(connectionId: string, id: number, mode: GraphMode, options: SubgraphOptions = {}): Promise<SubgraphResponse> {
  const params = new URLSearchParams({ mode });
  if (options.depth) params.set("depth", String(options.depth));
  if (options.kinds?.length) params.set("kind", options.kinds.map((k) => k.toLowerCase()).join(","));
  return heavyFetch<SubgraphResponse>(`/repos/${encodeURIComponent(connectionId)}/nodes/${id}/graph?${params.toString()}`);
}

/** Every node/edge in a repo, not centered on any seed -- the Knowledge Graph screen's "pick a repo" entry point. `SubgraphNode.depth` is always 0 here (there's no BFS distance without a seed). */
export interface RepoGraphResponse {
  repo: string;
  nodes: SubgraphNode[];
  edges: SubgraphEdge[];
  /** `true` if the backend's NODE_CAP (150) was hit -- expect this often for large repos unless `kinds` narrows the result. */
  truncated: boolean;
}

export function getRepoGraph(connectionId: string, kinds: NodeKind[] = []): Promise<RepoGraphResponse> {
  const params = new URLSearchParams();
  if (kinds.length) params.set("kind", kinds.map((k) => k.toLowerCase()).join(","));
  const qs = params.toString();
  return heavyFetch<RepoGraphResponse>(`/repos/${encodeURIComponent(connectionId)}/graph${qs ? `?${qs}` : ""}`);
}

// Documentation Viewer types mirror `agentops-docgen::model` exactly (see
// that crate's `model.rs` for the Rust source of truth). `DocPage` is
// `Serialize`-only in Rust -- these types are never sent back to the
// server, only read from `GET /repos/{id}/docs`'s response.

/** `#[serde(rename_all = "snake_case")]` on `DocGroup` -- no `execution_flows` variant exists yet (see that enum's own doc comment: no signal in the graph derives a call-chain "flow"). */
export type DocGroup = "repository" | "core_modules" | "knowledge" | "setup";

export interface SymbolRow {
  name: string;
  /** From a `Documents`-edge-connected `Definition` node's first line, when one exists (`explain_symbol`'s opt-in output) -- empty string otherwise, never fabricated. */
  one_liner: string;
  gotcha_count: number;
  node_id: number;
}

/** `#[serde(tag = "block_type", rename_all = "snake_case")]` -- discriminated union, switch on `block_type`. */
export type DocBlock =
  | { block_type: "prose"; markdown: string }
  | { block_type: "symbol_table"; file: string; rows: SymbolRow[] }
  | { block_type: "dependency_chips"; deps: string[] }
  | {
      block_type: "knowledge_callout";
      kind: NodeKind;
      node_id: number;
      title: string;
      body: string;
      /** e.g. "affects refreshSession()" -- empty string if the note has no Affects edge yet. */
      affects: string;
      /** `[path, line]` of the affected symbol/file, when known. */
      source: [string, number] | null;
    };

export interface DocSection {
  /** Stable slug -- nav href + TOC anchor. */
  id: string;
  group: DocGroup;
  title: string;
  blocks: DocBlock[];
}

export interface DocPage {
  repo: string;
  /** `ScanHistory.started_at` text, same lexicographically-sortable format as `ActivityEvent.started_at`. */
  generated_at: string;
  node_count: number;
  sections: DocSection[];
}

/** Per-repo key -- unlike `REPOS_SWR_KEY`/`GOTCHAS_SWR_KEY` (one global list each), docs are scoped per repo, so callers compose `${DOCS_SWR_KEY}/${repo}` as the actual SWR key. */
export const DOCS_SWR_KEY = "/docs";

export function getDocs(connectionId: string): Promise<DocPage> {
  return heavyFetch<DocPage>(`/repos/${encodeURIComponent(connectionId)}/docs`);
}
