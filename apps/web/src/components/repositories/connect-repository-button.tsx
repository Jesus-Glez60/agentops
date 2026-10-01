"use client";

import Link from "next/link";
import useSWR from "swr";
import { getRepos, REPOS_SWR_KEY } from "@/lib/api/repos-api";
import { Button } from "@/components/ui/button";

/** Replaces the old single-step `ConnectRepositoryDialog` -- now a link into the full connect-repository wizard (`/repositories/connect`), which covers both the GitHub App and SSH deploy-key paths plus indexing progress/failure, none of which fit inside a modal dialog. */
export function ConnectRepositoryButton() {
  // Reads the same cached response `RepositoriesTable` fetches -- no extra
  // request, just needs `can_connect` to decide whether to render at all.
  const { data } = useSWR(REPOS_SWR_KEY, getRepos);
  if (data && !data.can_connect) return null;

  // No "+" icon -- the prototype renders this as a literal "+" character
  // in the label text, not a lucide icon (same "colors and shapes, not
  // icons" pattern as everywhere else this pass touched).
  return (
    <Button size="cta" asChild>
      <Link href="/repositories/connect">+ Connect repository</Link>
    </Button>
  );
}
