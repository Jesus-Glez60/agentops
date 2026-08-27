import { ChevronDown } from "lucide-react";
import type { GraphMode } from "@/lib/api/repos-api";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { RepoPicker } from "@/components/graph/repo-picker";
import { cn } from "@/lib/utils";

const MODE_TABS: { label: string; value: GraphMode }[] = [
  { label: "Local graph", value: "local" },
  { label: "Dep. chain", value: "dep_chain" },
  { label: "Impact", value: "impact" },
  { label: "Knowledge", value: "knowledge" },
];

export function GraphHeader({
  seedLabel,
  mode,
  onModeChange,
  repo,
  onChangeRepo,
}: {
  /** The centered node's display name/path, shown breadcrumb-style. `null` in whole-repo mode (no seed picked yet). */
  seedLabel: string | null;
  /** `null` in whole-repo mode -- the 4 tabs are all seed-relative (BFS direction/relation filters), meaningless without one. */
  mode: GraphMode | null;
  onModeChange: (mode: GraphMode) => void;
  repo: string;
  onChangeRepo: (repo: string) => void;
}) {
  return (
    <div className="flex h-[52px] shrink-0 items-center justify-between border-b border-border-strong px-5">
      <div className="flex items-center gap-2 text-section">
        <span className="font-medium text-ink-100">Knowledge Graph</span>
        <span className="text-ink-500">/</span>
        <span className="font-mono text-ink-300">{seedLabel ?? "all nodes"}</span>
      </div>
      <div className="flex items-center gap-2">
        {mode !== null && (
          <div className="flex items-center gap-0.5 rounded-md border border-border-strong bg-panel p-0.5">
            {MODE_TABS.map((tab) => (
              <button
                key={tab.value}
                type="button"
                onClick={() => onModeChange(tab.value)}
                aria-pressed={mode === tab.value}
                className={cn(
                  "rounded-md border px-2.5 py-1 text-section transition-colors",
                  mode === tab.value ? "border-primary bg-raised text-ink-100" : "border-border-strong text-ink-500 hover:text-ink-300",
                )}
              >
                {tab.label}
              </button>
            ))}
          </div>
        )}
        {/* Repo picker, not a scope filter -- a subgraph/repo-graph is
            always single-repo (edges never cross repos), so this switches
            which repo's data is loaded rather than multi-filtering it. */}
        <Popover>
          <PopoverTrigger asChild>
            <button
              type="button"
              className="flex items-center gap-1.5 rounded-md border border-border-strong px-2 py-1.5 text-mono-code text-ink-400 transition-colors hover:border-border-strong/80 hover:text-ink-100"
            >
              {repo}
              <ChevronDown className="size-3 text-ink-500" />
            </button>
          </PopoverTrigger>
          <PopoverContent align="end" className="w-64">
            <RepoPicker onSelect={onChangeRepo} />
          </PopoverContent>
        </Popover>
      </div>
    </div>
  );
}
