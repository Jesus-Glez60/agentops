import { ArrowRight } from "lucide-react";
import { cn } from "@/lib/utils";

export function RelationshipChip({
  relation,
  target,
  onClick,
  className,
}: {
  relation: string;
  target: string;
  onClick?: () => void;
  className?: string;
}) {
  const Tag = onClick ? "button" : "span";
  return (
    <Tag
      onClick={onClick}
      className={cn(
        "inline-flex items-center gap-1 rounded-md border border-border-strong bg-raised px-2 py-1 text-mono-code text-ink-300",
        onClick && "cursor-pointer transition-colors hover:border-primary/50 hover:text-ink-100",
        className,
      )}
    >
      <ArrowRight className="size-3 text-ink-500" />
      <span className="text-ink-500">{relation}</span>
      {target}
    </Tag>
  );
}
