"use client";

import { useEffect, useState } from "react";

type Visibility = "Public" | { Private: string };

type Library = {
  id: number;
  slug: string;
  name: string;
  github_repo: string | null;
  docs_url: string | null;
  visibility: Visibility;
};

const API_BASE = process.env.NEXT_PUBLIC_DOCBRAIN_API_URL || "http://127.0.0.1:8421";

function visibilityLabel(v: Visibility): string {
  return v === "Public" ? "public" : `private:${v.Private}`;
}

export default function LibrariesPage() {
  const [org, setOrg] = useState("");
  const [libraries, setLibraries] = useState<Library[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  const load = (orgFilter: string) => {
    setLoading(true);
    setError(null);
    const url = new URL("/libraries", API_BASE);
    if (orgFilter.trim()) url.searchParams.set("org", orgFilter.trim());

    fetch(url.toString())
      .then((res) => {
        if (!res.ok) throw new Error(`docbrain-api returned ${res.status}`);
        return res.json();
      })
      .then((data) => setLibraries(data.libraries))
      .catch((e) =>
        setError(
          `Could not reach docbrain-api at ${API_BASE} (${e instanceof Error ? e.message : String(e)}). ` +
            `Run \`agentops docbrain-serve-api\` and reload.`
        )
      )
      .finally(() => setLoading(false));
  };

  useEffect(() => {
    load("");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return (
    <main className="mx-auto max-w-3xl px-8 py-16">
      <h1 className="text-2xl font-semibold">Docbrain library browser</h1>
      <p className="mt-2 text-zinc-600 dark:text-zinc-400">
        Libraries ingested via <code className="text-sm">agentops sync-docs</code> and <code className="text-sm">discover_library</code> —
        version-aware docs/changelogs, scoped by visibility (Docbrain-1/2/4).
      </p>

      <form
        className="mt-6 flex gap-2"
        onSubmit={(e) => {
          e.preventDefault();
          load(org);
        }}
      >
        <input
          type="text"
          value={org}
          onChange={(e) => setOrg(e.target.value)}
          placeholder="org id (blank = public only)"
          className="flex-1 rounded-md border border-black/[.08] bg-transparent px-3 py-2 text-sm dark:border-white/[.145]"
        />
        <button
          type="submit"
          className="rounded-md bg-foreground px-4 py-2 text-sm text-background transition-colors hover:bg-[#383838] dark:hover:bg-[#ccc]"
        >
          Filter
        </button>
      </form>

      <div className="mt-6">
        {loading && <p className="text-sm text-zinc-500">Loading…</p>}
        {error && (
          <div className="rounded-md border border-red-500/30 bg-red-500/5 p-4 text-sm text-red-700 dark:text-red-400">{error}</div>
        )}
        {!loading && !error && libraries && libraries.length === 0 && (
          <p className="text-sm text-zinc-500">No libraries visible for this scope yet.</p>
        )}
        {!loading && !error && libraries && libraries.length > 0 && (
          <div className="overflow-x-auto">
            <table className="w-full border-collapse text-sm">
              <thead>
                <tr className="border-b border-black/[.08] text-left dark:border-white/[.145]">
                  <th className="py-2 pr-4 font-medium text-zinc-500">Slug</th>
                  <th className="py-2 pr-4 font-medium text-zinc-500">Name</th>
                  <th className="py-2 pr-4 font-medium text-zinc-500">Repo</th>
                  <th className="py-2 pr-4 font-medium text-zinc-500">Docs</th>
                  <th className="py-2 pr-4 font-medium text-zinc-500">Visibility</th>
                </tr>
              </thead>
              <tbody>
                {libraries.map((lib) => (
                  <tr key={lib.id} className="border-b border-black/[.05] dark:border-white/[.08]">
                    <td className="py-2 pr-4 font-mono">{lib.slug}</td>
                    <td className="py-2 pr-4">{lib.name}</td>
                    <td className="py-2 pr-4">
                      {lib.github_repo ? (
                        <a href={lib.github_repo} target="_blank" rel="noreferrer" className="underline">
                          repo
                        </a>
                      ) : (
                        "-"
                      )}
                    </td>
                    <td className="py-2 pr-4">
                      {lib.docs_url ? (
                        <a href={lib.docs_url} target="_blank" rel="noreferrer" className="underline">
                          docs
                        </a>
                      ) : (
                        "-"
                      )}
                    </td>
                    <td className="py-2 pr-4">{visibilityLabel(lib.visibility)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </div>
    </main>
  );
}
