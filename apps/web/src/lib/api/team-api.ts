// Typed client for the Team Management screen -- same /api/heavy/* proxy
// pattern as profile-api.ts (see that file's doc comment for why: session
// token must stay server-side).

import { heavyFetch } from "@/lib/api/heavy-fetch";
import { withBasePath } from "@/lib/base-path";

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
  return `${window.location.origin}${withBasePath(`/invite/${token}`)}`;
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

export const TEAM_MCP_ACCESS_MODE_SWR_KEY = "/team/mcp-access-mode";

export type McpAccessMode = "advisor" | "full";

/**
 * Gates every `/mcp` write tool (`scan_repo`, `explain_symbol`, task tools,
 * ...) for the whole org, not just the caller -- defaults to `"advisor"`
 * (read-only) server-side, admin-only to change (`CAP_MCP_MANAGE_ACCESS_MODE`).
 * `add_note`/`ingest_notes` are never gated by this at all -- growing the
 * knowledge base is safe even in Advisor mode, so there's nothing to
 * "unlock" there regardless of what this is set to.
 */
export function getMcpAccessMode(): Promise<{ mode: McpAccessMode }> {
  return heavyFetch<{ mode: McpAccessMode }>(TEAM_MCP_ACCESS_MODE_SWR_KEY);
}

export function setMcpAccessMode(mode: McpAccessMode): Promise<{ mode: McpAccessMode }> {
  return heavyFetch<{ mode: McpAccessMode }>(TEAM_MCP_ACCESS_MODE_SWR_KEY, { method: "PUT", body: JSON.stringify({ mode }) });
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

// --- Org switcher (self-service multi-tenant membership) ----------------

export const MY_MEMBERSHIPS_SWR_KEY = "/me/memberships";

export interface Membership {
  tenant: string;
  /** `""` for a tenant nobody's ever named (e.g. a freelancer's own default personal tenant) -- same fallback the Team screen's own name field uses. */
  name: string;
  role: MemberRole;
  status: MemberStatus;
  joined_at: string;
  is_current: boolean;
}

/** Every org the caller belongs to, across the whole instance -- not just the currently active one. Backs the org switcher; a solo user always gets back exactly one row (their own tenant). */
export function getMyMemberships(): Promise<{ memberships: Membership[] }> {
  return heavyFetch(MY_MEMBERSHIPS_SWR_KEY);
}

/** Switches the caller's active tenant to one they're already an active member of -- 403s otherwise. No new session token: the same bearer token keeps working and simply resolves to the new tenant from the next request onward, so callers must still refresh any tenant-scoped data themselves after this resolves (see `use-org-switch.ts`). */
export function switchTenant(tenant: string): Promise<{ tenant: string; role: MemberRole; name: string }> {
  return heavyFetch("/me/switch-tenant", { method: "POST", body: JSON.stringify({ tenant }) });
}

/** Shared by the sidebar's ScopeSwitcher and the Overview page's hero eyebrow -- `user.tenant` is a connection-id-shaped string on real deployments, never fit for display, so both fall back the same way: the current membership's name if set, else the email's local part. */
export function resolveOrgDisplayName(memberships: Membership[], user: { tenant: string; email: string }): string {
  return memberships.find((m) => m.tenant === user.tenant)?.name || user.email.split("@")[0] || user.tenant;
}

/** Unauthenticated -- goes through /api/invites/{token}, not /api/heavy/* (which always requires a session). See that route's doc comment. */
export async function getInvitePreview(token: string): Promise<InvitePreview> {
  const res = await fetch(withBasePath(`/api/invites/${encodeURIComponent(token)}`), { cache: "no-store" });
  const data = await res.json().catch(() => null);
  if (!res.ok) {
    const message = data && typeof data === "object" && typeof data.error === "string" ? data.error : "This invite link is invalid or has expired.";
    throw new Error(message);
  }
  return data as InvitePreview;
}
