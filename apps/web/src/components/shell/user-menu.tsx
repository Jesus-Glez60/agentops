"use client";

import { useRouter } from "next/navigation";
import { UserRound, Users } from "lucide-react";
import type { SessionUser } from "@/lib/auth/types";
import { Avatar, AvatarFallback, AvatarImage } from "@/components/ui/avatar";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";

// Org switching lives in the sidebar's ScopeSwitcher now (see redesign plan
// Phase 2), and "Connect a coding tool"/"Log out" now live as persistent
// sidebar-footer controls (Session 3 update) rather than buried here -- kept
// out of this dropdown so there's exactly one place to do each, not two
// competing controls.
export function UserMenu({ user }: { user: SessionUser }) {
  const router = useRouter();

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button className="flex w-full items-center gap-2 rounded-md px-2 py-2 text-left hover:bg-accent">
          <Avatar className="size-7 shrink-0">
            {user.avatar_url && <AvatarImage src={user.avatar_url} alt="" />}
            <AvatarFallback className="text-label">{user.first_name.charAt(0).toUpperCase()}</AvatarFallback>
          </Avatar>
          <div className="min-w-0 flex-1 group-data-[collapsible=icon]:hidden">
            <p className="truncate text-section text-ink-100">
              {user.first_name} {user.last_name}
            </p>
            <p className="truncate text-mono-path text-ink-500">{user.email}</p>
          </div>
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" side="top" className="w-56">
        <DropdownMenuItem onSelect={() => router.push("/profile")}>
          <UserRound className="size-4" />
          Profile
        </DropdownMenuItem>
        <DropdownMenuItem onSelect={() => router.push("/settings")}>
          <Users className="size-4" />
          Team Settings
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
