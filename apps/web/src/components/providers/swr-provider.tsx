"use client";

import type { ReactNode } from "react";
import { SWRConfig } from "swr";

/**
 * Global SWR defaults. `revalidateOnFocus` is off since none of this app's
 * backends push state changes from a tab-focus event -- the default would
 * just cause redundant refetches with no benefit.
 */
export function SwrProvider({ children }: { children: ReactNode }) {
  return <SWRConfig value={{ revalidateOnFocus: false }}>{children}</SWRConfig>;
}
