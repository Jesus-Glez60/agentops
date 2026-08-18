// Typed client for both integrations layers -- same heavyFetch/*_SWR_KEY
// convention as team-api.ts/profile-api.ts/repos-api.ts. Org-wide
// (Owner/Admin-managed, Team Management's Integrations tab, `/integrations*`)
// and personal (any member, Profile's Integrations tab, `/integrations/me*`)
// share this one file since they're structurally identical -- same request
// shapes, same response shapes -- just different base paths and audiences.

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

export interface IntegrationSummary {
  provider: string;
  auth_type: "api_key" | "oauth";
  created_at: string;
  updated_at: string;
}

export interface StoreIntegrationInput {
  auth_type: "api_key" | "oauth";
  secret: string;
  refresh_token?: string;
  expires_at?: string;
}

// --- Org-wide (Owner/Admin only -- backend enforces `integrations.manage`, this client never re-derives that check) ---

export const ORG_INTEGRATIONS_SWR_KEY = "/integrations";

export function getOrgIntegrations(): Promise<IntegrationSummary[]> {
  return heavyFetch<IntegrationSummary[]>(ORG_INTEGRATIONS_SWR_KEY);
}

export function storeOrgIntegration(provider: string, input: StoreIntegrationInput): Promise<void> {
  return heavyFetch<void>(`/integrations/${encodeURIComponent(provider)}`, { method: "POST", body: JSON.stringify(input) });
}

export function deleteOrgIntegration(provider: string): Promise<void> {
  return heavyFetch<void>(`/integrations/${encodeURIComponent(provider)}`, { method: "DELETE" });
}

// --- Personal (any member, self-scoped -- no capability check on the backend) ---

export const MY_INTEGRATIONS_SWR_KEY = "/integrations/me";

export function getMyIntegrations(): Promise<IntegrationSummary[]> {
  return heavyFetch<IntegrationSummary[]>(MY_INTEGRATIONS_SWR_KEY);
}

export function storeMyIntegration(provider: string, input: StoreIntegrationInput): Promise<void> {
  return heavyFetch<void>(`/integrations/me/${encodeURIComponent(provider)}`, { method: "POST", body: JSON.stringify(input) });
}

export function deleteMyIntegration(provider: string): Promise<void> {
  return heavyFetch<void>(`/integrations/me/${encodeURIComponent(provider)}`, { method: "DELETE" });
}
