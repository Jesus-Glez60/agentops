import { Button } from "@/components/ui/button";
import Link from "next/link";

function timeOfDayGreeting(): string {
  const hour = new Date().getHours();
  if (hour < 12) return "Good morning";
  if (hour < 18) return "Good afternoon";
  return "Good evening";
}

/** Three real states, not the prototype's two -- its mockup copy hardcodes
 * "one repo still needs attention" even when nothing actually does, which
 * doesn't generalize. See redesign plan's Session 3 update, Section B. */
function subtitle(needsCurationCount: number, anyRepoUnhealthy: boolean): string {
  if (needsCurationCount > 0) {
    return anyRepoUnhealthy ? `${needsCurationCount} gotcha${needsCurationCount === 1 ? "" : "s"} are waiting on you, and one repo needs attention.` : `${needsCurationCount} gotcha${needsCurationCount === 1 ? "" : "s"} are waiting on you.`;
  }
  if (anyRepoUnhealthy) return "The curation queue is clear. One repo still needs attention.";
  return "Everything's in good shape.";
}

export function OverviewHero({ orgName, firstName, needsCurationCount, anyRepoUnhealthy }: { orgName: string; firstName: string; needsCurationCount: number; anyRepoUnhealthy: boolean }) {
  return (
    <div className="flex flex-wrap items-end justify-between gap-6">
      <div className="flex max-w-[760px] flex-col gap-3">
        <span className="font-mono text-[12px] uppercase tracking-wide text-mauve">
          {orgName} · Overview
        </span>
        {/* clamp(34px,3.6vw,48px) in the prototype -- Tailwind arbitrary value carries the clamp() straight through. */}
        <h1 className="text-[clamp(34px,3.6vw,48px)] leading-[1.02] font-extrabold tracking-[-0.04em] text-balance text-ink-100">
          {timeOfDayGreeting()}, {firstName}.
        </h1>
        <p className="text-[17px] leading-normal text-ink-300">{subtitle(needsCurationCount, anyRepoUnhealthy)}</p>
      </div>
      <div className="flex gap-2">
        <Button variant="outline" size="cta" asChild>
          <Link href="/search">Ask the graph</Link>
        </Button>
        <Button size="cta" asChild>
          <Link href="/repositories/connect">+ Connect repository</Link>
        </Button>
      </div>
    </div>
  );
}
