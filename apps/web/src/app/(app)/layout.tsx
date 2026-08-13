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
          {/* min-w-0 overrides flexbox's default min-width:auto -- without it,
              wide content anywhere in a page (e.g. a wrapping chip row) forces
              this whole flex column to grow past the viewport instead of the
              content wrapping/scrolling internally. */}
          <main className="min-w-0 flex-1 overflow-y-auto">{children}</main>
        </SidebarInset>
      </SidebarProvider>
    </SwrProvider>
  );
}
