import type { NodeKind } from "@/lib/api/repos-api";
import type { NavShape } from "@/lib/nav-config";

// The prototype's one global node-kind -> {color, shape} map (its own
// `this.KIND` object, reused verbatim across search results, connected-node
// rows, gotchas rows, and the sidebar's Gotchas nav item) -- a colored
// circle/square/diamond marker, never a lucide icon, wherever a node kind
// needs a visual tag. `Definition` and `DocSection` have no prototype
// entries of their own (not user-filterable kinds); they borrow the
// nearest existing token, matching the precedent `KIND_TAG_CLASSNAME`
// already set.
export const NODE_KIND_SHAPE: Record<NodeKind, { color: string; shape: NavShape }> = {
  File: { color: "var(--node-file)", shape: "square" },
  Symbol: { color: "var(--node-symbol)", shape: "circle" },
  Gotcha: { color: "var(--node-gotcha)", shape: "diamond" },
  Decision: { color: "var(--node-decision)", shape: "diamond" },
  Note: { color: "var(--node-note)", shape: "diamond" },
  Definition: { color: "var(--node-symbol)", shape: "circle" },
  DocSection: { color: "var(--node-file)", shape: "square" },
};

/**
 * Safe accessor -- bracket-indexing `NODE_KIND_SHAPE` directly and
 * destructuring the result throws if `kind` is ever a value this map
 * doesn't cover (the old badge-className lookup this replaced was a `cn()`
 * call, which silently no-ops on `undefined`; a raw object/array
 * destructure has no such tolerance). The backend's `NodeKind` enum should
 * always match this map 1:1, but call sites render data the frontend
 * doesn't control end-to-end (connected-node relations, stored vault
 * content) -- falling back to a neutral ink-colored circle here means a
 * genuinely unexpected value degrades to an unstyled marker instead of
 * crashing the whole detail panel mid-render.
 */
export function nodeKindShape(kind: NodeKind): { color: string; shape: NavShape } {
  return NODE_KIND_SHAPE[kind] ?? { color: "var(--ink-500)", shape: "circle" };
}
