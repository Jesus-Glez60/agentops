import { requireUser } from "@/lib/auth/session";
import { AppSidebar } from "@/components/shell/app-sidebar";
import { AppHeader } from "@/components/shell/app-header";
import { SwrProvider } from "@/components/providers/swr-provider";
import { SidebarInset, SidebarProvider } from "@/components/ui/sidebar";

export default async function AppLayout({ children }: { children: React.ReactNode }) {
  // Belt-and-suspenders with middleware.ts's cheap cookie-presence check --
  // this call actually validates the session against agentops-heavy-api.
  const user = await requireUser();

  return (
    <SwrProvider>
      <SidebarProvider>
        <AppSidebar user={user} />
        <SidebarInset>
          <AppHeader />
          {/* min-w-0/min-h-0 override flexbox's default min-width/min-height:auto
              -- without them, content wide or tall enough (a wrapping chip row,
              or a page with its own internal scroll regions like the
              Documentation Viewer) forces this whole flex column, and the
              `h-svh` shell above it, to grow past the viewport instead of
              wrapping/scrolling internally. Requires `SidebarInset`
              (components/ui/sidebar.tsx) to be `h-svh`-bound too -- min-height
              alone doesn't create a clipping boundary, it only permits
              shrinking below content size when something else already caps
              the size. */}
          <main className="min-h-0 min-w-0 flex-1 overflow-y-auto">{children}</main>
        </SidebarInset>
      </SidebarProvider>
    </SwrProvider>
  );
}
