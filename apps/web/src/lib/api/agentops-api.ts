import { AGENTOPS_API_BASE, apiFetch } from "./fetcher";
import type { CallToolResult, DocsResponse, GraphResponse, RepoSummary, ToolDefinition } from "./types";

export function getRepos(): Promise<RepoSummary[]> {
  return apiFetch<{ repos: RepoSummary[] }>(AGENTOPS_API_BASE, "/repos").then((r) => r.repos);
}

export function getGraph(path: string): Promise<GraphResponse> {
  return apiFetch<GraphResponse>(AGENTOPS_API_BASE, "/graph", { query: { path } });
}

export function getDocs(path: string): Promise<DocsResponse> {
  return apiFetch<DocsResponse>(AGENTOPS_API_BASE, "/docs", { query: { path } });
}

export function listTools(): Promise<ToolDefinition[]> {
  return apiFetch<{ tools: ToolDefinition[] }>(AGENTOPS_API_BASE, "/tools").then((r) => r.tools);
}

export function callTool(name: string, args: Record<string, unknown> = {}): Promise<CallToolResult> {
  return apiFetch<CallToolResult>(AGENTOPS_API_BASE, `/tools/${name}`, {
    method: "POST",
    body: JSON.stringify(args),
  });
}
