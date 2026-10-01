"use client";

import useSWR from "swr";
import { getActivity } from "@/lib/api/repos-api";
import { relativeTimeFromIsoString } from "@/lib/relative-time";
import { displayRepoName } from "@/lib/utils";

// Vertical list card, matching the prototype's Activity feed layout. Only
// scan-rescan events are available today (getActivity's real, exact data) --
// the prototype also shows gotcha-pin and agent-registration events, which
// aren't backed by the current /activity endpoint; not fabricated here, see
// redesign plan's Session 3 update, Section B.
export function ActivityTicker() {
  const { data: activity } = useSWR("/activity", getActivity);

  return (
    <section className="flex flex-col gap-3.5">
      <h2 className="text-page-title font-extrabold tracking-[-0.02em] text-ink-100">Activity</h2>
      <div className="flex flex-col divide-y divide-border rounded-2xl border border-border-strong bg-panel px-5">
        {!activity || activity.length === 0 ? (
          <p className="py-4 text-[14px] text-ink-500">No activity yet.</p>
        ) : (
          activity.map((event, i) => {
            const nodesUpdated = event.files_added + event.files_changed + event.files_removed + event.symbols_added + event.symbols_changed + event.symbols_removed;
            return (
              <div key={`${event.repo}-${event.started_at}-${i}`} className="flex flex-col gap-1 py-3.5">
                <span className="text-[14px] leading-snug text-ink-100">
                  <span className="font-bold">{displayRepoName(event.repo)}</span> rescanned
                </span>
                <span className="font-mono text-[11.5px] text-ink-500">
                  {relativeTimeFromIsoString(event.started_at)} · <span className="text-health-healthy">{nodesUpdated} node{nodesUpdated === 1 ? "" : "s"} updated</span>
                </span>
              </div>
            );
          })
        )}
      </div>
    </section>
  );
}
