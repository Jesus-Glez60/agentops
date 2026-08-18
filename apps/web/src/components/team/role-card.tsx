import { Crown, User, Eye, CreditCard } from "lucide-react";
import type { RoleInfo, MemberRole } from "@/lib/api/team-api";
import { Card, CardContent } from "@/components/ui/card";

const ROLE_ICONS: Record<MemberRole, typeof Crown> = { admin: Crown, member: User, viewer: Eye, billing: CreditCard };

export function RoleCard({ role }: { role: RoleInfo }) {
  const Icon = ROLE_ICONS[role.role];
  return (
    <Card size="sm">
      <CardContent className="space-y-2">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-2">
            <div className="flex size-6 items-center justify-center rounded-md border border-border-strong bg-panel">
              <Icon className="size-3.5 text-ink-400" />
            </div>
            <span className="text-body font-semibold text-ink-100">{role.label}</span>
          </div>
          <span className="text-mono-code text-ink-500">
            {role.member_count} member{role.member_count === 1 ? "" : "s"}
          </span>
        </div>
        <p className="text-section text-ink-400">{role.description}</p>
      </CardContent>
    </Card>
  );
}
