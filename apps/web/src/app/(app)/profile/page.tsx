import { requireUser } from "@/lib/auth/session";
import { ProfilePageClient } from "@/components/profile/profile-page-client";

export default async function ProfilePage() {
  const user = await requireUser();
  return <ProfilePageClient initialUser={user} />;
}
