import { Suspense } from "react";
import { TenantProvider } from "@/lib/tenant-context";
import { AppShell } from "@/components/shell/app-shell";

// TenantProvider uses useSearchParams(), which requires a Suspense
// boundary during static rendering -- this wraps the whole (app) route
// group, since every real page lives inside it.
export default function AppLayout({ children }: { children: React.ReactNode }) {
  return (
    <Suspense fallback={null}>
      <TenantProvider>
        <AppShell>{children}</AppShell>
      </TenantProvider>
    </Suspense>
  );
}
