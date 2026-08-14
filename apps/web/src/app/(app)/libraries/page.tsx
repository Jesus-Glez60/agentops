import { AddLibraryDialog } from "@/components/libraries/add-library-dialog";
import { LibraryTable } from "@/components/libraries/library-table";

export default function LibrariesPage() {
  return (
    <div className="flex h-full flex-col">
      <div className="flex h-[52px] shrink-0 items-center justify-between border-b border-border-strong px-5">
        <span className="text-section font-medium text-ink-100">Libraries</span>
        <AddLibraryDialog />
      </div>
      <div className="min-h-0 flex-1">
        <LibraryTable />
      </div>
    </div>
  );
}
