// Typed client for docbrain-api's /libraries routes. Types mirror
// docbrain-graph's Library/RepoLibraryUsage response shapes exactly; see
// docbrain-core/crates/docbrain-graph/src/lib.rs for the Rust source of
// truth. A separate base URL from agentops-api.ts -- docbrain-api is its
// own server/port, not a route on agentops-api.
import { apiFetch } from "@/lib/api/fetcher";

const DOCBRAIN_API_URL = process.env.NEXT_PUBLIC_DOCBRAIN_API_URL ?? "http://127.0.0.1:8421";

export const LIBRARIES_SWR_KEY = "/libraries";

export interface Library {
  id: number;
  slug: string;
  name: string;
  /** Fetched during discovery (npm/PyPI/GitHub metadata); `null` for libraries registered before this was persisted, or with no published description. */
  description: string | null;
  github_repo: string | null;
  docs_url: string | null;
  doc_snapshots: number;
  changelog_versions: number;
  /** Distinct scraped versions, ascending -- last element is "latest indexed" for mismatch comparison. */
  versions: string[];
  total_nodes: number;
  /** Count of distinct repos with a declared-version row for this library. */
  used_in_count: number;
  /** `MAX(doc_snapshots.scraped_at)` -- `null` if never scraped. */
  last_indexed_at: string | null;
  /** List-endpoint-only: `true` if any repo's declared version differs from `versions.at(-1)`. Computed server-side, not present on the detail endpoint's nested `library` (use `used_in[].mismatch` there instead). */
  has_mismatch: boolean;
}

/** One repo's manifest-declared dependency on a library -- from `agentops sync-docs`, not import scanning. */
export interface RepoLibraryUsage {
  repo_identifier: string;
  declared_version: string;
  updated_at: string;
  /** `declared_version !== library.versions.at(-1)` -- derived server-side at read time, never stored. */
  mismatch: boolean;
}

export interface LibraryDetail {
  library: Library;
  used_in: RepoLibraryUsage[];
}

export async function getLibraries(): Promise<Library[]> {
  const { libraries } = await apiFetch<{ libraries: Library[] }>(DOCBRAIN_API_URL, "/libraries");
  return libraries;
}

export async function getLibrary(slug: string): Promise<LibraryDetail> {
  return apiFetch<LibraryDetail>(DOCBRAIN_API_URL, `/libraries/${encodeURIComponent(slug)}`);
}

interface ToolResult {
  content: Array<{ text: string; type: string }>;
  isError: boolean;
}

async function callTool(name: string, args: Record<string, unknown>): Promise<{ isError: boolean; text: string }> {
  const result = await apiFetch<ToolResult>(DOCBRAIN_API_URL, `/tools/${name}`, { method: "POST", body: JSON.stringify(args) });
  return { isError: result.isError, text: result.content[0]?.text ?? "" };
}

/** One per package manager for a language this project's scanner supports (Python, TypeScript/JavaScript, Go, Rust, C#) -- mirrors `docbrain_ingest::Ecosystem` exactly. C++ has no single dominant package manager, so it isn't listed here; use `registerLibraryManually` for it (and for anything else with no registry entry at all, like Clerk). */
export type Ecosystem = "npm" | "pypi" | "cargo" | "go" | "nuget";

export const ECOSYSTEM_LABELS: Record<Ecosystem, string> = {
  npm: "npm",
  pypi: "PyPI",
  cargo: "crates.io (Rust)",
  go: "Go modules",
  nuget: "NuGet (.NET)",
};

/** `discover_library`'s tool-result envelope -- not a genuinely-REST shape. Used by the "Add library" flow, which has no dedicated REST route. */
export async function discoverLibrary(ecosystem: Ecosystem, name: string): Promise<{ isError: boolean; text: string }> {
  return callTool("discover_library", { ecosystem, name });
}

/** No registry lookup -- for a library with no package-manager entry at all (a product with its own standalone docs site, e.g. Clerk or Stripe). */
export async function registerLibraryManually(slug: string, docsUrl: string, githubRepo?: string): Promise<{ isError: boolean; text: string }> {
  return callTool("register_library", { slug, name: slug, docs_url: docsUrl, github_repo: githubRepo || undefined });
}

/** `get_docs`'s text wraps each node in a synthetic `## {topic} (node {id}, distance ...)` heading -- node ids/distances are useful for an agent (e.g. to call `get_code_examples`) but the topic almost always duplicates the real heading the node's own scraped content leads with, so a human reading it sees the same title twice in a row. Stripped for display only -- `getLibraryDocs` still returns the same text an agent would get, just without this one wrapper line per node. */
function stripSyntheticNodeHeadings(text: string): string {
  return text.replace(/^#{2,3} .*\(node \d+.*\)\s*$\n?/gm, "");
}

/** Token-budgeted doc-node dump for one library@version, rendered as plain text -- docbrain's docs are raw scraped chunks, not agentops-docgen's structured `DocPage`, so there's no separate JSON shape to render into a symbol table; this is the same content an agent would get, just without the per-node id/distance wrapper headings (see `stripSyntheticNodeHeadings`). */
export async function getLibraryDocs(slug: string, version: string): Promise<string> {
  const result = await callTool("get_docs", { slug, version, max_tokens: 3000 });
  return stripSyntheticNodeHeadings(result.text);
}

/** Every synced changelog entry, most recent first -- not scoped to the two currently-viewed doc-snapshot versions. Real GitHub release tags (what changelog entries are keyed on, e.g. "16.2.1") and a library's coarser doc-snapshot versions (whatever `scrape_library` was told to fetch, e.g. "16") are different granularities with no guaranteed exact match, so an exact-pair lookup would silently return nothing for most libraries. */
export async function getLibraryChangelog(slug: string): Promise<string> {
  const result = await callTool("list_changelog", { slug });
  return result.text;
}

/** `background: true` returns immediately with a job id (used by the detail page's "Re-index" action -- the user's already looking at the library, no need to block on it). `background: false` waits for the real scrape result, so the "Add library" flow can show genuine success/failure and the list can refresh with real doc counts, not just a registered-but-empty row. */
export async function scrapeLibrary(slug: string, version: string, background: boolean): Promise<{ isError: boolean; text: string }> {
  return callTool("scrape_library", { slug, version, background, max_pages: 6 });
}

export async function rescrapeLibrary(slug: string, version: string): Promise<{ isError: boolean; text: string }> {
  return scrapeLibrary(slug, version, true);
}
