"use client";

import { useRouter } from "next/navigation";
import { CommandIcon } from "lucide-react";
import { NAV_ITEMS } from "@/lib/nav-config";
import { Button } from "@/components/ui/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
// cmdk's CommandDialog does not auto-wrap children in <Command> -- the
// explicit <Command> root below is required or item filtering/selection
// silently doesn't work (day-one-bug-checklist.md #4).
import { Command, CommandDialog, CommandEmpty, CommandGroup, CommandInput, CommandItem, CommandList } from "@/components/ui/command";

// Mounted once (in AppShell) so the header's icon trigger and the sidebar's
// "Search or jump to..." row both open this same dialog instance instead of
// each owning a separate <CommandDialog> (two portals/overlays, real bug
// risk). Controlled via plain open/onOpenChange props, per cmdk's own docs.
export function CommandPaletteDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const router = useRouter();

  return (
    <CommandDialog open={open} onOpenChange={onOpenChange}>
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
                  onOpenChange(false);
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
  );
}

// Icon-button trigger for the top header.
export function CommandPaletteTrigger({ onOpen }: { onOpen: () => void }) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button variant="outline" size="icon" onClick={onOpen} aria-label="Open command palette">
          <CommandIcon className="size-4" />
        </Button>
      </TooltipTrigger>
      <TooltipContent>Jump to a page (⌘K)</TooltipContent>
    </Tooltip>
  );
}
