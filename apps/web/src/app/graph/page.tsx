"use client";

import { useMemo, useState } from "react";

type Node = {
  id: number;
  kind: "File" | "Symbol" | "Gotcha" | "Decision";
  repo: string;
  path: string | null;
  name: string | null;
  start_line: number | null;
  end_line: number | null;
  content: string | null;
};

type Edge = {
  id: number;
  src_id: number;
  dst_id: number;
  relation: "DependsOn" | "Documents" | "Affects";
};

const API_BASE = process.env.NEXT_PUBLIC_AGENTOPS_API_URL || "http://127.0.0.1:8420";

export default function GraphPage() {
  const [path, setPath] = useState("");
  const [nodes, setNodes] = useState<Node[] | null>(null);
  const [edges, setEdges] = useState<Edge[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [search, setSearch] = useState("");

  const load = (repoPath: string) => {
    if (!repoPath.trim()) return;
    setLoading(true);
    setError(null);
    const url = new URL("/graph", API_BASE);
    url.searchParams.set("path", repoPath.trim());

    fetch(url.toString())
      .then(async (res) => {
        const data = await res.json();
        if (!res.ok) throw new Error(data.error || `agentops-api returned ${res.status}`);
        return data;
      })
      .then((data) => {
        setNodes(data.nodes);
        setEdges(data.edges);
      })
      .catch((e) => setError(e instanceof Error ? e.message : String(e)))
      .finally(() => setLoading(false));
  };

  const nodesById = useMemo(() => {
    const map = new Map<number, Node>();
    nodes?.forEach((n) => map.set(n.id, n));
    return map;
  }, [nodes]);

  const notes = useMemo(() => nodes?.filter((n) => n.kind === "Gotcha" || n.kind === "Decision") ?? [], [nodes]);

  const affectsFor = (noteId: number): Node[] =>
    (edges ?? [])
      .filter((e) => e.src_id === noteId && e.relation === "Affects")
      .map((e) => nodesById.get(e.dst_id))
      .filter((n): n is Node => !!n);

  const filteredNodes = useMemo(() => {
    if (!nodes) return [];
    const q = search.trim().toLowerCase();
    const symbolsAndFiles = nodes.filter((n) => n.kind === "Symbol" || n.kind === "File");
    if (!q) return symbolsAndFiles;
    return symbolsAndFiles.filter((n) => (n.name ?? "").toLowerCase().includes(q) || (n.path ?? "").toLowerCase().includes(q));
  }, [nodes, search]);

  return (
    <main className="mx-auto max-w-4xl px-8 py-16">
      <h1 className="text-2xl font-semibold">Neuron / graph browser</h1>
      <p className="mt-2 text-zinc-600 dark:text-zinc-400">
        The indexed code graph — symbols, files, and every gotcha/decision node edge-connected to the exact symbol it affects
        (Codebrain-2). Read from <code className="text-sm">agentops-graph</code> via <code className="text-sm">agentops-api</code>.
      </p>

      <form
        className="mt-6 flex gap-2"
        onSubmit={(e) => {
          e.preventDefault();
          load(path);
        }}
      >
        <input
          type="text"
          value={path}
          onChange={(e) => setPath(e.target.value)}
          placeholder="absolute path to an already-scanned repo"
          className="flex-1 rounded-md border border-black/[.08] bg-transparent px-3 py-2 text-sm font-mono dark:border-white/[.145]"
        />
        <button
          type="submit"
          className="rounded-md bg-foreground px-4 py-2 text-sm text-background transition-colors hover:bg-[#383838] dark:hover:bg-[#ccc]"
        >
          Load
        </button>
      </form>

      {loading && <p className="mt-6 text-sm text-zinc-500">Loading…</p>}
      {error && (
        <div className="mt-6 rounded-md border border-red-500/30 bg-red-500/5 p-4 text-sm text-red-700 dark:text-red-400">
          {error}
        </div>
      )}

      {!loading && !error && nodes && (
        <>
          <div className="mt-6 flex gap-6 text-sm text-zinc-500">
            <span>{nodes.filter((n) => n.kind === "File").length} files</span>
            <span>{nodes.filter((n) => n.kind === "Symbol").length} symbols</span>
            <span>{nodes.filter((n) => n.kind === "Gotcha").length} gotchas</span>
            <span>{nodes.filter((n) => n.kind === "Decision").length} decisions</span>
          </div>

          {notes.length > 0 && (
            <section className="mt-8">
              <h2 className="text-sm font-medium text-zinc-500">Gotchas &amp; decisions</h2>
              <div className="mt-3 space-y-4">
                {notes.map((note) => (
                  <div key={note.id} className="rounded-md border border-black/[.08] p-4 dark:border-white/[.145]">
                    <div className="flex items-center gap-2">
                      <span
                        className={
                          "rounded px-2 py-0.5 text-xs " +
                          (note.kind === "Gotcha"
                            ? "bg-amber-500/10 text-amber-700 dark:text-amber-400"
                            : "bg-blue-500/10 text-blue-700 dark:text-blue-400")
                        }
                      >
                        {note.kind}
                      </span>
                      <span className="font-medium">{note.name}</span>
                    </div>
                    <p className="mt-2 text-sm text-zinc-600 dark:text-zinc-400">{note.content}</p>
                    {affectsFor(note.id).map((target) => (
                      <div key={target.id} className="mt-2 text-xs text-zinc-500">
                        → affects{" "}
                        <code className="rounded bg-black/[.03] px-1.5 py-0.5 dark:bg-white/[.05]">{target.name}</code> in{" "}
                        <code className="rounded bg-black/[.03] px-1.5 py-0.5 dark:bg-white/[.05]">{target.path}</code>
                      </div>
                    ))}
                  </div>
                ))}
              </div>
            </section>
          )}

          <section className="mt-8">
            <h2 className="text-sm font-medium text-zinc-500">Files &amp; symbols</h2>
            <input
              type="text"
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              placeholder="filter by name or path…"
              className="mt-3 w-full rounded-md border border-black/[.08] bg-transparent px-3 py-2 text-sm dark:border-white/[.145]"
            />
            <div className="mt-3 max-h-96 overflow-y-auto">
              <table className="w-full border-collapse text-sm">
                <thead>
                  <tr className="border-b border-black/[.08] text-left dark:border-white/[.145]">
                    <th className="py-2 pr-4 font-medium text-zinc-500">Kind</th>
                    <th className="py-2 pr-4 font-medium text-zinc-500">Name</th>
                    <th className="py-2 pr-4 font-medium text-zinc-500">Path</th>
                  </tr>
                </thead>
                <tbody>
                  {filteredNodes.map((n) => (
                    <tr key={n.id} className="border-b border-black/[.05] dark:border-white/[.08]">
                      <td className="py-1.5 pr-4 text-zinc-500">{n.kind}</td>
                      <td className="py-1.5 pr-4 font-mono">{n.name ?? "—"}</td>
                      <td className="py-1.5 pr-4 text-zinc-500">{n.path ?? "—"}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </section>
        </>
      )}
    </main>
  );
}
