"use client";

import { useMemo, useState } from "react";
import Link from "next/link";
import { ChevronRight, FunctionSquare, Search, TriangleAlert, Workflow } from "lucide-react";
import type { GraphEdge, GraphNode } from "@/lib/api/types";
import { NotePreviewCard } from "@/components/shared/note-preview-card";
import { NodeDetailDialog } from "@/components/shared/node-detail-dialog";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { cn } from "@/lib/utils";

const DEFAULT_VISIBLE = 15;

interface FileEntry {
  path: string;
  symbols: GraphNode[];
}

/**
 * The actual "neurons and synapses" this product is built around: rather
 * than parsing docgen's flat markdown text for a per-file symbol list (the
 * previous approach), this reads the same `Affects` edges the Knowledge
 * Graph view uses -- Gotcha/Decision nodes edge-connected directly to the
 * Symbol they apply to (created via `agentops note --affects <symbol>`).
 * That's a real structural link, not a text-proximity guess, and it's what
 * makes a symbol's attached knowledge genuinely clickable here instead of
 * static text.
 *
 * Default ordering surfaces files with attached knowledge first (most
 * gotchas, then most decisions, then most symbols) -- "what's most worth
 * knowing" in a repo isn't just what's most-referenced, it's what has real
 * caught issues/decisions recorded against it.
 */
