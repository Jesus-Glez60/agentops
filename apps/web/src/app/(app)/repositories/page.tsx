import { ConnectRepositoryButton } from "@/components/repositories/connect-repository-button";
import { RepositoriesTable } from "@/components/repositories/repositories-table";

export default function RepositoriesPage() {
  return (
    <div className="flex h-full flex-col">
      {/* Hero band, not the old 52px breadcrumb bar -- same eyebrow + H1 +
          subtitle + primary-action pattern as Overview/Gotchas (redesign
          plan Phase 6). */}
      <div className="flex flex-wrap items-end justify-between gap-6 border-b border-border-strong px-6 py-6">
        <div className="flex flex-col gap-2">
          <span className="font-mono text-[12px] uppercase tracking-wide text-mauve">Sources</span>
          <h1 className="text-display-hero font-extrabold tracking-[-0.04em] text-ink-100">Repositories</h1>
          <p className="max-w-[560px] text-body-lg text-ink-300">Every repo your agents can read and write to, and how healthy its scan is.</p>
        </div>
        <ConnectRepositoryButton />
      </div>
      <div className="min-h-0 flex-1">
        <RepositoriesTable />
      </div>
    </div>
  );
}
