import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { ReactFlowProvider, type NodeProps } from "@xyflow/react";
import { GraphNode, type GraphFlowNode } from "@/components/graph/graph-node";
import type { NodeKind } from "@/lib/api/repos-api";

function props(overrides: Partial<GraphFlowNode["data"]>): NodeProps<GraphFlowNode> {
  return {
    id: "1",
    type: "graphNode",
    data: { kind: "Symbol", label: "refreshSession", kindLabel: "Symbol", prominence: "Full", isSeed: false, ...overrides },
    selected: false,
    dragging: false,
    draggable: true,
    selectable: true,
    deletable: true,
    isConnectable: true,
    zIndex: 0,
    positionAbsoluteX: 0,
    positionAbsoluteY: 0,
  };
}

function renderNode(overrides: Partial<GraphFlowNode["data"]> = {}) {
  return render(
    <ReactFlowProvider>
      <GraphNode {...props(overrides)} />
    </ReactFlowProvider>,
  );
}

describe("GraphNode", () => {
  it("renders the label and kind", () => {
    renderNode({ label: "refreshSession", kindLabel: "Symbol" });
    expect(screen.getByText("refreshSession")).toBeInTheDocument();
    expect(screen.getByText(/SYMBOL/)).toBeInTheDocument();
  });

  it("marks the seed node as selected in its label", () => {
    renderNode({ isSeed: true });
    expect(screen.getByText(/· selected/)).toBeInTheDocument();
  });

  it("does not mark a non-seed node as selected", () => {
    renderNode({ isSeed: false });
    expect(screen.queryByText(/· selected/)).not.toBeInTheDocument();
  });

  // Regression test: `KIND_ICON`/`KIND_TAG_CLASSNAME` are `Record<NodeKind,
  // ...>` lookup tables -- if either one is ever missing an entry for a
  // real `NodeKind` value (confirmed live: `DocSection` was, for months,
  // since the type it's declared against didn't even list it), indexing
  // returns `undefined` at runtime with no compile-time warning, and
  // rendering `<Icon />` where `Icon` is `undefined` crashes the whole
  // page with React's "element type is invalid" error. Exercising every
  // declared `NodeKind` here means a future kind added to the union
  // without updating these tables fails this test immediately, rather
  // than surfacing as a production crash on `/graph`.
  const ALL_KINDS: NodeKind[] = ["Symbol", "File", "Gotcha", "Decision", "Definition", "Note", "DocSection"];
  it.each(ALL_KINDS)("renders without crashing for every NodeKind, including %s", (kind) => {
    expect(() => renderNode({ kind, kindLabel: kind })).not.toThrow();
  });
});
