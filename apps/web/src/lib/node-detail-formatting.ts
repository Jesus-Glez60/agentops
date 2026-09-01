import type { NodeKind } from "@/lib/api/repos-api";

// "Docs" has no dedicated NodeKind of its own -- it maps display-wise to
// Note. Shared by the search page and the gotchas page so both label a
// Note-kind node the same way.
export function kindLabel(kind: NodeKind): string {
  if (kind === "Note") return "Docs";
  if (kind === "DocSection") return "Doc section";
  return kind;
}

// Symbol names alone are frequently ambiguous (every Next.js route handler
// is named `POST`/`GET`) -- trail the last couple of path segments so the
// row reads as e.g. "POST · auth/login/route.ts" instead of just "POST".
export function connectedNodeLabel(node: { name: string | null; path: string | null; id: number }): string {
  const label = node.name ?? `#${node.id}`;
  if (!node.path) return label;
  const segments = node.path.split("/").filter(Boolean);
  const context = segments.slice(-2).join("/");
  return node.name ? `${label} · ${context}` : context;
}

// The backend prefixes an incoming edge's label with "← " (see search.rs's
// relation_label) -- strip it for display since ConnectedNodeRow already has
// its own directional arrow icon; keep the underlying word for the relation
// column.
export function relationText(relation: string): string {
  return relation.replace(/^←\s*/, "");
}
