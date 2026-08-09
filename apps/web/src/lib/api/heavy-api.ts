import { HEAVY_API_BASE, apiFetch } from "./fetcher";
import type {
  ConnectRepoResponse,
  ConnectionView,
  DocSearchResponse,
  IndexResponse,
  InstallUrlResponse,
  SearchResponse,
  VerifyResponse,
} from "./types";

export function connectRepo(tenant: string, repoId: string, repoUrl: string): Promise<ConnectRepoResponse> {
  return apiFetch<ConnectRepoResponse>(HEAVY_API_BASE, "/repos/connect", {
    method: "POST",
    body: JSON.stringify({ tenant, repo_id: repoId, repo_url: repoUrl }),
  });
}

export function getConnections(tenant: string): Promise<ConnectionView[]> {
  return apiFetch<{ connections: ConnectionView[] }>(HEAVY_API_BASE, "/repos", { query: { tenant } }).then((r) => r.connections);
}

export function verifyConnection(id: string, tenant: string): Promise<VerifyResponse> {
  return apiFetch<VerifyResponse>(HEAVY_API_BASE, `/repos/${id}/verify`, {
    method: "POST",
    query: { tenant },
  });
}

/** Throws `ApiError` with status 404 when no GitHub App is configured for this deployment. */
export function getGithubAppInstallUrl(): Promise<InstallUrlResponse> {
  return apiFetch<InstallUrlResponse>(HEAVY_API_BASE, "/repos/github-app/install-url");
}

/** Throws `ApiError` with status 402 when semantic search isn't licensed/configured. */
export function search(path: string, query: string, topK = 5): Promise<SearchResponse> {
  return apiFetch<SearchResponse>(HEAVY_API_BASE, "/search", { query: { path, q: query, top_k: topK } });
}

export function indexRepoForSearch(path: string): Promise<IndexResponse> {
  return apiFetch<IndexResponse>(HEAVY_API_BASE, "/search/index", {
    method: "POST",
    body: JSON.stringify({ path }),
  });
}

/** Throws `ApiError` with status 402 when semantic search isn't licensed/configured. */
export function docsSearch(slug: string, query: string, topK = 5): Promise<DocSearchResponse> {
  return apiFetch<DocSearchResponse>(HEAVY_API_BASE, "/docs/search", { query: { slug, q: query, top_k: topK } });
}

export function indexDocsForSearch(slug: string, org?: string): Promise<IndexResponse> {
  return apiFetch<IndexResponse>(HEAVY_API_BASE, "/docs/search/index", {
    method: "POST",
    body: JSON.stringify({ slug, org }),
  });
}
