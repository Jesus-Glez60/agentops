// Typed client for agentops-api's dashboard + search routes. Types mirror
// agentops-core's agentops-api/src/{repos,search}.rs response shapes
// exactly (RepoSummary/RepoCounts/ActivityEvent/SearchResult/NodeDetail);
// see those files for the Rust source of truth.
import { apiFetch } from "@/lib/api/fetcher";

const AGENTOPS_API_URL = process.env.NEXT_PUBLIC_AGENTOPS_API_URL ?? "http://127.0.0.1:8420";

/** Shared SWR key so the Overview page's repo table and the topbar's pill/dot subscribe to one cache entry, not two independent fetches. */
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

export interface RepoSummary {
  name: string;
  path: string;
  branch: string | null;
  /** Unix seconds -- matches `agentops-manifest`'s own `ManifestEntry.last_scanned_at`. */
  last_scanned_at: number;
  /** `null` when the repo has never actually been scanned -- never fabricated zeros. */
  counts: RepoCounts | null;
  /** `true` when the manifest's recorded path no longer exists on disk. */
  path_missing: boolean;
}

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

export async function getRepos(): Promise<RepoSummary[]> {
  const { repos } = await apiFetch<{ repos: RepoSummary[] }>(AGENTOPS_API_URL, "/scans");
  return repos;
}

export async function rescanRepo(name: string): Promise<RepoSummary> {
  return apiFetch<RepoSummary>(AGENTOPS_API_URL, `/repos/${encodeURIComponent(name)}/rescan`, { method: "POST" });
}

export async function getActivity(): Promise<ActivityEvent[]> {
  const { activity } = await apiFetch<{ activity: ActivityEvent[] }>(AGENTOPS_API_URL, "/activity");
  return activity;
}

// Rust's `NodeKind` serializes as its bare (PascalCase) variant name, e.g.
// `NodeKind::Symbol` -> `"Symbol"` -- no #[serde(rename_all)] involved.
export type NodeKind = "Symbol" | "File" | "Gotcha" | "Decision" | "Definition" | "Note";

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
  /** Repo names to search; omitted/empty means all scanned repos. */
  repos?: string[];
  /** Kind filter; omitted/empty means all kinds. */
  kinds?: NodeKind[];
  topK?: number;
}

export async function search(query: string, options: SearchOptions = {}): Promise<SearchResult[]> {
  const params = new URLSearchParams({ q: query });
  if (options.repos?.length) params.set("repos", options.repos.join(","));
  if (options.kinds?.length) params.set("kind", options.kinds.map((k) => k.toLowerCase()).join(","));
  if (options.topK) params.set("top_k", String(options.topK));

  const { results } = await apiFetch<{ results: SearchResult[] }>(AGENTOPS_API_URL, `/local-search?${params.toString()}`);
  return results;
}

export async function getNodeDetail(repo: string, id: number): Promise<NodeDetail> {
  return apiFetch<NodeDetail>(AGENTOPS_API_URL, `/repos/${encodeURIComponent(repo)}/nodes/${id}`);
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

export async function getGotchas(bucket?: GotchaBucket): Promise<GotchaSummary[]> {
  const params = bucket ? `?bucket=${bucket}` : "";
  const { gotchas } = await apiFetch<{ gotchas: GotchaSummary[] }>(AGENTOPS_API_URL, `/gotchas${params}`);
  return gotchas;
}

/** `reason` is required (non-empty) when `prominence` is `"Reduced"`, ignored otherwise -- enforced server-side too. */
export async function setCuration(repo: string, id: number, prominence: NodeProminence, reason: string | null): Promise<{ id: number; prominence: NodeProminence }> {
  return apiFetch(AGENTOPS_API_URL, `/repos/${encodeURIComponent(repo)}/nodes/${id}/curation`, {
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

export async function getSubgraph(repo: string, id: number, mode: GraphMode, options: SubgraphOptions = {}): Promise<SubgraphResponse> {
  const params = new URLSearchParams({ mode });
  if (options.depth) params.set("depth", String(options.depth));
  if (options.kinds?.length) params.set("kind", options.kinds.map((k) => k.toLowerCase()).join(","));
  return apiFetch<SubgraphResponse>(AGENTOPS_API_URL, `/repos/${encodeURIComponent(repo)}/nodes/${id}/graph?${params.toString()}`);
}

/** Every node/edge in a repo, not centered on any seed -- the Knowledge Graph screen's "pick a repo" entry point. `SubgraphNode.depth` is always 0 here (there's no BFS distance without a seed). */
export interface RepoGraphResponse {
  repo: string;
  nodes: SubgraphNode[];
  edges: SubgraphEdge[];
  /** `true` if the backend's NODE_CAP (150) was hit -- expect this often for large repos unless `kinds` narrows the result. */
  truncated: boolean;
}

export async function getRepoGraph(repo: string, kinds: NodeKind[] = []): Promise<RepoGraphResponse> {
  const params = new URLSearchParams();
  if (kinds.length) params.set("kind", kinds.map((k) => k.toLowerCase()).join(","));
  const qs = params.toString();
  return apiFetch<RepoGraphResponse>(AGENTOPS_API_URL, `/repos/${encodeURIComponent(repo)}/graph${qs ? `?${qs}` : ""}`);
}

// Documentation Viewer types mirror `agentops-docgen::model` exactly (see
// that crate's `model.rs` for the Rust source of truth). `DocPage` is
// `Serialize`-only in Rust -- these types are never sent back to the
// server, only read from `GET /repos/{name}/docs`'s response.

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

export async function getDocs(repo: string): Promise<DocPage> {
  return apiFetch<DocPage>(AGENTOPS_API_URL, `/repos/${encodeURIComponent(repo)}/docs`);
}
