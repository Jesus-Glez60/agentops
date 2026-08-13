import type { DocBlock } from "@/lib/api/agentops-api";

type SymbolTableBlock = Extract<DocBlock, { block_type: "symbol_table" }>;

export function SymbolTable({ block }: { block: SymbolTableBlock }) {
  return (
    <div className="mb-4 overflow-hidden rounded-md border border-border-strong">
      <div className="border-b border-border-strong bg-canvas px-3 py-1.5 text-mono-path text-ink-500">{block.file}</div>
      <div className="divide-y divide-border-strong">
        {block.rows.map((row) => (
          <div key={row.node_id} className="flex items-center gap-3 px-4 py-2.5">
            <span className="shrink-0 text-mono-code text-ink-100">{row.name}()</span>
            {row.one_liner && <span className="min-w-0 flex-1 truncate text-body text-ink-500">{row.one_liner}</span>}
            {row.gotcha_count > 0 && (
              <span className="ml-auto shrink-0 rounded border border-node-gotcha/30 bg-node-gotcha/10 px-1.5 py-0.5 text-label text-node-gotcha">
                {row.gotcha_count} gotcha{row.gotcha_count > 1 ? "s" : ""}
              </span>
            )}
          </div>
        ))}
      </div>
    </div>
  );
}
