import { GitBranch } from "lucide-react";
import { EmptyState } from "@/components/shared/empty-state";

export default function RepositoriesPage() {
  return <EmptyState icon={GitBranch} title="Nothing here yet" description="Repositories content is coming in a future session." />;
}
