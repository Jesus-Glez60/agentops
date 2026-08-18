import { ConnectRepositoryDialog } from "@/components/repositories/connect-repository-dialog";
import { RepositoriesTable } from "@/components/repositories/repositories-table";

export default function RepositoriesPage() {
  return (
    <div className="flex h-full flex-col">
      <div className="flex h-[52px] shrink-0 items-center justify-between border-b border-border-strong px-5">
        <span className="text-section font-medium text-ink-100">Repositories</span>
        <ConnectRepositoryDialog />
      </div>
      <div className="min-h-0 flex-1">
        <RepositoriesTable />
      </div>
    </div>
  );
}
