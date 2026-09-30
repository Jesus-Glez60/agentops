"use client";

import { useEffect, useState } from "react";
import type { SessionUser } from "@/lib/auth/types";
import { AppSidebar } from "@/components/shell/app-sidebar";
import { AppHeader } from "@/components/shell/app-header";
import { CommandPaletteDialog } from "@/components/shell/command-palette";
import { SidebarInset, SidebarProvider } from "@/components/ui/sidebar";

// Owns the command palette's open state so both the header's icon trigger
// and the sidebar's "Search or jump to..." row open the same dialog
// instance -- plain prop-passing down two known siblings rather than a new
// Context, since AppLayout (the server component above this) can't hold
// React state itself.
export function AppShell({ user, children }: { user: SessionUser; children: React.ReactNode }) {
  const [paletteOpen, setPaletteOpen] = useState(false);

  useEffect(() => {
    function handleKeydown(e: KeyboardEvent) {
      if ((e.metaKey || e.ctrlKey) && e.key === "k") {
        e.preventDefault();
        setPaletteOpen((prev) => !prev);
      }
    }
    document.addEventListener("keydown", handleKeydown);
    return () => document.removeEventListener("keydown", handleKeydown);
  }, []);

  return (
    <SidebarProvider>
      <AppSidebar user={user} onOpenPalette={() => setPaletteOpen(true)} />
      <SidebarInset>
        <AppHeader user={user} onOpenPalette={() => setPaletteOpen(true)} />
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
      <CommandPaletteDialog open={paletteOpen} onOpenChange={setPaletteOpen} />
    </SidebarProvider>
  );
}
