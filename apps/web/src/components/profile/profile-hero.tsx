import type { SessionUser } from "@/lib/auth/types";
import { Avatar, AvatarFallback, AvatarImage } from "@/components/ui/avatar";

export function ProfileHero({ user }: { user: SessionUser }) {
  return (
    <div className="border-b border-border-strong px-8 py-6">
      <div className="flex items-end gap-4">
        <Avatar className="size-16 shrink-0 rounded-xl">
          {user.avatar_url && <AvatarImage src={user.avatar_url} alt="" />}
          <AvatarFallback className="rounded-xl text-lg">{user.first_name.charAt(0).toUpperCase()}</AvatarFallback>
        </Avatar>
        <div className="min-w-0 flex-1">
          <div className="mb-1 flex items-center gap-2">
            <h1 className="text-lg font-semibold text-ink-100">
              {user.first_name} {user.last_name}
            </h1>
            {user.handle && <span className="text-mono-code text-ink-500">@{user.handle}</span>}
          </div>
          <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-body text-ink-400">
            <span>{user.email}</span>
            {user.location && <span>{user.location}</span>}
          </div>
        </div>
      </div>
    </div>
  );
}
