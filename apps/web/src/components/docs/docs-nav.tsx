import { ChevronDown } from "lucide-react";
import type { DocGroup, DocSection, NodeKind } from "@/lib/api/repos-api";
import { cn } from "@/lib/utils";

const GROUP_LABELS: Record<DocGroup, string> = {
  repository: "Repository",
  core_modules: "Core Modules",
  knowledge: "Knowledge",
  setup: "Setup",
};

// Matches `knowledge-callout.tsx`'s `CALLOUT_STYLE` kind → color mapping
// (badge variant, not imported directly, since this only needs the
// border/bg/text classes, not the icon).
const BADGE_CLASSNAME: Record<NodeKind, string> = {
  Gotcha: "border-node-gotcha/30 bg-node-gotcha/10 text-node-gotcha",
  Decision: "border-node-decision/30 bg-node-decision/10 text-node-decision",
  Note: "border-node-note/30 bg-node-note/10 text-node-note",
  Symbol: "border-node-symbol/30 bg-node-symbol/10 text-node-symbol",
  File: "border-node-file/30 bg-node-file/10 text-node-file",
  Definition: "border-node-symbol/30 bg-node-symbol/10 text-node-symbol",
};

// No `execution_flows` entry -- ships as an omitted nav group for v1, see
// `DocGroup`'s own doc comment.
const GROUP_ORDER: DocGroup[] = ["repository", "core_modules", "knowledge", "setup"];

/** Left 220px pane: `DocSection[]` grouped by `DocGroup`, in a fixed group
 * order. A group with zero sections (e.g. `knowledge` on a repo with no
 * gotchas/decisions yet) renders nothing, matching the backend's own
 * "never fabricate an empty section" convention. Each item is its own
 * "page" -- `onSelectSection` switches which section renders in the center
 * pane, it doesn't scroll to an anchor within one long document.
 *
 * A section made entirely of knowledge callouts (Notes/Known Gotchas/
 * Architectural Decisions) expands into a book-index-style sub-list of
 * every individual entry once it's the active section -- only the active
 * one expands, so a repo with 30+ gotchas doesn't dump every title into
 * the nav at once. Picking a sub-item opens straight to that entry in the
 * center pane, skipping its own list view. */
export function DocsNav({
  sections,
  activeSectionId,
  activeItemIndex,
  onSelectSection,
  onSelectItem,
}: {
  sections: DocSection[];
  activeSectionId: string;
  activeItemIndex: number | null;
  onSelectSection: (id: string) => void;
  onSelectItem: (sectionId: string, index: number) => void;
}) {
  const byGroup = GROUP_ORDER.map((group) => ({ group, sections: sections.filter((s) => s.group === group) })).filter((g) => g.sections.length > 0);

  return (
    <nav className="h-full w-[264px] shrink-0 overflow-y-auto border-r border-border-strong bg-panel px-2 py-3">
      {byGroup.map(({ group, sections: groupSections }) => (
        <div key={group}>
          <p className="mb-0.5 mt-3 px-2 text-label uppercase tracking-wide text-ink-500 first:mt-0">{GROUP_LABELS[group]}</p>
          {groupSections.map((section) => {
            const isActive = activeSectionId === section.id;
            const isAllCallouts = section.blocks.length > 0 && section.blocks.every((b) => b.block_type === "knowledge_callout");
            const expanded = isActive && isAllCallouts;

            return (
              <div key={section.id}>
                <button
                  type="button"
                  onClick={() => onSelectSection(section.id)}
                  className={cn(
                    "flex w-full items-center gap-1.5 rounded-md px-2 py-1 text-left text-section transition-colors",
                    isActive ? "bg-primary/10 text-primary" : "text-ink-400 hover:bg-white/5 hover:text-ink-100",
                  )}
                >
                  <span className="min-w-0 flex-1 truncate">{section.title}</span>
                  {isAllCallouts && section.blocks[0]?.block_type === "knowledge_callout" && (
                    <span className={cn("shrink-0 rounded border px-1 py-0.5 text-label", BADGE_CLASSNAME[section.blocks[0].kind])}>{section.blocks.length}</span>
                  )}
                  {isAllCallouts && <ChevronDown className={cn("size-3 shrink-0 transition-transform", expanded ? "rotate-180" : "rotate-0")} />}
                </button>

                {expanded && (
                  <div className="ml-2 flex flex-col gap-0.5 border-l border-border-strong py-1 pl-2">
                    {section.blocks.map(
                      (block, i) =>
                        block.block_type === "knowledge_callout" && (
                          <button
                            key={i}
                            type="button"
                            onClick={() => onSelectItem(section.id, i)}
                            className={cn(
                              "truncate rounded px-1.5 py-0.5 text-left text-label transition-colors",
                              activeItemIndex === i ? "text-primary" : "text-ink-500 hover:text-ink-200",
                            )}
                            title={block.title}
                          >
                            {block.title}
                          </button>
                        ),
                    )}
                  </div>
                )}
              </div>
            );
          })}
        </div>
      ))}
    </nav>
  );
}
