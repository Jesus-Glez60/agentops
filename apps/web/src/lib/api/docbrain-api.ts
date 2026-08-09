import { DOCBRAIN_API_BASE, apiFetch } from "./fetcher";
import type { CallToolResult, Library, ToolDefinition } from "./types";

export function getLibraries(org?: string): Promise<Library[]> {
  return apiFetch<{ libraries: Library[] }>(DOCBRAIN_API_BASE, "/libraries", { query: { org } }).then((r) => r.libraries);
}

export function listDocbrainTools(): Promise<ToolDefinition[]> {
  return apiFetch<{ tools: ToolDefinition[] }>(DOCBRAIN_API_BASE, "/tools").then((r) => r.tools);
}

/**
 * Every docbrain-mcp tool goes through this one generic dispatch --
 * get_docs/get_changelog/resolve_library/scrape_library/get_job_status/etc.
 * Responses are the `CallToolResult` text envelope (see types.ts); callers
 * parse `result.content[0].text` themselves since these tools don't return
 * structured JSON.
 */
export function callDocbrainTool(name: string, args: Record<string, unknown> = {}): Promise<CallToolResult> {
  return apiFetch<CallToolResult>(DOCBRAIN_API_BASE, `/tools/${name}`, {
    method: "POST",
    body: JSON.stringify(args),
  });
}

/**
 * There's no `list_doc_versions` tool -- the only place a library's known
 * versions surface at all is inside `get_docs`'s own *success* text (not
 * an error -- `tool_get_docs` returns `Ok(...)` either way) when the
 * requested version has no docs: `"No docs found for {slug}@{version}.
 * Available versions: [...]"`. Deliberately probing with a version that
 * can never be real is a documented, fragile-on-purpose workaround for
 * that missing tool (see the plan's Phase 2.5 note) -- not a hidden hack.
 */
export async function probeDocVersions(slug: string, org?: string): Promise<string[]> {
  const result = await callDocbrainTool("get_docs", { slug, version: "__version_probe__", org });
  const text = result.content[0]?.text ?? "";
  const match = /Available versions: \[(.*)\]/.exec(text);
  if (!match) return [];
  return match[1]
    .split(",")
    .map((s) => s.trim().replace(/^"|"$/g, ""))
    .filter(Boolean);
}
