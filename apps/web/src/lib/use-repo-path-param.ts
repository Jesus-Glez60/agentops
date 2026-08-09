"use client";

import { useCallback, useEffect, useState } from "react";
import { useRouter, useSearchParams } from "next/navigation";

/**
 * Extracts the exact byte-identical boilerplate confirmed duplicated
 * between the old graph/page.tsx and docs/page.tsx: read `?path=` from the
 * URL on mount, keep it in local state, and keep the URL in sync when it
 * changes. Callers still need to wrap their page in <Suspense> themselves
 * (useSearchParams requires it), since the fallback UI is page-specific.
 */
export function useRepoPathParam() {
  const searchParams = useSearchParams();
  const router = useRouter();
  const [path, setPathState] = useState(searchParams.get("path") ?? "");

  // Reading the initial value into state is done above via useState's
  // lazy-ish default (searchParams is available synchronously at render
  // time here, unlike localStorage) -- no effect needed for that part.
  // This effect only re-syncs if a *different* page navigates here with a
  // new ?path= while this hook instance is already mounted.
  useEffect(() => {
    const fromUrl = searchParams.get("path");
    if (fromUrl && fromUrl !== path) {
      // Legitimate "synchronize with an external system" use of an effect
      // (the external system being the URL/router, not local component
      // state) -- not the anti-pattern this rule otherwise warns about.
      // eslint-disable-next-line react-hooks/set-state-in-effect
      setPathState(fromUrl);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [searchParams]);

  const setPath = useCallback(
    (next: string) => {
      setPathState(next);
      const params = new URLSearchParams(searchParams.toString());
      if (next) {
        params.set("path", next);
      } else {
        params.delete("path");
      }
      router.replace(`?${params.toString()}`);
    },
    [router, searchParams],
  );

  return { path, setPath };
}
