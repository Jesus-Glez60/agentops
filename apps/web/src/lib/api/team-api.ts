// Typed client for the Team Management screen -- same /api/heavy/* proxy
// pattern as profile-api.ts (see that file's doc comment for why: session
// token must stay server-side).

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

export const TEAM_SWR_KEY = "/team";

export interface TeamInfo {
  tenant: string;
  name: string;
  member_count: number;
  /** The caller's own role in this org. */
  role: string;
  /** Whether the caller is this org's single Owner -- gates ownership transfer, org deletion, and demoting/removing another admin. */
  is_owner: boolean;
}

export function getTeam(): Promise<TeamInfo> {
  return heavyFetch<TeamInfo>(TEAM_SWR_KEY);
}

/** Owner-only -- naming the org, used by the `/welcome` onboarding checklist's workspace-setup item. */
export function renameOrg(name: string): Promise<{ name: string }> {
  return heavyFetch<{ name: string }>("/team", { method: "PATCH", body: JSON.stringify({ name }) });
}

export const TEAM_MEMBERS_SWR_KEY = "/team/members";

export type MemberRole = "admin" | "member" | "viewer" | "billing";
export type MemberStatus = "active" | "suspended";

export interface TeamMember {
  user_id: number;
  email: string;
  first_name: string;
  last_name: string;
  avatar_url: string | null;
  /** One of `MemberRole`, or a custom role's `role_key` (`"custom:{id}"`) -- widened to `string` rather than `MemberRole` since a member can hold either. */
  role: string;
  status: MemberStatus;
  joined_at: string;
  is_you: boolean;
}

export function getTeamMembers(): Promise<TeamMember[]> {
  return heavyFetch<TeamMember[]>(TEAM_MEMBERS_SWR_KEY);
}

export function updateTeamMember(userId: number, update: { role?: string; status?: MemberStatus }): Promise<void> {
  return heavyFetch<void>(`/team/members/${userId}`, { method: "PATCH", body: JSON.stringify(update) });
}

export function removeTeamMember(userId: number): Promise<void> {
  return heavyFetch<void>(`/team/members/${userId}`, { method: "DELETE" });
}

/** Owner-only; the target must be an active admin. */
export function transferOwnership(toUserId: number): Promise<void> {
  return heavyFetch<void>("/team/transfer-ownership", { method: "POST", body: JSON.stringify({ to_user_id: toUserId }) });
}

/** Owner-only, full cascade delete -- `confirmTenant` must exactly match the caller's own tenant (type-to-confirm), not just a button click. Response includes the `new_tenant` the caller is switched into. */
export function deleteOrganization(confirmTenant: string): Promise<{ deleted: boolean; new_tenant: string }> {
  return heavyFetch<{ deleted: boolean; new_tenant: string }>("/team/delete-organization", { method: "POST", body: JSON.stringify({ confirm_tenant: confirmTenant }) });
}

export const TEAM_INVITES_SWR_KEY = "/team/invites";

export interface TeamInvite {
  id: number;
  email: string;
  /** `MemberRole` or a custom role's `role_key` -- see `TeamMember.role`. */
  role: string;
  note: string | null;
  status: string;
  created_at: string;
  expires_at: string;
}

export function getTeamInvites(): Promise<TeamInvite[]> {
  return heavyFetch<TeamInvite[]>(TEAM_INVITES_SWR_KEY);
}

/** Response includes the raw invite `token` -- no email is sent (no email infrastructure exists), so the caller must show a copyable `/invite/{token}` link. */
export function createTeamInvite(input: { email: string; role: string; note?: string }): Promise<TeamInvite & { token: string }> {
  return heavyFetch<TeamInvite & { token: string }>(TEAM_INVITES_SWR_KEY, { method: "POST", body: JSON.stringify(input) });
}

export function resendTeamInvite(id: number): Promise<{ token: string }> {
  return heavyFetch<{ token: string }>(`/team/invites/${id}/resend`, { method: "POST" });
}

export function cancelTeamInvite(id: number): Promise<void> {
  return heavyFetch<void>(`/team/invites/${id}`, { method: "DELETE" });
}

export function inviteUrl(token: string): string {
  return `${window.location.origin}/invite/${token}`;
}

export function acceptInvite(token: string): Promise<{ tenant: string; role: string }> {
  return heavyFetch<{ tenant: string; role: string }>("/invites/accept", { method: "POST", body: JSON.stringify({ token }) });
}

export const TEAM_ROLES_SWR_KEY = "/team/roles";

export interface RoleInfo {
  role: MemberRole;
  label: string;
  description: string;
  member_count: number;
}

