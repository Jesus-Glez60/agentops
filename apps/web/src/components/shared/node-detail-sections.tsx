import { TriangleAlert } from "lucide-react";
import type { ConnectedNode, NodeDetail } from "@/lib/api/agentops-api";
import { connectedNodeLabel, kindLabel, relationText } from "@/lib/node-detail-formatting";
import { languageFromPath } from "@/lib/language-from-path";
import { CodeBlock } from "@/components/shared/code-block";
import { CopyButton } from "@/components/shared/copy-button";
import { Prose } from "@/components/shared/prose";
import { ConnectedNodeRow } from "@/components/search/connected-node-row";

/**
 * Content / Source Location / Connected Nodes -- the three sections shared
 * verbatim between the search page's detail panel and the gotchas page's
 * detail panel, extracted here so neither copy-pastes the other's JSX.
 */
export function NodeDetailSections({
  detail,
  branch,
  onSelectConnected,
  splitKnowledge = false,
}: {
  detail: NodeDetail;
  branch: string | null | undefined;
  onSelectConnected: (node: ConnectedNode) => void;
  /** When true, partitions `connected` into an "Attached knowledge" section
   * (Gotcha/Decision) shown before "Relationships" (everything else) --
   * the Knowledge Graph screen's layout. Default false preserves the
   * single flat "Connected nodes" list the search and gotchas pages
   * already render, unchanged. */
  splitKnowledge?: boolean;
}) {
  const knowledgeNodes = splitKnowledge ? detail.connected.filter((n) => n.kind === "Gotcha" || n.kind === "Decision") : [];
  const otherNodes = splitKnowledge ? detail.connected.filter((n) => n.kind !== "Gotcha" && n.kind !== "Decision") : detail.connected;

  return (
    <>
      {/* Never hidden -- a Reduced-prominence node is still fully real
          knowledge, just ranked lower elsewhere; the reason is shown
          wherever the node itself is shown, same as the MCP tools/docgen
          annotations, so the demotion is never silent. */}
      {detail.prominence === "Reduced" && (
        <div className="flex items-start gap-2 rounded-md border border-curation-reduced/40 bg-curation-reduced/10 p-3 text-body text-ink-300">
          <TriangleAlert className="mt-0.5 size-4 shrink-0 text-curation-reduced" />
          <div>
            <p className="font-medium text-ink-100">Reduced prominence</p>
            <p>{detail.curation_reason ?? "No reason recorded."}</p>
          </div>
        </div>
      )}

      {detail.content && (
        <div className="flex flex-col gap-2">
          <p className="text-label uppercase tracking-wide text-ink-500">Content</p>
          {detail.start_line !== null ? (
            <CodeBlock
              code={detail.content}
              title={`${detail.path ?? ""}:${detail.start_line}${detail.end_line ? `-${detail.end_line}` : ""}`}
              language={languageFromPath(detail.path)}
            />
          ) : (
            // Vault-backed content (Gotcha/Decision/Note) has no source
            // line range -- it's hand-written Markdown, not code, so it
            // renders bold/inline-code spans instead of a non-wrapping
            // <pre> code block or literal ** / ` tokens.
            <div className="rounded-md border border-border-strong bg-raised p-4">
              <Prose text={detail.content} className="text-body leading-relaxed text-ink-300" />
            </div>
          )}
        </div>
      )}

      {detail.path && (
        <div className="flex flex-col gap-2">
          <p className="text-label uppercase tracking-wide text-ink-500">Source location</p>
          <div className="flex flex-col gap-1 rounded-md border border-border-strong bg-raised px-3 py-2">
            <div className="flex items-center justify-between gap-2">
              <p className="min-w-0 truncate text-mono-path text-slate-blue">
                {detail.repo} / {detail.path}
                {detail.start_line ? `:${detail.start_line}${detail.end_line ? `-${detail.end_line}` : ""}` : ""}
              </p>
              <CopyButton value={`${detail.repo}/${detail.path}`} label="" className="h-6 w-6 shrink-0 justify-center border-0 p-0" />
            </div>
            {/* Real branch from the same /repos data the Overview page and topbar
                already fetch -- never fabricated when a repo's branch is unknown. */}
            {branch && <p className="text-mono-path text-ink-500">branch: {branch}</p>}
          </div>
        </div>
      )}

      {knowledgeNodes.length > 0 && (
        <div className="flex flex-col gap-2">
          <p className="text-label uppercase tracking-wide text-ink-500">Attached knowledge</p>
          <div className="flex flex-col gap-2">
            {knowledgeNodes.map((node) => (
              <ConnectedNodeRow
                key={`${node.relation}:${node.id}`}
                relation={relationText(node.relation)}
                kind={node.kind}
                kindLabel={kindLabel(node.kind)}
                label={connectedNodeLabel(node)}
                onClick={() => onSelectConnected(node)}
              />
            ))}
          </div>
        </div>
      )}

      {otherNodes.length > 0 && (
        <div className="flex flex-col gap-2">
          <p className="text-label uppercase tracking-wide text-ink-500">{splitKnowledge ? "Relationships" : "Connected nodes"}</p>
          <div className="flex flex-col gap-2">
            {otherNodes.map((node) => (
              <ConnectedNodeRow
                key={`${node.relation}:${node.id}`}
                relation={relationText(node.relation)}
                kind={node.kind}
                kindLabel={kindLabel(node.kind)}
                label={connectedNodeLabel(node)}
                onClick={() => onSelectConnected(node)}
              />
            ))}
          </div>
        </div>
      )}
    </>
  );
}
