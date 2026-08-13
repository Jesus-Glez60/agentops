import type { DocBlock, DocPage, DocSection } from "@/lib/api/agentops-api";
import { Prose } from "@/components/shared/prose";
import { SymbolTable } from "@/components/docs/symbol-table";
import { DependencyChips } from "@/components/docs/dependency-chips";
import { KnowledgeCallout } from "@/components/docs/knowledge-callout";
import { KnowledgeSectionBrowser } from "@/components/docs/knowledge-section-browser";

/** Center pane: the repo title block (always shown) plus exactly one
 * section's blocks -- each nav item is its own "page" within the doc, not
 * an anchor scrolled to within one long document, so a repo with a lot of
 * generated content (this one, easily 1000+ symbols) never forces a giant
 * scrollable blob. */
export function DocsContent({
  page,
  section,
  selectedItem,
  onSelectItem,
  onViewInGraph,
}: {
  page: DocPage;
  section: DocSection;
  /** Which callout is open within an all-callout section (`null` = list view) -- lifted to `DocsPageInner` so the nav's book-index sub-list drives the same state. */
  selectedItem: number | null;
  onSelectItem: (index: number | null) => void;
  onViewInGraph?: (nodeId: number) => void;
}) {
  // A Notes/Known Gotchas/Architectural Decisions section is entirely
  // knowledge-callout blocks -- a list of individually-long entries, not a
  // page of prose -- so it gets the list+detail browser instead of every
  // entry stacked in one scroll. `key={section.id}` (set at the call site
  // below) resets the browser back to its list view whenever the nav
  // switches to a different section.
  const isAllCallouts = section.blocks.length > 0 && section.blocks.every((b): b is Extract<DocBlock, { block_type: "knowledge_callout" }> => b.block_type === "knowledge_callout");

  return (
    <div className="mx-auto max-w-[700px] px-8 pb-16 pt-8">
      <div className="mb-2 flex items-center gap-2 text-mono-code text-ink-500">
        <span>{page.repo}</span>
        <span>·</span>
        <span>
          Generated from graph centrality · <span className="text-node-decision">Indexed {page.generated_at}</span>
        </span>
      </div>
      <h1 className="mb-3 text-page-title font-bold text-ink-100">{page.repo}</h1>

      <h2 className="mb-2 mt-7 text-subheading font-semibold text-ink-100">{section.title}</h2>

      {isAllCallouts ? (
        <KnowledgeSectionBrowser
          blocks={section.blocks as Extract<DocBlock, { block_type: "knowledge_callout" }>[]}
          selected={selectedItem}
          onSelect={onSelectItem}
          onViewInGraph={onViewInGraph}
        />
      ) : (
        section.blocks.map((block, i) => {
          switch (block.block_type) {
            case "prose":
              return <Prose key={i} text={block.markdown} className="mb-3 text-body leading-relaxed text-ink-300" />;
            case "symbol_table":
              return <SymbolTable key={i} block={block} />;
            case "dependency_chips":
              return <DependencyChips key={i} block={block} />;
            case "knowledge_callout":
              return <KnowledgeCallout key={i} block={block} onViewInGraph={onViewInGraph} />;
            default:
              return null;
          }
        })
      )}
    </div>
  );
}
