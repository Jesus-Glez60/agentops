import type { NodeKind } from "@/lib/api/repos-api";

// Same --node-* tokens search-filters.tsx's pills use, so a node's kind tag
// always matches the color of the filter pill that would isolate it --
// shared by search-result-card.tsx and connected-node-row.tsx so both kind
// tags on the search page agree.
export const KIND_TAG_CLASSNAME: Record<NodeKind, string> = {
  Symbol: "border-node-symbol/40 text-node-symbol",
  File: "border-node-file/40 text-node-file",
  Gotcha: "border-node-gotcha/40 text-node-gotcha",
  Decision: "border-node-decision/40 text-node-decision",
  Definition: "border-node-symbol/40 text-node-symbol",
  Note: "border-node-note/40 text-node-note",
  // No dedicated --node-doc-section token exists -- reuses File's neutral
  // tone, same "borrow the nearest existing token" precedent Definition
  // already sets above rather than growing the design-token set for a
  // node kind that isn't user-filterable (see GRAPH_FILTERABLE_KINDS).
  DocSection: "border-node-file/40 text-node-file",
};
