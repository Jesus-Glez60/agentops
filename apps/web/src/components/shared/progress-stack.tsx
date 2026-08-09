import type { RepoCounts } from "@/lib/api/types";

const SEGMENTS: { key: keyof RepoCounts; className: string }[] = [
  { key: "symbols", className: "bg-node-symbol" },
  { key: "files", className: "bg-node-file" },
  { key: "gotchas", className: "bg-node-gotcha" },
  { key: "decisions", className: "bg-node-decision" },
];

/** Stacked horizontal bar showing Files/Symbols/Gotchas/Decisions proportions. */
export function ProgressStack({ counts, className }: { counts: RepoCounts; className?: string }) {
  const total = counts.files + counts.symbols + counts.gotchas + counts.decisions;
  if (total === 0) {
    return <div className={`h-1.5 w-full rounded-full bg-raised ${className ?? ""}`} />;
  }

  return (
    <div className={`flex h-1.5 w-full overflow-hidden rounded-full bg-raised ${className ?? ""}`}>
      {SEGMENTS.map((segment) => {
        const width = (counts[segment.key] / total) * 100;
        if (width === 0) return null;
        return <div key={segment.key} className={segment.className} style={{ width: `${width}%` }} />;
      })}
    </div>
  );
}
