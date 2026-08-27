import type { NodeKind } from "@/lib/api/repos-api";
import { Checkbox } from "@/components/ui/checkbox";
import { Slider } from "@/components/ui/slider";

// Same node kinds/colors search-filters.tsx's pills use -- kept as a
// vertical checkbox list here per the mockup, not pills, since this panel
// is a persistent sidebar section rather than an inline filter row.
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

export function GraphFilterPanel({
  kinds,
  onToggleKind,
  depth,
  onDepthChange,
  showDepth = true,
}: {
  /** Empty means "no filter" (all kinds shown) -- same convention `search`/`getGotchas` already use. */
  kinds: NodeKind[];
  onToggleKind: (kind: NodeKind) => void;
  depth: number;
  onDepthChange: (depth: number) => void;
  /** Depth is a BFS-around-a-seed concept -- hidden in whole-repo mode, where there's no seed to measure distance from. */
  showDepth?: boolean;
}) {
  return (
    <div className="flex w-48 shrink-0 flex-col gap-4 rounded-lg border border-border-strong bg-panel p-3">
      <div className="flex flex-col gap-1.5">
        <p className="text-label uppercase tracking-wide text-ink-500">Node types</p>
        <div className="flex flex-col gap-1.5">
          {KIND_FILTERS.map((filter) => (
            <label key={filter.kind} className="flex items-center gap-2 text-body text-ink-300">
              <Checkbox checked={kinds.length === 0 || kinds.includes(filter.kind)} onCheckedChange={() => onToggleKind(filter.kind)} />
              <span className={`size-2 shrink-0 rounded-full ${filter.dotClassName}`} />
              {filter.label}
            </label>
          ))}
        </div>
      </div>

      {showDepth && (
        <div className="flex flex-col gap-1.5">
          <p className="text-label uppercase tracking-wide text-ink-500">Depth</p>
          <div className="flex items-center gap-2">
            <Slider value={[depth]} onValueChange={(v) => onDepthChange(v[0])} min={MIN_DEPTH} max={MAX_DEPTH} step={1} />
            <span className="w-4 text-mono-code text-ink-400">{depth}</span>
          </div>
        </div>
      )}
    </div>
  );
}
