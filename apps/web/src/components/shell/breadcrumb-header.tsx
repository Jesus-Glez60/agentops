"use client";

import { useEffect, useState } from "react";
import { useRouter } from "next/navigation";
import { usePathname } from "next/navigation";
import { Bell, CommandIcon } from "lucide-react";
import { NAV_ITEMS, navLabelForPath } from "@/lib/nav-config";
import { Breadcrumb, BreadcrumbItem, BreadcrumbLink, BreadcrumbList, BreadcrumbPage, BreadcrumbSeparator } from "@/components/ui/breadcrumb";
import { Button } from "@/components/ui/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { Command, CommandDialog, CommandEmpty, CommandGroup, CommandInput, CommandItem, CommandList } from "@/components/ui/command";

export function BreadcrumbHeader() {
  const pathname = usePathname();
  const router = useRouter();
  const [paletteOpen, setPaletteOpen] = useState(false);

  useEffect(() => {
    function handleKeydown(e: KeyboardEvent) {
      if ((e.metaKey || e.ctrlKey) && e.key === "k") {
        e.preventDefault();
        setPaletteOpen((open) => !open);
      }
    }
    document.addEventListener("keydown", handleKeydown);
    return () => document.removeEventListener("keydown", handleKeydown);
  }, []);

  const label = navLabelForPath(pathname);
  const isRoot = pathname === "/";

  return (
    <header className="flex h-14 shrink-0 items-center justify-between border-b border-border bg-panel px-4">
      <Breadcrumb>
        <BreadcrumbList>
          <BreadcrumbItem>
            {isRoot ? <BreadcrumbPage>Overview</BreadcrumbPage> : <BreadcrumbLink href="/">Overview</BreadcrumbLink>}
          </BreadcrumbItem>
          {!isRoot && (
            <>
              <BreadcrumbSeparator />
              <BreadcrumbItem>
                <BreadcrumbPage>{label}</BreadcrumbPage>
              </BreadcrumbItem>
            </>
          )}
        </BreadcrumbList>
      </Breadcrumb>

      <div className="flex items-center gap-1">
        <Tooltip>
          <TooltipTrigger asChild>
            <Button variant="outline" size="icon" onClick={() => setPaletteOpen(true)}>
              <CommandIcon className="size-4" />
            </Button>
          </TooltipTrigger>
          <TooltipContent>Search pages (⌘K)</TooltipContent>
        </Tooltip>
        <Tooltip>
          <TooltipTrigger asChild>
            {/* No notifications backend exists -- rendered disabled/empty
                rather than showing a fake unread count. */}
            <Button variant="outline" size="icon" disabled>
              <Bell className="size-4" />
            </Button>
          </TooltipTrigger>
          <TooltipContent>No notifications</TooltipContent>
        </Tooltip>
      </div>

      <CommandDialog open={paletteOpen} onOpenChange={setPaletteOpen}>
        <Command>
          <CommandInput placeholder="Jump to a page..." />
          <CommandList>
            <CommandEmpty>No matching page.</CommandEmpty>
            <CommandGroup heading="Pages">
              {NAV_ITEMS.map((item) => (
                <CommandItem
                  key={item.href}
                  onSelect={() => {
                    router.push(item.href);
                    setPaletteOpen(false);
                  }}
                >
                  <item.icon className="size-4" />
                  {item.label}
                </CommandItem>
              ))}
            </CommandGroup>
          </CommandList>
        </Command>
      </CommandDialog>
    </header>
  );
}
