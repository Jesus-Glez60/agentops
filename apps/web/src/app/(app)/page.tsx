import { requireUser } from "@/lib/auth/session";
import { OverviewPageClient } from "@/components/dashboard/overview-page-client";

export default async function OverviewPage() {
  const user = await requireUser();
  return <OverviewPageClient user={user} />;
}
