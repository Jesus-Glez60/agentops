"use client";

import Link from "next/link";
import { useEffect, useState } from "react";

const API_BASE = process.env.NEXT_PUBLIC_AGENTOPS_API_URL || "http://127.0.0.1:8420";

type RepoCounts = { files: number; symbols: number; gotchas: number; decisions: number };
type ScannedRepo = { path: string; last_scanned_at: number; counts: RepoCounts | null };

type Summary = RepoCounts & { hasDoc: boolean };

function formatLastScanned(unixSeconds: number): string {
  const diffMs = Date.now() - unixSeconds * 1000;
  const diffMins = Math.round(diffMs / 60000);
  if (diffMins < 1) return "just now";
  if (diffMins < 60) return `${diffMins}m ago`;
  const diffHours = Math.round(diffMins / 60);
  if (diffHours < 24) return `${diffHours}h ago`;
  return `${Math.round(diffHours / 24)}d ago`;
}

export default function ReposPage() {
  const [repos, setRepos] = useState<ScannedRepo[] | null>(null);
  const [reposError, setReposError] = useState<string | null>(null);

  const [path, setPath] = useState("");
  const [summary, setSummary] = useState<Summary | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  const loadRepoList = () => {
    fetch(new URL("/repos", API_BASE))
      .then(async (res) => {
        const data = await res.json();
        if (!res.ok) throw new Error(data.error || `agentops-api returned ${res.status}`);
        return data;
      })
      .then((data) => setRepos(data.repos))
      .catch((e) => setReposError(e instanceof Error ? e.message : String(e)));
  };

  useEffect(() => {
    loadRepoList();
  }, []);

  const load = (repoPath: string) => {
    if (!repoPath.trim()) return;
    setLoading(true);
    setError(null);
    setSummary(null);

    const graphUrl = new URL("/graph", API_BASE);
    graphUrl.searchParams.set("path", repoPath.trim());
    const docsUrl = new URL("/docs", API_BASE);
    docsUrl.searchParams.set("path", repoPath.trim());

    Promise.all([
      fetch(graphUrl.toString()).then(async (res) => {
        const data = await res.json();
        if (!res.ok) throw new Error(data.error || `agentops-api returned ${res.status}`);
        return data;
      }),
      fetch(docsUrl.toString()).then((res) => res.ok),
    ])
      .then(([graph, hasDoc]) => {
        const nodes = graph.nodes as { kind: string }[];
        setSummary({
          files: nodes.filter((n) => n.kind === "File").length,
          symbols: nodes.filter((n) => n.kind === "Symbol").length,
          gotchas: nodes.filter((n) => n.kind === "Gotcha").length,
          decisions: nodes.filter((n) => n.kind === "Decision").length,
          hasDoc,
        });
      })
      .catch((e) => setError(e instanceof Error ? e.message : String(e)))
      .finally(() => setLoading(false));
  };

  return (
    <main className="mx-auto max-w-3xl px-8 py-16">
      <h1 className="text-2xl font-semibold">Repo overview</h1>
      <p className="mt-2 text-zinc-600 dark:text-zinc-400">
        Every repo <code className="text-sm">agentops install</code> has scanned on this machine — reads live from{" "}
        <code className="text-sm">agentops-api</code>.
      </p>

      <div className="mt-6">
        {reposError && (
          <div className="rounded-md border border-red-500/30 bg-red-500/5 p-4 text-sm text-red-700 dark:text-red-400">{reposError}</div>
        )}
        {!reposError && repos === null && <p className="text-sm text-zinc-500">Loading…</p>}
        {!reposError && repos !== null && repos.length === 0 && (
          <p className="text-sm text-zinc-500">
            No repos scanned yet on this machine. Run <code className="text-sm">agentops install --path &lt;repo&gt;</code> to index one.
          </p>
        )}
        {!reposError && repos !== null && repos.length > 0 && (
          <div className="space-y-3">
            {repos.map((r) => (
              <div key={r.path} className="rounded-md border border-black/[.08] p-4 dark:border-white/[.145]">
                <div className="flex items-center justify-between gap-4">
                  <div className="flex items-center gap-2 overflow-hidden">
                    <span className="h-2 w-2 shrink-0 rounded-full bg-green-500" />
                    <span className="truncate font-mono text-sm">{r.path}</span>
                  </div>
                  <span className="shrink-0 text-xs text-zinc-500">{formatLastScanned(r.last_scanned_at)}</span>
                </div>
                {r.counts && (
                  <div className="mt-3 flex gap-5 text-xs text-zinc-500">
                    <span>{r.counts.files} files</span>
                    <span>{r.counts.symbols} symbols</span>
                    <span>{r.counts.gotchas} gotchas</span>
                    <span>{r.counts.decisions} decisions</span>
                  </div>
                )}
                <div className="mt-3 flex gap-4 text-sm">
                  <Link href={`/graph?path=${encodeURIComponent(r.path)}`} className="underline">
                    Graph →
                  </Link>
                  <Link href={`/docs?path=${encodeURIComponent(r.path)}`} className="underline">
                    Docs →
                  </Link>
                </div>
              </div>
            ))}
          </div>
        )}
      </div>

      <details className="mt-8">
        <summary className="cursor-pointer text-sm text-zinc-500">Check a repo by path directly</summary>
        <form
          className="mt-3 flex gap-2"
          onSubmit={(e) => {
            e.preventDefault();
            load(path);
          }}
        >
          <input
            type="text"
            value={path}
            onChange={(e) => setPath(e.target.value)}
            placeholder="absolute path to a scanned repo"
            className="flex-1 rounded-md border border-black/[.08] bg-transparent px-3 py-2 text-sm font-mono dark:border-white/[.145]"
          />
          <button
            type="submit"
            className="rounded-md bg-foreground px-4 py-2 text-sm text-background transition-colors hover:bg-[#383838] dark:hover:bg-[#ccc]"
          >
            Check
          </button>
        </form>

        {loading && <p className="mt-4 text-sm text-zinc-500">Loading…</p>}
        {error && (
          <div className="mt-4 rounded-md border border-red-500/30 bg-red-500/5 p-4 text-sm text-red-700 dark:text-red-400">{error}</div>
        )}
        {!loading && !error && summary && (
          <div className="mt-4 rounded-md border border-black/[.08] p-5 dark:border-white/[.145]">
            <div className="flex items-center gap-2">
              <span className="h-2 w-2 rounded-full bg-green-500" />
              <span className="text-sm font-medium">healthy</span>
              <span className="font-mono text-xs text-zinc-500">{path}</span>
            </div>
            <div className="mt-4 grid grid-cols-4 gap-4 text-sm">
              <div><div className="text-lg font-semibold">{summary.files}</div><div className="text-zinc-500">files</div></div>
              <div><div className="text-lg font-semibold">{summary.symbols}</div><div className="text-zinc-500">symbols</div></div>
              <div><div className="text-lg font-semibold">{summary.gotchas}</div><div className="text-zinc-500">gotchas</div></div>
              <div><div className="text-lg font-semibold">{summary.decisions}</div><div className="text-zinc-500">decisions</div></div>
            </div>
            <div className="mt-5 flex gap-4 text-sm">
              <Link href={`/graph?path=${encodeURIComponent(path)}`} className="underline">
                Open graph browser →
              </Link>
              {summary.hasDoc && (
                <Link href={`/docs?path=${encodeURIComponent(path)}`} className="underline">
                  Open onboarding doc →
                </Link>
              )}
            </div>
          </div>
        )}
      </details>

      <p className="mt-8 text-sm text-zinc-600 dark:text-zinc-400">
        Looking to connect a repo for hosted (heavy-tier) access instead?{" "}
        <Link href="/repos/connect" className="underline">
          GitHub App / SSH deploy-key connection flow →
        </Link>
      </p>
    </main>
  );
}
