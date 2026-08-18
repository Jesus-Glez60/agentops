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
