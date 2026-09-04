import { publicApiUrl } from "@/lib/server/public-api-url";
import { RepoDetailPageClient } from "./repo-detail-page-client";

export default async function RepoDetailPage() {
  // Server-only (`next/headers`) -- see `publicApiUrl`'s doc comment. Needed
  // here for the Usage card's `usage sync --remote` command, same reason
  // `/welcome`/`/profile` need it for their `connect --remote` command.
  const { url: apiUrl } = await publicApiUrl();
  return <RepoDetailPageClient apiUrl={apiUrl} />;
}
