"use client";

import { useState } from "react";
import useSWR from "swr";
import { Search } from "lucide-react";
import { getAuditLog } from "@/lib/api/team-api";
import { AuditLogRow } from "@/components/team/audit-log-row";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";

const ACTION_OPTIONS = [
  { value: "all", label: "All actions" },
  { value: "member_invited", label: "Member invited" },
  { value: "invite_resent", label: "Invite resent" },
  { value: "invite_revoked", label: "Invite revoked" },
  { value: "member_joined", label: "Member joined" },
  { value: "member_removed", label: "Member removed" },
  { value: "role_changed", label: "Role changed" },
  { value: "status_changed", label: "Status changed" },
  { value: "repo_access_changed", label: "Repository access changed" },
];

export function AuditLogTab() {
  const [search, setSearch] = useState("");
  const [action, setAction] = useState("all");
  const { data: events, isLoading } = useSWR(["team-audit-log", action, search], () => getAuditLog({ action: action === "all" ? undefined : action, search: search || undefined }));

  return (
    <div>
      <div className="mb-3 flex items-center gap-2">
        <div className="relative max-w-xs flex-1">
          <Search className="absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-ink-500" />
          <Input value={search} onChange={(e) => setSearch(e.target.value)} placeholder="Search audit events…" className="pl-7" />
        </div>
        <Select value={action} onValueChange={setAction}>
          <SelectTrigger className="w-48">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {ACTION_OPTIONS.map((o) => (
              <SelectItem key={o.value} value={o.value}>
                {o.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>
      <div className="rounded-lg border border-border-strong">
        <div className="grid grid-cols-[160px_1fr_160px_100px] gap-2 border-b border-border-strong px-4 py-2 text-mono-code uppercase tracking-wide text-ink-500">
          <span>Timestamp</span>
          <span>Event</span>
          <span>Actor</span>
          <span>IP</span>
        </div>
        <div className="divide-y divide-border-strong">
          {isLoading && <p className="px-4 py-6 text-center text-body text-ink-500">Loading…</p>}
          {!isLoading && (events ?? []).length === 0 && <p className="px-4 py-6 text-center text-body text-ink-500">No audit events yet.</p>}
          {(events ?? []).map((event) => (
            <AuditLogRow key={event.id} event={event} />
          ))}
        </div>
      </div>
    </div>
  );
}
