"use client";

import useSWR from "swr";
import type { SessionUser } from "@/lib/auth/types";
import { TEAM_SWR_KEY, getTeam } from "@/lib/api/team-api";
import { Avatar, AvatarFallback, AvatarImage } from "@/components/ui/avatar";

const ROLE_LABELS: Record<string, string> = { admin: "Admin", member: "Member", viewer: "Viewer", billing: "Billing" };

// Hero sizing per redesign plan Phase 8 -- 64px avatar -> 72px, name
// text-lg (18px) -> 36px (roughly a 2x jump, so the surrounding spacing is
// re-tuned here too, not just the two font-size classes). Adds the
// "{email} · {role} of {org}" subtitle line the design has and this hero
// didn't -- `SessionUser` has no role/org-name field itself, so this pulls
// the same `/team` data the Settings hero already fetches.
export function ProfileHero({ user }: { user: SessionUser }) {
  const { data: team } = useSWR(TEAM_SWR_KEY, getTeam);
  const roleLine = team ? `${team.is_owner ? "Owner" : ROLE_LABELS[team.role] ?? team.role} of ${team.name || "your organization"}` : null;

  return (
    <div className="border-b border-border-strong px-8 py-7">
      <div className="flex items-end gap-5">
        <Avatar className="size-[72px] shrink-0 rounded-xl">
          {user.avatar_url && <AvatarImage src={user.avatar_url} alt="" />}
          <AvatarFallback className="rounded-xl text-2xl">{user.first_name.charAt(0).toUpperCase()}</AvatarFallback>
        </Avatar>
        <div className="min-w-0 flex-1">
          <div className="mb-1 flex items-center gap-2">
            <h1 className="text-[36px] font-bold tracking-[-0.02em] text-ink-100">
              {user.first_name} {user.last_name}
            </h1>
            {user.handle && <span className="text-mono-code text-ink-500">@{user.handle}</span>}
          </div>
          <div className="flex flex-wrap items-center gap-x-2 gap-y-1 text-body-lg text-ink-400">
            <span>{user.email}</span>
            {roleLine && (
              <>
                <span className="text-ink-600">·</span>
                <span>{roleLine}</span>
              </>
            )}
            {user.location && (
              <>
                <span className="text-ink-600">·</span>
                <span>{user.location}</span>
              </>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
