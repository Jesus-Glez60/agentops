import { AddLibraryDialog } from "@/components/libraries/add-library-dialog";
import { LibraryTable } from "@/components/libraries/library-table";

export default function LibrariesPage() {
  return (
    <div className="flex h-full flex-col">
      {/* Hero band, not the old 52px breadcrumb bar -- same eyebrow + H1 +
          subtitle + primary-action pattern as Repositories/Overview/Gotchas
          (redesign plan Phase 6). */}
      <div className="flex flex-wrap items-end justify-between gap-6 border-b border-border-strong px-6 py-6">
        <div className="flex flex-col gap-2">
          <span className="font-mono text-[12px] uppercase tracking-wide text-mauve">Sources</span>
          <h1 className="text-display-hero font-extrabold tracking-[-0.04em] text-ink-100">Libraries</h1>
          <p className="max-w-[560px] text-body-lg text-ink-300">Third-party dependencies your agents reference, with version tracking and generated docs.</p>
        </div>
        <AddLibraryDialog />
      </div>
      <div className="min-h-0 flex-1">
        <LibraryTable />
      </div>
    </div>
  );
}
