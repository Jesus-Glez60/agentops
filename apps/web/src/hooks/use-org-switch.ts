"use client";

import { useState } from "react";
import { useRouter } from "next/navigation";
import { useSWRConfig } from "swr";
import { toast } from "sonner";
import { switchTenant } from "@/lib/api/team-api";
import type { SessionUser } from "@/lib/auth/types";

// Shared by every org-switch UI (sidebar ScopeSwitcher, and previously
// UserMenu) so there's one implementation of "switch tenant, refetch
// everything tenant-scoped" rather than copies drifting apart.
export function useOrgSwitch(user: SessionUser) {
  const router = useRouter();
  const { mutate } = useSWRConfig();
  const [switching, setSwitching] = useState<string | null>(null);

  async function switchOrg(tenant: string) {
    if (tenant === user.tenant || switching) return;
    setSwitching(tenant);
    try {
      await switchTenant(tenant);
      // No new session token is issued (the existing bearer token already
      // resolves to the new tenant from here on) -- everything tenant-
      // scoped still needs refetching though: `router.refresh()` re-runs
      // the server-side session lookup that supplies `user` to this whole
      // shell, and revalidating every SWR key picks up client-fetched
      // tenant-scoped data (repos, gotchas, team, ...) without a full page
      // reload, so in-progress state elsewhere on the page isn't lost.
      await mutate(() => true, undefined, { revalidate: true });
      router.refresh();
      router.push("/");
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Couldn't switch organizations. Please try again.");
    } finally {
      setSwitching(null);
    }
  }

  return { switchOrg, switching };
}
