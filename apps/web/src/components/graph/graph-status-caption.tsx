import type { GraphMode } from "@/lib/api/agentops-api";

const MODE_LABEL: Record<GraphMode, string> = {
  local: "Local graph",
  dep_chain: "Dep. chain",
  impact: "Impact",
  knowledge: "Knowledge",
};

export function GraphStatusCaption({ repo, branch, mode, depth }: { repo: string; branch: string | null | undefined; mode: GraphMode | null; depth: number | null }) {
  return (
    <div className="pointer-events-none absolute bottom-4 right-4 text-mono-code text-ink-500">
      {repo}
      {branch ? ` · ${branch}` : ""} · {mode ? MODE_LABEL[mode] : "All nodes"}
      {depth !== null ? ` · depth ${depth}` : ""}
    </div>
  );
}
