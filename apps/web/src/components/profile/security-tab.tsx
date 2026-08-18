import type { SessionUser } from "@/lib/auth/types";
import { ChangePasswordForm } from "@/components/profile/change-password-form";
import { TwoFactorCard } from "@/components/profile/two-factor-card";
import { ActiveSessionsCard } from "@/components/profile/active-sessions-card";

export function SecurityTab({ user }: { user: SessionUser }) {
  return (
    <div className="max-w-[900px] space-y-5">
      <ChangePasswordForm />
      <TwoFactorCard user={user} />
      <ActiveSessionsCard />
    </div>
  );
}
