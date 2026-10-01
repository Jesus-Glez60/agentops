import Link from "next/link";
import { Card, CardContent } from "@/components/ui/card";
import { cn } from "@/lib/utils";

// No icon -- the prototype's own stat-strip (`ovStats`) is label + a big
// colored number + a note, nothing else. An icon here was never in the
// design; it crept in during an earlier pass. Border/radius also matched
// the prototype's own `border:1px solid #313244` (this project's default
// `border` color, not the stronger `border-border-strong`) + 16px radius.
export function StatCard({
  label,
  value,
  valueClassName,
  href,
  note,
}: {
  label: string;
  value: string | number;
  valueClassName?: string;
  /** When set, the whole card is a real link -- e.g. to the gotchas page's actual review queue, not a "coming soon" placeholder. */
  href?: string;
  /** Small descriptive line under the value, matching the prototype's stat-strip notes (e.g. "symbols, files, gotchas, decisions"). */
  note?: string;
}) {
  const card = (
    <Card className={cn("rounded-2xl border bg-panel py-4", href && "transition-colors hover:border-ink-500")}>
      <CardContent className="flex flex-col gap-1.5 px-4">
        <div className="text-[13.5px] text-ink-500">{label}</div>
        <div className={cn("text-stat-value font-extrabold tracking-tight text-ink-100", valueClassName)}>{value}</div>
        {note && <div className="text-[12.5px] text-ink-500">{note}</div>}
      </CardContent>
    </Card>
  );

  return href ? <Link href={href}>{card}</Link> : card;
}
