import Link from "next/link";
import { cn } from "@/lib/utils";

export interface StatStripItem {
  label: string;
  value: string | number;
  valueClassName?: string;
  /** Small descriptive line under the value -- Overview's strip has these, Libraries' doesn't. */
  note?: string;
  /** When set, the whole cell is a real link (e.g. Overview's "Gotchas needing curation" -> /gotchas). */
  href?: string;
}

/**
 * One continuous bordered/16px-radius container with `border-right`
 * dividers between cells -- the prototype's own `ovStats`/`libStats`
 * pattern (a single strip, not a grid of individually-bordered cards).
 * A prior pass on this screen mistakenly rebuilt it as separate cards,
 * confused by Gotchas' own (different, card-grid) stat treatment --
 * re-reading the prototype's actual markup for *this* screen directly
 * is what caught it.
 */
export function StatStrip({ items, size = "lg", minColWidth = "150px" }: { items: StatStripItem[]; size?: "lg" | "md"; minColWidth?: string }) {
  return (
    <div className="grid overflow-hidden rounded-2xl border bg-panel" style={{ gridTemplateColumns: `repeat(auto-fit, minmax(${minColWidth}, 1fr))` }}>
      {items.map((item) => {
        const cell = (
          <div className={cn("flex flex-col gap-1 border-r last:border-r-0", size === "lg" ? "gap-1.5 px-6 py-[22px]" : "px-[22px] py-[18px]")}>
            <span className="text-[13.5px] text-ink-300">{item.label}</span>
            <span className={cn(size === "lg" ? "text-stat-value" : "text-[28px] tracking-[-0.03em]", "font-extrabold text-ink-100", item.valueClassName)}>{item.value}</span>
            {item.note && <span className="text-[12.5px] text-ink-500">{item.note}</span>}
          </div>
        );
        return item.href ? (
          <Link key={item.label} href={item.href} className="transition-colors hover:bg-raised">
            {cell}
          </Link>
        ) : (
          <div key={item.label}>{cell}</div>
        );
      })}
    </div>
  );
}
