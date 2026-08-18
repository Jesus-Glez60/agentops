import { Building2, Users } from "lucide-react";
import type { TeamInfo } from "@/lib/api/team-api";

export function TeamHeader({ team }: { team: TeamInfo }) {
  return (
    <div className="flex items-center gap-4 border-b border-border-strong px-8 py-6">
      <div className="flex size-12 shrink-0 items-center justify-center rounded-xl border border-border-strong bg-panel">
        <Building2 className="size-5 text-ink-400" />
      </div>
      <div className="min-w-0 flex-1">
        <h1 className="text-lg font-semibold text-ink-100">{team.name || "Your organization"}</h1>
        <div className="flex items-center gap-1.5 text-body text-ink-400">
          <Users className="size-3.5" />
          {team.member_count} member{team.member_count === 1 ? "" : "s"}
        </div>
      </div>
    </div>
  );
}
