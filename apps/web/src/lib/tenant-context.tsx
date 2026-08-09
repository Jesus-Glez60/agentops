"use client";

import { createContext, useCallback, useContext, useEffect, useState, type ReactNode } from "react";
import { useRouter, useSearchParams } from "next/navigation";
import { getGithubAppInstallUrl } from "@/lib/api/heavy-api";
import { ApiError } from "@/lib/api/fetcher";

const STORAGE_KEY = "agentops.tenant";

interface TenantContextValue {
  /** `null` means "public" (no org selected) -- the light-tier default. */
  tenant: string | null;
  setTenant: (tenant: string | null) => void;
  hasTenant: boolean;
  /**
   * `null` while the one-time probe is in flight, then `true`/`false`.
   * `false` means this deployment has no heavy tier at all (the
   * GitHub-App install-url probe 404s) -- distinct from `hasTenant` being
   * false, which just means no org has been picked yet.
   */
  heavyTierAvailable: boolean | null;
}

const TenantContext = createContext<TenantContextValue | null>(null);

export function TenantProvider({ children }: { children: ReactNode }) {
  const router = useRouter();
  const searchParams = useSearchParams();
  const [tenant, setTenantState] = useState<string | null>(null);
  const [heavyTierAvailable, setHeavyTierAvailable] = useState<boolean | null>(null);

  // Source of truth is the `?tenant=` URL param (heavy-api already requires
  // this as a query string on every call), falling back to localStorage so
  // a reload/bookmark without the param still remembers the last org.
  //
  // This has to run as an effect, not a lazy useState initializer: reading
  // `window.localStorage` during the initial render would run on the
  // server too (this is "use client", but still SSR'd for the first pass)
  // and produce a value that can't match the server-rendered HTML, causing
  // a hydration mismatch. Deferring the read to an effect (which only
  // fires post-hydration, client-side) is the correct, intentional use of
  // "synchronize with an external system" here -- not the anti-pattern
  // react-hooks/set-state-in-effect otherwise warns about.
  useEffect(() => {
    const fromUrl = searchParams.get("tenant");
    if (fromUrl) {
      // eslint-disable-next-line react-hooks/set-state-in-effect
      setTenantState(fromUrl);
      window.localStorage.setItem(STORAGE_KEY, fromUrl);
      return;
    }
    const fromStorage = window.localStorage.getItem(STORAGE_KEY);
    if (fromStorage) setTenantState(fromStorage);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // One-time probe: a 404 from the install-url endpoint means no GitHub App
  // is configured for this deployment at all, i.e. heavy tier isn't
  // deployed here -- distinct from "tenant not picked yet."
  useEffect(() => {
    let cancelled = false;
    getGithubAppInstallUrl()
      .then(() => {
        if (!cancelled) setHeavyTierAvailable(true);
      })
      .catch((err) => {
        if (cancelled) return;
        // A 404 specifically means "not configured" -- any other failure
        // (network error, 5xx) is ambiguous, so don't claim heavy tier is
        // definitively unavailable, just leave it unresolved (null).
        if (err instanceof ApiError && err.status === 404) {
          setHeavyTierAvailable(false);
        }
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const setTenant = useCallback(
    (next: string | null) => {
      setTenantState(next);
      if (next) {
        window.localStorage.setItem(STORAGE_KEY, next);
      } else {
        window.localStorage.removeItem(STORAGE_KEY);
      }
      const params = new URLSearchParams(searchParams.toString());
      if (next) {
        params.set("tenant", next);
      } else {
        params.delete("tenant");
      }
      router.replace(`?${params.toString()}`);
    },
    [router, searchParams],
  );

  return (
    <TenantContext.Provider value={{ tenant, setTenant, hasTenant: tenant !== null, heavyTierAvailable }}>
      {children}
    </TenantContext.Provider>
  );
}

export function useTenant(): TenantContextValue {
  const ctx = useContext(TenantContext);
  if (!ctx) throw new Error("useTenant must be used within a TenantProvider");
  return ctx;
}
