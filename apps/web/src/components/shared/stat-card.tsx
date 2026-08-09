import Link from "next/link";
import type { LucideIcon } from "lucide-react";
import { Card, CardContent } from "@/components/ui/card";
import { cn } from "@/lib/utils";

export function StatCard({
  label,
  value,
  icon: Icon,
  valueClassName,
  href,
}: {
  label: string;
  value: string | number;
  icon: LucideIcon;
  valueClassName?: string;
  /** When set, the whole card links out (e.g. to a filtered graph view) instead of being purely decorative. */
  href?: string;
}) {
  const card = (
    <Card className={cn("border-border-strong bg-panel py-4", href && "transition-colors hover:border-border-strong hover:bg-raised")}>
      <CardContent className="flex flex-col gap-2 px-4">
        <div className="flex items-center gap-2 text-section text-ink-500">
          <Icon className="size-4" />
          {label}
        </div>
        <div className={cn("text-2xl font-bold text-ink-100", valueClassName)}>{value}</div>
      </CardContent>
    </Card>
  );

  if (!href) return card;
  return (
    <Link href={href} className="block rounded-md focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring">
      {card}
    </Link>
  );
}
