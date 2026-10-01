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
export function StatStrip({ items, size = "lg" }: { items: StatStripItem[]; size?: "lg" | "md" }) {
  return (
    // A fixed column count, not `repeat(auto-fit, minmax(...))` -- at this
    // screen's real (narrower-than-the-prototype's-1280px-canvas) content
    // width, auto-fit only fit 3 of the 4 columns at the chosen min-width,
    // so the 4th item wrapped onto its own row with the other 3 columns'
    // worth of space sitting empty next to it. A flex-basis wrap-based
    // collapse under auto-fit is a layout bug, not a design-fidelity
    // question -- this screen's strip must always keep every item in one
    // row, shrinking cell width rather than ever dropping items to a new
    // row.
    <div className="grid overflow-hidden rounded-2xl border bg-panel" style={{ gridTemplateColumns: `repeat(${items.length}, minmax(0, 1fr))` }}>
      {items.map((item, i) => {
        // `border-r`/`last:border-r-0` live on *this* element -- the actual
        // grid child whose sibling position matters -- not on an inner div,
        // which would be the sole child of its own one-off wrapper and so
        // always match `:last-child` trivially, silently dropping every
        // divider (a real bug an earlier pass here shipped).
        const className = cn("flex flex-col gap-1 border-r last:border-r-0", size === "lg" ? "gap-1.5 px-6 py-[22px]" : "px-[22px] py-[18px]", item.href && "transition-colors hover:bg-raised");
        const content = (
          <>
            {/* `min-h` reserves 2-line height regardless of whether this
                particular label wraps, so every cell's value/note starts at
                the same vertical offset -- a label that wraps on a
                narrower column no longer knocks that one cell's number out
                of alignment with its siblings. */}
            <span className="min-h-[34px] text-[13.5px] leading-snug text-ink-300">{item.label}</span>
            <span className={cn(size === "lg" ? "text-stat-value" : "text-[28px] tracking-[-0.03em]", "font-extrabold text-ink-100", item.valueClassName)}>{item.value}</span>
            {item.note && <span className="text-[12.5px] text-ink-500">{item.note}</span>}
          </>
        );
        return item.href ? (
          <Link key={i} href={item.href} className={className}>
            {content}
          </Link>
        ) : (
          <div key={i} className={className}>
            {content}
          </div>
        );
      })}
    </div>
  );
}
