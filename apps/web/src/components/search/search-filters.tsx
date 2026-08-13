import type { NodeKind } from "@/lib/api/agentops-api";
import { cn } from "@/lib/utils";

// "Docs" has no dedicated NodeKind of its own -- it maps to Note, the kind
// freeform project notes already flow through. See search.rs's plan notes
// for why Note was chosen over Definition (rarely populated in practice).
const FILTERS: { label: string; kind: NodeKind; dotClassName: string }[] = [
  { label: "Symbols", kind: "Symbol", dotClassName: "bg-node-symbol" },
  { label: "Files", kind: "File", dotClassName: "bg-node-file" },
  { label: "Gotchas", kind: "Gotcha", dotClassName: "bg-node-gotcha" },
  { label: "Decisions", kind: "Decision", dotClassName: "bg-node-decision" },
  { label: "Docs", kind: "Note", dotClassName: "bg-node-note" },
];

export function SearchFilters({
  selected,
  onToggle,
}: {
  selected: NodeKind[];
  onToggle: (kind: NodeKind) => void;
}) {
  return (
    <div className="flex flex-wrap gap-2">
      {FILTERS.map((filter) => {
        const active = selected.includes(filter.kind);
        return (
          <button
            key={filter.kind}
            type="button"
            onClick={() => onToggle(filter.kind)}
            aria-pressed={active}
            className={cn(
              "inline-flex items-center gap-1.5 rounded-md border px-2.5 py-1 text-section transition-colors",
              active ? "border-primary bg-raised text-ink-100" : "border-border-strong text-ink-500 hover:text-ink-300",
            )}
          >
            <span className={cn("size-1.5 rounded-full", filter.dotClassName)} />
            {filter.label}
          </button>
        );
      })}
    </div>
  );
}
