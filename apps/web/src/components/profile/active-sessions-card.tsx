"use client";

import useSWR from "swr";
import { toast } from "sonner";
import { Monitor } from "lucide-react";
import { SESSIONS_SWR_KEY, getSessions, revokeSession, revokeOtherSessions } from "@/lib/api/profile-api";
import { relativeTimeFromIsoString } from "@/lib/relative-time";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";

export function ActiveSessionsCard() {
  const { data: sessions, mutate, isLoading } = useSWR(SESSIONS_SWR_KEY, getSessions);

  async function handleRevoke(id: number) {
    try {
      await revokeSession(id);
      await mutate();
      toast.success("Session revoked");
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Couldn't revoke that session. Please try again.");
    }
  }

  async function handleRevokeOthers() {
    try {
      await revokeOtherSessions();
      await mutate();
      toast.success("Other sessions revoked");
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Couldn't revoke other sessions. Please try again.");
    }
  }

  const hasOtherSessions = (sessions ?? []).some((s) => !s.is_current);

  return (
    <Card>
      <CardHeader className="flex-row items-center justify-between border-b border-border-strong pb-4">
        <CardTitle className="flex items-center gap-2">
          <Monitor className="size-4 text-ink-400" />
          Active Sessions
        </CardTitle>
        <Button variant="outline" size="sm" disabled={!hasOtherSessions} onClick={handleRevokeOthers}>
          Revoke all other
        </Button>
      </CardHeader>
      <CardContent className="divide-y divide-border-strong p-0">
        {isLoading && <p className="px-6 py-4 text-body text-ink-500">Loading…</p>}
        {!isLoading && (sessions ?? []).length === 0 && <p className="px-6 py-4 text-body text-ink-500">No active sessions.</p>}
        {(sessions ?? []).map((session) => (
          <div key={session.id} className="flex items-center justify-between px-6 py-3">
            <div>
              <p className="text-body font-medium text-ink-100">{session.user_agent || "Unknown device"}</p>
              <p className="text-mono-code text-ink-500">
                {session.ip_address || "unknown IP"} &middot; {session.is_current ? <span className="text-health-healthy">Current session</span> : relativeTimeFromIsoString(session.last_seen_at)}
              </p>
            </div>
            {!session.is_current && (
              <Button variant="outline" size="sm" onClick={() => handleRevoke(session.id)}>
                Revoke
              </Button>
            )}
          </div>
        ))}
      </CardContent>
    </Card>
  );
}
