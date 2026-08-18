import type { AuditEvent } from "@/lib/api/team-api";
import { relativeTimeFromIsoString } from "@/lib/relative-time";

const ACTION_LABELS: Record<string, string> = {
  member_invited: "Member invited",
  invite_resent: "Invite resent",
  invite_revoked: "Invite revoked",
  member_joined: "Member joined",
  member_removed: "Member removed",
  role_changed: "Role changed",
  status_changed: "Status changed",
  repo_access_changed: "Repository access changed",
};

function describe(event: AuditEvent): string {
  const label = ACTION_LABELS[event.action] ?? event.action;
  if (event.action === "role_changed" && typeof event.metadata.role === "string") {
    return `${label} — ${event.target ?? "unknown"} → ${event.metadata.role}`;
  }
  if (event.action === "status_changed" && typeof event.metadata.status === "string") {
    return `${label} — ${event.target ?? "unknown"} → ${event.metadata.status}`;
  }
  if (event.target) return `${label} — ${event.target}`;
  return label;
}

export function AuditLogRow({ event }: { event: AuditEvent }) {
  return (
    <div className="grid grid-cols-[160px_1fr_160px_100px] items-start gap-2 px-4 py-2.5">
      <div className="pt-0.5 text-mono-code text-ink-500">{relativeTimeFromIsoString(event.created_at)}</div>
      <div className="text-body text-ink-200">{describe(event)}</div>
      <div className="text-body text-ink-400">{event.actor_email ?? "system"}</div>
      <div className="text-mono-code text-ink-500">{event.ip_address ?? "—"}</div>
    </div>
  );
}
