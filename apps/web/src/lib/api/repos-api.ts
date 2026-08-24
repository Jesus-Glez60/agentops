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
