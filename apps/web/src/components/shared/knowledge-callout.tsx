import { NodeBadge } from "@/components/shared/node-badge";
import { cn } from "@/lib/utils";

/** Inline gotcha/decision callout box -- shared between the doc viewer and the graph's floating callout cards. */
export function KnowledgeCallout({
  kind,
  relation,
  target,
  children,
  className,
}: {
  kind: "Gotcha" | "Decision";
  relation?: string;
  target?: string;
  children: React.ReactNode;
  className?: string;
}) {
  const borderClass = kind === "Gotcha" ? "border-node-gotcha/40 bg-node-gotcha/5" : "border-node-decision/40 bg-node-decision/5";
  return (
    <div className={cn("rounded-md border-l-4 p-3", borderClass, className)}>
      <div className="mb-1 flex items-center gap-2">
        <NodeBadge kind={kind} />
        {relation && target && (
          <span className="text-mono-code text-ink-500">
            {relation} {target}
          </span>
        )}
      </div>
      <div className="text-body text-ink-100">{children}</div>
    </div>
  );
}
