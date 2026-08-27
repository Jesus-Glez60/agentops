import type { DocBlock } from "@/lib/api/repos-api";

type DependencyChipsBlock = Extract<DocBlock, { block_type: "dependency_chips" }>;

export function DependencyChips({ block }: { block: DependencyChipsBlock }) {
  if (block.deps.length === 0) return null;
  return (
    <div className="mb-4 flex flex-wrap gap-2">
      {block.deps.map((dep) => (
        <span key={dep} className="rounded border border-border-strong px-2 py-1 text-mono-code text-ink-300">
          {dep}
        </span>
      ))}
    </div>
  );
}
