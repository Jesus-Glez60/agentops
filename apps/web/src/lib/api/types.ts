// Single source of truth for every backend response shape this app reads.
// Field names and casing here are confirmed against the actual Rust
// serialization (plain `#[derive(Serialize)]` enums serialize as their
// PascalCase variant name, not the lowercase strings used internally for
// SQL storage) -- do not "helpfully" lowercase these.

// ---------------------------------------------------------------------------
// agentops-api (light tier)
// ---------------------------------------------------------------------------

export interface RepoCounts {
  files: number;
  symbols: number;
  gotchas: number;
  decisions: number;
}

export interface RepoSummary {
  path: string;
  last_scanned_at: number; // unix seconds
  counts: RepoCounts | null;
}

export type NodeKind = "Symbol" | "File" | "Gotcha" | "Decision";
export type EdgeRelation = "DependsOn" | "Documents" | "Affects";

export interface GraphNode {
  id: number;
  kind: NodeKind;
  repo: string;
  path: string | null;
  name: string | null;
  start_line: number | null;
  end_line: number | null;
  content: string | null;
}

export interface GraphEdge {
  id: number;
  src_id: number;
  dst_id: number;
  relation: EdgeRelation;
}

export interface GraphResponse {
  nodes: GraphNode[];
  edges: GraphEdge[];
}

export interface DocsResponse {
  content: string;
}

// ---------------------------------------------------------------------------
// docbrain-api (light tier)
// ---------------------------------------------------------------------------

// Rust: `enum Visibility { Public, Private(String) }`, default serde shape
// for a mixed unit/tuple enum: unit variant -> its bare name, tuple variant
// -> `{ VariantName: innerValue }`.
export type Visibility = "Public" | { Private: string };

export function isPrivateVisibility(v: Visibility): v is { Private: string } {
  return typeof v === "object" && v !== null && "Private" in v;
}

export function visibilityLabel(v: Visibility): string {
  return isPrivateVisibility(v) ? `private (${v.Private})` : "public";
}

export interface Library {
  id: number;
  slug: string;
  name: string;
  github_repo: string | null;
  docs_url: string | null;
  visibility: Visibility;
  /** How many times this library has been scraped/ingested (not distinct versions -- see `versions`). */
  doc_snapshots: number;
  changelog_versions: number;
  /** Distinct versions with at least one doc snapshot, ascending. */
  versions: string[];
  total_nodes: number;
}

// ---------------------------------------------------------------------------
// agentops-heavy-api (heavy tier)
// ---------------------------------------------------------------------------

export type ConnectionMethod = "ssh" | "github_app";

// The real shape is `"pending" | "active" | "failed: {reason}"` -- the
// failure case is a single templated string, not a structured variant, so
// this stays a plain `string` with a helper rather than an unreliable
// template-literal-type narrowing.
export type ConnectionStatus = string;

export function isFailedStatus(status: ConnectionStatus): boolean {
  return status.startsWith("failed");
}

export function failureReason(status: ConnectionStatus): string | null {
  if (!isFailedStatus(status)) return null;
  const idx = status.indexOf(":");
  return idx === -1 ? null : status.slice(idx + 1).trim();
}

export interface ConnectionView {
  id: string;
  tenant: string;
  repo_url: string;
  method: ConnectionMethod;
  public_key_openssh: string | null;
  status: ConnectionStatus;
  created_at: string;
}

export interface ConnectRepoResponse {
  connection: ConnectionView;
  instructions: string;
}

export interface VerifyResponse {
  status: "active" | "failed";
  reason?: string;
}

export interface InstallUrlResponse {
  install_url: string;
}

export interface SearchHit {
  id: number | string;
  score: number;
  kind: string;
  name: string | null;
  path: string | null;
  text: string;
}

export interface SearchResponse {
  results: SearchHit[];
}

export interface DocSearchHit {
  id: number | string;
  score: number;
  slug: string;
  topic: string | null;
  version: string | null;
  text: string;
}

export interface DocSearchResponse {
  results: DocSearchHit[];
}

export interface IndexResponse {
  indexed: number;
}

// ---------------------------------------------------------------------------
// Generic MCP tool-call envelope (agentops-api & docbrain-api `/tools/*`)
// ---------------------------------------------------------------------------

export interface ToolDefinition {
  name: string;
  description: string;
  inputSchema: unknown;
}

export interface ToolContentBlock {
  type: "text";
  text: string;
}

export interface CallToolResult {
  content: ToolContentBlock[];
  isError: boolean;
}
