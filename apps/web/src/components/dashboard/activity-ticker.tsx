"use client";

import useSWR from "swr";
import { getActivity } from "@/lib/api/agentops-api";
import { relativeTimeFromIsoString } from "@/lib/relative-time";

export function ActivityTicker() {
  const { data: activity } = useSWR("/activity", getActivity);

  if (!activity || activity.length === 0) return null;

  return (
    <div className="flex items-center gap-6 overflow-x-auto rounded-md border border-border bg-panel px-4 py-2 text-mono-code whitespace-nowrap text-ink-300">
      {activity.map((event, i) => {
        const nodesUpdated = event.files_added + event.files_changed + event.files_removed + event.symbols_added + event.symbols_changed + event.symbols_removed;
        return (
          <span key={`${event.repo}-${event.started_at}-${i}`} className="flex items-center gap-1.5">
            <span className="font-medium text-ink-100">{event.repo}</span>
            rescanned {relativeTimeFromIsoString(event.started_at)}
            <span className="text-health-healthy">
              {nodesUpdated} node{nodesUpdated === 1 ? "" : "s"} updated
            </span>
          </span>
        );
      })}
    </div>
  );
}
