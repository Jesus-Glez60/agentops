import type { NodeKind } from "@/lib/api/repos-api";
import { NODE_KIND_SHAPE } from "@/lib/node-kind-shapes";
import { NavDot } from "@/components/shell/nav-dot";
import { cn } from "@/lib/utils";

// "Docs" has no dedicated NodeKind of its own -- it maps to Note, the kind
// freeform project notes already flow through. See search.rs's plan notes
// for why Note was chosen over Definition (rarely populated in practice).
const FILTERS: { label: string; kind: NodeKind }[] = [
  { label: "Symbols", kind: "Symbol" },
  { label: "Files", kind: "File" },
  { label: "Gotchas", kind: "Gotcha" },
  { label: "Decisions", kind: "Decision" },
  { label: "Docs", kind: "Note" },
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
        const { color, shape } = NODE_KIND_SHAPE[filter.kind];
        return (
          <button
            key={filter.kind}
            type="button"
            onClick={() => onToggle(filter.kind)}
            aria-pressed={active}
            className={cn(
              "inline-flex items-center gap-1.5 rounded-full border px-3 py-1.5 text-[13px] font-semibold transition-colors",
              active ? "border-primary bg-raised text-ink-100" : "border-border-strong text-ink-500 hover:text-ink-300",
            )}
          >
            <NavDot color={color} shape={shape} />
            {filter.label}
          </button>
        );
      })}
    </div>
  );
}
