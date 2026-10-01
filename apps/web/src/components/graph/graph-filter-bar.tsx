import type { NodeKind } from "@/lib/api/repos-api";
import { Checkbox } from "@/components/ui/checkbox";
import { Slider } from "@/components/ui/slider";
import { cn } from "@/lib/utils";

// Same node kinds/colors search-filters.tsx's pills use. A full-width
// horizontal bar below the header, not the old fixed-width left sidebar
// (redesign plan Phase 3) -- "Show" kind-toggle buttons with live counts on
// the left, depth/hotspot controls on the right. The design's "Most
// central" ranked-symbols column is deliberately omitted here, not shown
// as an empty state or placeholder -- it needs a new backend symbol-level
// centrality pass that doesn't exist yet (see the plan's Phase 3b
// follow-up); this bar gets that column added once that data exists.
const KIND_FILTERS: { label: string; kind: NodeKind; dotClassName: string }[] = [
  { label: "Symbols", kind: "Symbol", dotClassName: "bg-node-symbol" },
  { label: "Files", kind: "File", dotClassName: "bg-node-file" },
  { label: "Gotchas", kind: "Gotcha", dotClassName: "bg-node-gotcha" },
  { label: "Decisions", kind: "Decision", dotClassName: "bg-node-decision" },
];

/** The full toggleable kind list, exported so the page can expand an empty
 * ("all") `kinds` array into an explicit one when a single kind is first
 * unchecked. */
export const GRAPH_FILTERABLE_KINDS: NodeKind[] = KIND_FILTERS.map((f) => f.kind);

const MIN_DEPTH = 1;
const MAX_DEPTH = 4;

export function GraphFilterBar({
  kinds,
  onToggleKind,
  kindCounts,
  depth,
  onDepthChange,
  showDepth = true,
  showHotspots,
  onToggleHotspots,
}: {
  /** Empty means "no filter" (all kinds shown) -- same convention `search`/`getGotchas` already use. */
  kinds: NodeKind[];
  onToggleKind: (kind: NodeKind) => void;
  /** Live counts from the currently-loaded graph payload -- omitted (not zero-filled) kinds just show no count. */
  kindCounts?: Partial<Record<NodeKind, number>>;
  depth: number;
  onDepthChange: (depth: number) => void;
  /** Depth is a BFS-around-a-seed concept -- hidden in whole-repo mode, where there's no seed to measure distance from. */
  showDepth?: boolean;
  /** Graph hotspot overlay toggle -- not seed-dependent, unlike depth, so always shown. */
  showHotspots: boolean;
  onToggleHotspots: () => void;
}) {
  return (
    <div className="flex flex-wrap items-center gap-5 border-b border-border-strong bg-panel px-6 py-3">
      <div className="flex flex-wrap items-center gap-2">
        <span className="text-label uppercase tracking-wide text-ink-500">Show</span>
        {KIND_FILTERS.map((filter) => {
          const active = kinds.length === 0 || kinds.includes(filter.kind);
          const count = kindCounts?.[filter.kind];
          return (
            <button
              key={filter.kind}
              type="button"
              onClick={() => onToggleKind(filter.kind)}
              aria-pressed={active}
              className={cn(
                "flex items-center gap-1.5 rounded-md border px-2.5 py-1 text-section transition-colors",
                active ? "border-primary bg-raised text-ink-100" : "border-border-strong text-ink-500 hover:text-ink-300",
              )}
            >
              <span className={cn("size-2 shrink-0 rounded-full", filter.dotClassName)} />
              {filter.label}
              {typeof count === "number" && <span className="text-mono-code text-ink-500">{count}</span>}
            </button>
          );
        })}
      </div>

      {showDepth && (
        <div className="flex items-center gap-2">
          <span className="text-label uppercase tracking-wide text-ink-500">Depth</span>
          <Slider value={[depth]} onValueChange={(v) => onDepthChange(v[0])} min={MIN_DEPTH} max={MAX_DEPTH} step={1} className="w-28" />
          <span className="w-4 text-mono-code text-ink-400">{depth}</span>
        </div>
      )}

      <label className="ml-auto flex items-center gap-2 text-body text-ink-300">
        <Checkbox checked={showHotspots} onCheckedChange={onToggleHotspots} />
        <span className="size-2 shrink-0 rounded-full bg-node-hotspot" />
        Hotspots
      </label>
    </div>
  );
}