export interface Capability {
  key: string;
  feature_area: string;
  label: string;
  allowed_roles: MemberRole[];
}

export interface CustomRole {
  role_key: string;
  label: string;
  cloned_from: MemberRole;
  capabilities: string[];
}

export const FIXED_ROLE_LABELS: Record<MemberRole, string> = { admin: "Admin", member: "Member", viewer: "Viewer", billing: "Billing" };

/** A member/invite's `role` field is either a fixed `MemberRole` or a custom role's `role_key` (`"custom:{id}"`) -- this resolves either to a display label, shared by the Members tab's role dropdown/badge and pending-invite rows. */
export function roleLabel(role: string, customRoles: CustomRole[]): string {
  if (role in FIXED_ROLE_LABELS) return FIXED_ROLE_LABELS[role as MemberRole];
  return customRoles.find((r) => r.role_key === role)?.label ?? role;
}

export interface TeamRoles {
  roles: RoleInfo[];
  matrix: Capability[];
  custom_roles: CustomRole[];
}

export function getTeamRoles(): Promise<TeamRoles> {
  return heavyFetch<TeamRoles>(TEAM_ROLES_SWR_KEY);
}

export const TEAM_REPO_ACCESS_SWR_KEY = "/team/repo-access";

export interface RepoAccessRepo {
  id: string;
  repo_url: string;
}

export interface RepoAccessMember {
  user_id: number;
  first_name: string;
  last_name: string;
  role: MemberRole;
}

export interface RepoAccessCell {
  user_id: number;
  repo_id: string;
  allowed: boolean;
  editable: boolean;
}

export interface RepoAccessGrid {
  repos: RepoAccessRepo[];
  members: RepoAccessMember[];
  access: RepoAccessCell[];
}

export function getRepoAccess(): Promise<RepoAccessGrid> {
  return heavyFetch<RepoAccessGrid>(TEAM_REPO_ACCESS_SWR_KEY);
}

export function saveRepoAccess(changes: { user_id: number; repo_id: string; allowed: boolean }[]): Promise<void> {
  return heavyFetch<void>(TEAM_REPO_ACCESS_SWR_KEY, { method: "PUT", body: JSON.stringify({ changes }) });
}

export const TEAM_AUDIT_LOG_SWR_KEY = "/team/audit-log";

export interface AuditEvent {
  id: number;
  action: string;
  target: string | null;
  actor_email: string | null;
  ip_address: string | null;
  metadata: Record<string, unknown>;
  created_at: string;
}

export interface AuditLogFilters {
  action?: string;
  search?: string;
}

export function getAuditLog(filters: AuditLogFilters = {}): Promise<AuditEvent[]> {
  const params = new URLSearchParams();
  if (filters.action) params.set("action", filters.action);
  if (filters.search) params.set("search", filters.search);
  const query = params.toString();
  return heavyFetch<AuditEvent[]>(`${TEAM_AUDIT_LOG_SWR_KEY}${query ? `?${query}` : ""}`);
}

/** Clones one of the 4 base roles + an initial capability list -- the base role's own capabilities are the sensible starting point, pre-checked and then freely toggleable in the creation dialog before this call is made. */
export function createCustomRole(input: { label: string; cloned_from: MemberRole; capabilities: string[] }): Promise<CustomRole> {
  return heavyFetch<CustomRole>("/team/roles/custom", { method: "POST", body: JSON.stringify(input) });
}

export function updateCustomRoleCapabilities(roleKey: string, capabilities: string[]): Promise<void> {
  return heavyFetch<void>(`/team/roles/custom/${encodeURIComponent(roleKey)}`, { method: "PATCH", body: JSON.stringify({ capabilities }) });
}

/** Blocked server-side (400) while any active member still holds this role. */
export function deleteCustomRole(roleKey: string): Promise<void> {
  return heavyFetch<void>(`/team/roles/custom/${encodeURIComponent(roleKey)}`, { method: "DELETE" });
}

export interface InvitePreview {
  tenant: string;
  org_name: string;
  email: string;
  role: MemberRole;
}

/** Unauthenticated -- goes through /api/invites/{token}, not /api/heavy/* (which always requires a session). See that route's doc comment. */
export async function getInvitePreview(token: string): Promise<InvitePreview> {
  const res = await fetch(`/api/invites/${encodeURIComponent(token)}`, { cache: "no-store" });
  const data = await res.json().catch(() => null);
  if (!res.ok) {
    const message = data && typeof data === "object" && typeof data.error === "string" ? data.error : "This invite link is invalid or has expired.";
    throw new Error(message);
  }
  return data as InvitePreview;
}
