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

// Hero band, not the old 52px breadcrumb bar -- matches every other
// top-level screen's header treatment (redesign plan Phase 3). The 28px
// H1 size is a one-off that doesn't match any named display token exactly
// (closest are 24px/30px), so it's an arbitrary Tailwind value per Phase
// 1's own "don't force a mismatched token" rule.
export function GraphHeader({
  seedLabel,
  mode,
  onModeChange,
  repo,
  onChangeRepo,
  branch,
  nodeCount,
}: {
  /** The centered node's display name/path, shown breadcrumb-style. `null` in whole-repo mode (no seed picked yet). */
  seedLabel: string | null;
  /** `null` in whole-repo mode -- the 4 tabs are all seed-relative (BFS direction/relation filters), meaningless without one. */
  mode: GraphMode | null;
  onModeChange: (mode: GraphMode) => void;
  repo: string;
  onChangeRepo: (repo: string) => void;
  branch?: string | null;
  nodeCount?: number;
}) {
  return (
    <div className="flex flex-col gap-3 border-b border-border-strong px-6 py-5">
      <div className="flex flex-wrap items-start justify-between gap-4">
        <div className="flex flex-col gap-1">
          <h1 className="text-[28px] font-extrabold tracking-[-0.02em] text-ink-100">Knowledge Graph</h1>
          <p className="font-mono text-section text-ink-500">
            {repo}
            {branch ? ` · ${branch}` : ""}
            {typeof nodeCount === "number" ? ` · ${nodeCount} nodes` : ""} · size shows centrality · hover to isolate connections ·{" "}
            <span className="text-ink-300">{seedLabel ?? "all nodes"}</span>
          </p>
        </div>
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
      {mode !== null && (
        <div className="flex items-center gap-0.5 self-start rounded-md border border-border-strong bg-panel p-0.5">
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
    </div>
  );
}
