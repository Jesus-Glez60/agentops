import type { TocEntry } from "@/lib/markdown-toc";
import { cn } from "@/lib/utils";

export function DocToc({ entries }: { entries: TocEntry[] }) {
  return (
    <nav className="space-y-1">
      <p className="mb-2 text-label uppercase tracking-wide text-ink-500">On this page</p>
      {entries.map((entry) => (
        <a
          key={entry.slug}
          href={`#${entry.slug}`}
          className={cn(
            "block truncate text-body text-ink-300 hover:text-ink-100",
            entry.level === 1 && "font-medium text-ink-100",
            entry.level === 3 && "pl-3 text-mono-path",
          )}
        >
          {entry.text}
        </a>
      ))}
    </nav>
  );
}
