"use client";

import { useSearchParams } from "next/navigation";
import { Suspense, useEffect, useState } from "react";

const API_BASE = process.env.NEXT_PUBLIC_AGENTOPS_API_URL || "http://127.0.0.1:8420";

export default function DocsPage() {
  return (
    <Suspense fallback={<main className="mx-auto max-w-3xl px-8 py-16 text-sm text-zinc-500">Loading…</main>}>
      <DocsPageInner />
    </Suspense>
  );
}

function DocsPageInner() {
  const searchParams = useSearchParams();
  const [path, setPath] = useState(searchParams.get("path") ?? "");
  const [content, setContent] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  const load = (repoPath: string) => {
    if (!repoPath.trim()) return;
    setLoading(true);
    setError(null);
    const url = new URL("/docs", API_BASE);
    url.searchParams.set("path", repoPath.trim());

    fetch(url.toString())
      .then(async (res) => {
        const data = await res.json();
        if (!res.ok) throw new Error(data.error || `agentops-api returned ${res.status}`);
        return data;
      })
      .then((data) => setContent(data.content))
      .catch((e) => setError(e instanceof Error ? e.message : String(e)))
      .finally(() => setLoading(false));
  };

  useEffect(() => {
    const initial = searchParams.get("path");
    if (initial) load(initial);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return (
    <main className="mx-auto max-w-3xl px-8 py-16">
      <h1 className="text-2xl font-semibold">Onboarding docs viewer</h1>
      <p className="mt-2 text-zinc-600 dark:text-zinc-400">
        Renders <code className="text-sm">agentops-docgen</code> output — Codebrain-3, generated directly from the indexed code
        graph, not hand-written.
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

      <div className="mt-6">
        {loading && <p className="text-sm text-zinc-500">Loading…</p>}
        {error && (
          <div className="rounded-md border border-red-500/30 bg-red-500/5 p-4 text-sm text-red-700 dark:text-red-400">
            {error}
          </div>
        )}
        {!loading && !error && content && (
          <pre className="overflow-x-auto whitespace-pre-wrap rounded-md border border-black/[.08] bg-black/[.02] p-4 text-xs leading-relaxed dark:border-white/[.145] dark:bg-white/[.03]">
            {content}
          </pre>
        )}
      </div>
    </main>
  );
}
