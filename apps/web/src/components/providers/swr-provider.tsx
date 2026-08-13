"use client";

import type { ReactNode } from "react";
import { SWRConfig } from "swr";

// One global refreshInterval rather than each useSWR call configuring its
// own (SWR's own documented pattern for a dashboard with multiple hooks
// sharing config). No global `fetcher` here -- each hook in this app passes
// its own typed fetch function (getRepos/getActivity/...) as the second
// arg, so there's no untyped string-keyed fetcher to get wrong.
export function SwrProvider({ children }: { children: ReactNode }) {
  return <SWRConfig value={{ refreshInterval: 60_000 }}>{children}</SWRConfig>;
}
