import type { LucideIcon } from "lucide-react";
import { cn } from "@/lib/utils";

export function EmptyState({
  icon: Icon,
  title,
  description,
  className,
}: {
  icon: LucideIcon;
  title: string;
  description?: string;
  className?: string;
}) {
  return (
    <div className={cn("flex flex-col items-center gap-2 rounded-md border border-dashed border-border-strong py-12 text-center", className)}>
      <Icon className="size-6 text-ink-500" />
      <p className="text-section font-medium text-ink-100">{title}</p>
      {description && <p className="max-w-sm text-body text-ink-500">{description}</p>}
    </div>
  );
}
