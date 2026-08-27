import { requireUser } from "@/lib/auth/session";
import { publicApiUrl } from "@/lib/server/public-api-url";
import { ProfilePageClient } from "@/components/profile/profile-page-client";

export default async function ProfilePage() {
  const user = await requireUser();
  // Same header-derivation `/welcome` uses -- see `publicApiUrl`'s doc
  // comment. Needed here too since the API Keys tab's "Connect a coding
  // tool" section (Initiative 2) generates the same `connect.sh` command,
  // just as a persistent, revisitable home for it alongside `/welcome`.
  const { url: apiUrl, isGuess: apiUrlIsGuessed } = await publicApiUrl();
  return <ProfilePageClient initialUser={user} apiUrl={apiUrl} apiUrlIsGuessed={apiUrlIsGuessed} />;
}
