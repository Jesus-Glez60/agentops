import { publicApiUrl } from "@/lib/server/public-api-url";
import { LocalRepoClient } from "./local-repo-client";

// Server component so `publicApiUrl()` (needs `next/headers`, server-only)
// can be resolved once up front, the same way `/welcome` does it -- the
// client component below only ever needs the already-resolved value, never
// calls server-only code itself.
export default async function LocalRepoPage() {
  const { url: apiUrl, isGuess: apiUrlIsGuessed } = await publicApiUrl();
  return <LocalRepoClient apiUrl={apiUrl} apiUrlIsGuessed={apiUrlIsGuessed} />;
}
