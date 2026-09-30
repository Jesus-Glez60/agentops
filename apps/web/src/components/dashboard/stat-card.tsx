import type { LucideIcon } from "lucide-react";
import Link from "next/link";
import { Card, CardContent } from "@/components/ui/card";
import { cn } from "@/lib/utils";

export function StatCard({
  label,
  value,
  icon: Icon,
  valueClassName,
  href,
  note,
}: {
  label: string;
  value: string | number;
  icon: LucideIcon;
  valueClassName?: string;
  /** When set, the whole card is a real link -- e.g. to the gotchas page's actual review queue, not a "coming soon" placeholder. */
  href?: string;
  /** Small descriptive line under the value, matching the prototype's stat-strip notes (e.g. "symbols, files, gotchas, decisions"). */
  note?: string;
}) {
  const card = (
    <Card className={cn("border-border-strong bg-panel py-4", href && "transition-colors hover:border-border-strong/80")}>
      <CardContent className="flex flex-col gap-1.5 px-4">
        <div className="flex items-center gap-2 text-[13.5px] text-ink-500">
          <Icon className="size-4" />
          {label}
        </div>
        <div className={cn("text-[34px] leading-none font-extrabold tracking-tight text-ink-100", valueClassName)}>{value}</div>
        {note && <div className="text-[12.5px] text-ink-500">{note}</div>}
      </CardContent>
    </Card>
  );

  return href ? <Link href={href}>{card}</Link> : card;
}
