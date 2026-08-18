import type { SessionUser } from "@/lib/auth/types";
import { PersonalInfoCard } from "@/components/profile/personal-info-card";
import { PreferencesCard } from "@/components/profile/preferences-card";

export function AccountTab({ user }: { user: SessionUser }) {
  return (
    <div className="space-y-5">
      <PersonalInfoCard user={user} />
      <PreferencesCard user={user} />
    </div>
  );
}
