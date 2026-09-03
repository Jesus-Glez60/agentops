// Typed client for the Profile screen -- unlike libraries-api.ts/agentops-api.ts,
// these calls go through this app's own /api/heavy/* proxy (see
// heavy-proxy.ts's doc comment), not straight to a backend, since
// agentops-heavy-api requires a bearer session token that must never reach
// the browser.
import type { SessionUser } from "@/lib/auth/types";

/** Matches the existing `getMe`-equivalent backend path -- there's no separate `/profile` route, `/auth/me` already returns the full profile. */
export const PROFILE_SWR_KEY = "/auth/me";

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

export function getProfile(): Promise<SessionUser> {
  return heavyFetch<SessionUser>(PROFILE_SWR_KEY);
}

export interface ProfileUpdateInput {
  first_name?: string;
  last_name?: string;
  handle?: string;
  bio?: string;
  location?: string;
}

export function updateProfile(update: ProfileUpdateInput): Promise<SessionUser> {
  return heavyFetch<SessionUser>("/auth/me", { method: "PATCH", body: JSON.stringify(update) });
}

export interface PreferencesUpdateInput {
  theme_pref?: string;
  default_search_scope?: string;
  show_gotcha_callouts?: boolean;
  graph_layout_algorithm?: string;
}

export function updatePreferences(update: PreferencesUpdateInput): Promise<SessionUser> {
  return heavyFetch<SessionUser>("/auth/me/preferences", { method: "PATCH", body: JSON.stringify(update) });
}

/** Marks the `/welcome` checklist done -- idempotent, safe to call from any item's completion or the persistent "Continue to dashboard" button. */
export function completeOnboarding(): Promise<SessionUser> {
  return heavyFetch<SessionUser>("/auth/me/complete-onboarding", { method: "POST" });
}

/** On success, revokes every session but the one that made this call -- the caller should expect other tabs/devices to be signed out. */
export function changePassword(currentPassword: string, newPassword: string): Promise<void> {
  return heavyFetch<void>("/auth/me/password", { method: "POST", body: JSON.stringify({ current_password: currentPassword, new_password: newPassword }) });
}

export interface TwoFactorEnrollment {
  secret: string;
  otpauth_uri: string;
  qr_data_uri: string;
}

export function begin2fa(): Promise<TwoFactorEnrollment> {
  return heavyFetch<TwoFactorEnrollment>("/auth/2fa/enroll", { method: "POST" });
}

/** Backup codes are returned raw exactly once -- the caller must show them to the user now, they're never retrievable again. */
export function confirm2fa(code: string): Promise<{ backup_codes: string[] }> {
  return heavyFetch<{ backup_codes: string[] }>("/auth/2fa/confirm", { method: "POST", body: JSON.stringify({ code }) });
}

export function disable2fa(password: string): Promise<void> {
  return heavyFetch<void>("/auth/2fa/disable", { method: "POST", body: JSON.stringify({ password }) });
}

export function regenerateBackupCodes(password: string): Promise<{ backup_codes: string[] }> {
  return heavyFetch<{ backup_codes: string[] }>("/auth/2fa/backup-codes/regenerate", { method: "POST", body: JSON.stringify({ password }) });
}

export const SESSIONS_SWR_KEY = "/auth/sessions";

export interface SessionInfo {
  id: number;
  user_agent: string;
  ip_address: string;
  created_at: string;
  last_seen_at: string;
  is_current: boolean;
}

export function getSessions(): Promise<SessionInfo[]> {
  return heavyFetch<SessionInfo[]>(SESSIONS_SWR_KEY);
}

export function revokeSession(id: number): Promise<void> {
  return heavyFetch<void>(`/auth/sessions/${id}`, { method: "DELETE" });
}

export function revokeOtherSessions(): Promise<void> {
  return heavyFetch<void>("/auth/sessions/revoke-others", { method: "POST" });
}

export const API_KEYS_SWR_KEY = "/auth/api-keys";

export interface ApiKeyInfo {
  id: number;
  name: string;
  key_prefix: string;
  last_used_at: string | null;
  created_at: string;
}

export function getApiKeys(): Promise<ApiKeyInfo[]> {
  return heavyFetch<ApiKeyInfo[]>(API_KEYS_SWR_KEY);
}

/** Response includes the raw `key` -- shown to the user exactly once, never retrievable again after this call returns. */
export function createApiKey(name: string): Promise<ApiKeyInfo & { key: string }> {
  return heavyFetch<ApiKeyInfo & { key: string }>(API_KEYS_SWR_KEY, { method: "POST", body: JSON.stringify({ name }) });
}

export function revokeApiKey(id: number): Promise<void> {
  return heavyFetch<void>(`/auth/api-keys/${id}`, { method: "DELETE" });
}

/** `gh auth login`-style CLI device-authorization flow -- the `/cli-auth` page's Approve/Deny buttons. */
export function approveDeviceAuth(userCode: string): Promise<void> {
  return heavyFetch<void>("/auth/cli/device/approve", { method: "POST", body: JSON.stringify({ user_code: userCode, action: "approve" }) });
}

export function denyDeviceAuth(userCode: string): Promise<void> {
  return heavyFetch<void>("/auth/cli/device/approve", { method: "POST", body: JSON.stringify({ user_code: userCode, action: "deny" }) });
}
