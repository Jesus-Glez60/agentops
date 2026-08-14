import { cn } from "@/lib/utils";

export function VersionPillSelector({ versions, selected, onSelect }: { versions: string[]; selected: string; onSelect: (v: string) => void }) {
  return (
    <div className="flex flex-wrap items-center justify-end gap-1.5">
      {versions
        .slice()
        .reverse()
        .map((v) => (
          <button
            key={v}
            onClick={() => onSelect(v)}
            className={cn(
              "rounded-md border px-2.5 py-1 text-mono-code transition-colors",
              v === selected ? "border-primary/40 bg-primary/10 text-primary" : "border-border-strong text-ink-400 hover:text-ink-100",
            )}
          >
            {v}
          </button>
        ))}
    </div>
  );
}
