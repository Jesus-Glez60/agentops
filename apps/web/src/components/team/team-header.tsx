import type { TeamInfo } from "@/lib/api/team-api";

// Hero band, not the old plain icon-box + text-lg name (redesign plan
// Phase 7) -- same eyebrow + H1 + subtitle pattern as every other
// top-level screen. The org's identity (name, member count) moves into
// the subtitle since the H1 itself is the generic page title here, same
// as the design.
export function TeamHeader({ team }: { team: TeamInfo }) {
  return (
    <div className="flex flex-col gap-2 border-b border-border-strong px-8 py-6">
      <span className="font-mono text-[12px] uppercase tracking-wide text-mauve">Workspace</span>
      <h1 className="text-display-hero font-extrabold tracking-[-0.04em] text-ink-100">Settings</h1>
      <p className="text-body-lg text-ink-300">
        {team.name || "Your organization"} · {team.member_count} member{team.member_count === 1 ? "" : "s"}
      </p>
    </div>
  );
}