export function SymbolBrowser({ nodes, edges, repoPath }: { nodes: GraphNode[]; edges: GraphEdge[]; repoPath: string }) {
  const [query, setQuery] = useState("");
  const [showAll, setShowAll] = useState(false);
  const [expandedFiles, setExpandedFiles] = useState<Set<string>>(new Set());
  const [expandedSymbols, setExpandedSymbols] = useState<Set<number>>(new Set());
  const [openNote, setOpenNote] = useState<GraphNode | null>(null);

  const nodesById = useMemo(() => new Map(nodes.map((n) => [n.id, n])), [nodes]);

  // Affects edges point Gotcha/Decision (src) -> Symbol (dst) -- see
  // agentops-notes::add_note. Group by target symbol id for O(1) lookup per
  // symbol row instead of filtering the whole edge list per render.
  const notesBySymbolId = useMemo(() => {
    const map = new Map<number, GraphNode[]>();
    for (const e of edges) {
      if (e.relation !== "Affects") continue;
      const note = nodesById.get(e.src_id);
      if (!note || (note.kind !== "Gotcha" && note.kind !== "Decision")) continue;
      const list = map.get(e.dst_id) ?? [];
      list.push(note);
      map.set(e.dst_id, list);
    }
    return map;
  }, [edges, nodesById]);

  const files = useMemo((): FileEntry[] => {
    const symbolsByPath = new Map<string, GraphNode[]>();
    for (const n of nodes) {
      if (n.kind !== "Symbol" || !n.path) continue;
      const list = symbolsByPath.get(n.path) ?? [];
      list.push(n);
      symbolsByPath.set(n.path, list);
    }
    const entries: FileEntry[] = nodes
      .filter((n) => n.kind === "File" && n.path)
      .map((f) => ({ path: f.path as string, symbols: symbolsByPath.get(f.path as string) ?? [] }));

    function gotchaCountFor(entry: FileEntry): number {
      return entry.symbols.reduce((sum, s) => sum + (notesBySymbolId.get(s.id)?.length ?? 0), 0);
    }
    return entries.sort((a, b) => gotchaCountFor(b) - gotchaCountFor(a) || b.symbols.length - a.symbols.length);
  }, [nodes, notesBySymbolId]);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return files;
    return files.filter((f) => f.path.toLowerCase().includes(q) || f.symbols.some((s) => (s.name ?? "").toLowerCase().includes(q)));
  }, [files, query]);

  const isFiltering = query.trim().length > 0;
  const visible = isFiltering || showAll ? filtered : filtered.slice(0, DEFAULT_VISIBLE);

  function toggleFile(path: string) {
    setExpandedFiles((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  }
  function toggleSymbol(id: number) {
    setExpandedSymbols((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }

  return (
    <div className="flex flex-col gap-3">
      <div className="relative">
        <Search className="pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-ink-500" />
        <Input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder={`Search ${files.length} files or symbols…`}
          className="pl-8 font-mono text-mono-path"
        />
      </div>

      <div className="overflow-hidden rounded-md border border-border-strong">
        {visible.length === 0 && <p className="p-4 text-body text-ink-500">No files match &ldquo;{query}&rdquo;.</p>}
        {visible.map((file) => {
          const isOpen = expandedFiles.has(file.path);
          const gotchaCount = file.symbols.reduce((sum, s) => sum + (notesBySymbolId.get(s.id)?.length ?? 0), 0);
          return (
            <div key={file.path} className="border-b border-border-strong last:border-b-0">
              <button
                type="button"
                onClick={() => toggleFile(file.path)}
                className="flex w-full items-center gap-2 px-3 py-2 text-left hover:bg-raised"
              >
                <ChevronRight className={cn("size-3.5 shrink-0 text-ink-500 transition-transform", isOpen && "rotate-90")} />
                <span className="min-w-0 flex-1 truncate text-mono-path text-ink-100">{file.path}</span>
                {gotchaCount > 0 && (
                  <Badge variant="outline" className="shrink-0 gap-1 border-node-gotcha/40 text-node-gotcha">
                    <TriangleAlert className="size-3" />
                    {gotchaCount}
                  </Badge>
                )}
                <span className="shrink-0 text-mono-path text-ink-500">
                  {file.symbols.length} symbol{file.symbols.length === 1 ? "" : "s"}
                </span>
              </button>
              {isOpen && (
                <div className="border-t border-border-strong bg-raised">
                  {file.symbols.length === 0 && <p className="px-4 py-2 text-body text-ink-500 italic">No symbols extracted for this file.</p>}
                  {file.symbols.map((sym) => {
                    const notes = notesBySymbolId.get(sym.id) ?? [];
                    const symOpen = expandedSymbols.has(sym.id);
                    return (
                      <div key={sym.id} className="border-b border-border-strong/50 last:border-b-0">
                        <button
                          type="button"
                          onClick={() => toggleSymbol(sym.id)}
                          className="flex w-full items-center gap-2 px-4 py-1.5 text-left hover:bg-panel"
                        >
                          <ChevronRight
                            className={cn("size-3 shrink-0 text-ink-500 transition-transform", symOpen && "rotate-90")}
                          />
                          <FunctionSquare className="size-3.5 shrink-0 text-node-symbol" />
                          <span className="text-mono-code text-ink-100">{sym.name}()</span>
                          {sym.start_line != null && (
                            <span className="text-mono-path text-ink-500">
                              lines {sym.start_line}-{sym.end_line}
                            </span>
                          )}
                          {notes.length > 0 && (
                            <Badge variant="outline" className="ml-auto gap-1 border-node-gotcha/40 text-node-gotcha">
                              <TriangleAlert className="size-3" />
                              {notes.length}
                            </Badge>
                          )}
                        </button>
                        {symOpen && (
                          <div className="space-y-2 border-t border-border-strong/50 bg-panel px-4 py-2">
                            {notes.length === 0 && <p className="text-body text-ink-500 italic">No gotchas or decisions attached to this symbol.</p>}
                            {notes.map((note) => (
                              <NotePreviewCard key={note.id} node={note} onOpen={() => setOpenNote(note)} />
                            ))}
                            <Button asChild variant="ghost" size="sm" className="gap-1.5 text-ink-500">
                              <Link href={`/graph?path=${encodeURIComponent(repoPath)}&tab=impact&node=${sym.id}`}>
                                <Workflow className="size-3.5" />
                                View in Knowledge Graph
                              </Link>
                            </Button>
                          </div>
                        )}
                      </div>
                    );
                  })}
                </div>
              )}
            </div>
          );
        })}
      </div>

      {!isFiltering && !showAll && filtered.length > DEFAULT_VISIBLE && (
        <Button variant="outline" size="sm" onClick={() => setShowAll(true)} className="self-start">
          Show all {filtered.length} files
        </Button>
      )}
      {isFiltering && (
        <p className="text-mono-path text-ink-500">
          {filtered.length} of {files.length} files match.
        </p>
      )}

      <NodeDetailDialog node={openNote} open={openNote != null} onOpenChange={(o) => !o && setOpenNote(null)} />
    </div>
  );
}
