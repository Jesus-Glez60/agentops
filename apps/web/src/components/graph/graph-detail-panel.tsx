"use client";

import { useRouter } from "next/navigation";
import { BookOpen, SearchIcon, Target, X } from "lucide-react";
import type { ConnectedNode, NodeDetail } from "@/lib/api/agentops-api";
import { kindLabel } from "@/lib/node-detail-formatting";
import { KIND_TAG_CLASSNAME } from "@/lib/node-kind-colors";
import { Button } from "@/components/ui/button";
import { CopyButton } from "@/components/shared/copy-button";
import { NodeDetailSections } from "@/components/shared/node-detail-sections";
import { cn } from "@/lib/utils";

export function GraphDetailPanel({
  detail,
  branch,
  isSeed,
  onSelectConnected,
  onClose,
  onCenter,
}: {
  detail: NodeDetail | undefined;
  branch: string | null | undefined;
  /** Hides "Center here" when the inspected node is already the seed -- re-centering on itself is a no-op. */
  isSeed: boolean;
  onSelectConnected: (node: ConnectedNode) => void;
  onClose: () => void;
  onCenter: () => void;
}) {
  const router = useRouter();

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex shrink-0 items-center justify-between border-b border-border-strong px-4 py-3">
        {detail ? (
          <div className="flex items-center gap-2">
            <span className={cn("inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-mono-code uppercase", KIND_TAG_CLASSNAME[detail.kind])}>{kindLabel(detail.kind)}</span>
            <span className="text-section font-semibold text-ink-100">{detail.name ?? detail.path ?? `Node ${detail.id}`}</span>
          </div>
        ) : (
          <span className="text-section text-ink-500">Loading…</span>
        )}
        <div className="flex shrink-0 items-center gap-1">
          {detail && !isSeed && (
            <button onClick={onCenter} title="Center graph on this node" className="flex size-6 items-center justify-center rounded text-ink-500 hover:text-ink-100">
              <Target className="size-3.5" />
            </button>
          )}
          <button onClick={onClose} aria-label="Close" className="flex size-6 items-center justify-center rounded text-ink-500 hover:text-ink-100">
            <X className="size-4" />
          </button>
        </div>
      </div>

      <div className="flex flex-1 flex-col gap-4 overflow-y-auto p-4 text-body">
        {!detail && <p className="text-body text-ink-500">Loading details…</p>}
        {detail && (
          <>
            <NodeDetailSections detail={detail} branch={branch} onSelectConnected={onSelectConnected} splitKnowledge />
            <div className="flex gap-2 pt-1">
              <Button
                size="sm"
                variant="outline"
                className="flex-1 gap-1.5"
                onClick={() => router.push(`/search?q=${encodeURIComponent(detail.name ?? detail.path ?? "")}`)}
              >
                <SearchIcon className="size-3.5 text-ink-400" />
                Search
              </Button>
              <Button size="sm" variant="outline" className="flex-1 gap-1.5" onClick={() => router.push("/docs")}>
                <BookOpen className="size-3.5 text-ink-400" />
                Docs
              </Button>
              <CopyButton
                value={detail.path ? `${detail.repo}/${detail.path}${detail.start_line ? `:${detail.start_line}` : ""}` : detail.repo}
                label="Source"
                className="flex-1 justify-center"
              />
            </div>
          </>
        )}
      </div>
    </div>
  );
}
