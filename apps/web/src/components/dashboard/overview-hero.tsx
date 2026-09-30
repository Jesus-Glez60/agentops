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
      <div className="flex flex-col gap-2">
        <span className="text-mono-path uppercase tracking-wide text-mauve">
          {orgName} · Overview
        </span>
        <h1 className="text-3xl font-bold tracking-tight text-ink-100">
          {timeOfDayGreeting()}, {firstName}.
        </h1>
        <p className="text-body text-ink-300">{subtitle(needsCurationCount, anyRepoUnhealthy)}</p>
      </div>
      <div className="flex gap-2">
        <Button variant="outline" asChild>
          <Link href="/search">Ask the graph</Link>
        </Button>
        <Button asChild>
          <Link href="/repositories/connect">+ Connect repository</Link>
        </Button>
      </div>
    </div>
  );
}
