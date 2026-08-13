import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { ReactFlowProvider, type NodeProps } from "@xyflow/react";
import { GraphNode, type GraphFlowNode } from "@/components/graph/graph-node";

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
});
