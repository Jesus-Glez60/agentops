import type { RepoCounts } from "@/lib/api/repos-api";
import { cn } from "@/lib/utils";

const SEGMENTS: { key: keyof RepoCounts; className: string }[] = [
  { key: "symbols", className: "bg-node-symbol" },
  { key: "files", className: "bg-node-file" },
  { key: "gotchas", className: "bg-node-gotcha" },
  { key: "decisions", className: "bg-node-decision" },
];

/** Stacked horizontal bar showing Symbols/Files/Gotchas/Decisions proportions. */
export function NodeCountBar({ counts, className }: { counts: RepoCounts; className?: string }) {
  const total = counts.symbols + counts.files + counts.gotchas + counts.decisions;
  if (total === 0) {
    return <div className={cn("h-1.5 w-full rounded-full bg-raised", className)} />;
  }

  return (
    <div className={cn("flex h-1.5 w-full overflow-hidden rounded-full bg-raised", className)}>
      {SEGMENTS.map((segment) => {
        const width = (counts[segment.key] / total) * 100;
        if (width === 0) return null;
        return <div key={segment.key} className={segment.className} style={{ width: `${width}%` }} />;
      })}
    </div>
  );
}
