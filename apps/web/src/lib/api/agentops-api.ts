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
  const { repos } = await apiFetch<{ repos: RepoSummary[] }>(AGENTOPS_API_URL, "/repos");
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

  const { results } = await apiFetch<{ results: SearchResult[] }>(AGENTOPS_API_URL, `/search?${params.toString()}`);
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
