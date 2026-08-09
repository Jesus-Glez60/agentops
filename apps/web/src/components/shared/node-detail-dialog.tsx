import { NodeBadge } from "@/components/shared/node-badge";
import { CodeBlock } from "@/components/shared/code-block";
import { DocContent } from "@/components/docs/doc-content";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import type { GraphNode } from "@/lib/api/types";

function isProseNode(kind: GraphNode["kind"]): boolean {
  return kind === "Gotcha" || kind === "Decision";
}

/**
 * Full-content view of a single graph node, shared between the Knowledge
 * Graph's node detail panel and the Documentation page's symbol browser --
 * one dialog implementation instead of two, so "expand a gotcha/decision to
 * read the whole thing" behaves identically everywhere it appears.
 */
export function NodeDetailDialog({ node, open, onOpenChange }: { node: GraphNode | null; open: boolean; onOpenChange: (open: boolean) => void }) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      {/* DialogContent's base classes already set `sm:max-w-sm`; a plain
          `max-w-3xl` loses to that at >=640px since the sm: variant is later
          in the generated stylesheet. Overriding the sm: variant directly is
          what actually wins the cascade. */}
      <DialogContent className="max-h-[85vh] w-full max-w-[calc(100%-2rem)] overflow-y-auto sm:max-w-3xl">
        {node && (
          <>
            <DialogHeader>
              <DialogTitle className="flex items-center gap-2">
                <NodeBadge kind={node.kind} />
                {node.name ?? node.path ?? `#${node.id}`}
              </DialogTitle>
            </DialogHeader>
            {node.content ? (
              isProseNode(node.kind) ? (
                <DocContent markdown={node.content} />
              ) : (
                <CodeBlock code={node.content} />
              )
            ) : (
              <p className="text-body text-ink-500">No content recorded.</p>
            )}
          </>
        )}
      </DialogContent>
    </Dialog>
  );
}
