/** Illustrative subset of AgentOps's own repo, used only to animate the
 * marketing site's decorative knowledge-graph demo. Not live data. */

export type NodeKind = 'file' | 'symbol' | 'gotcha' | 'decision';

export interface DemoNode {
  id: string;
  label: string;
  kind: NodeKind;
  path: string;
}

export interface DemoEdge {
  a: string;
  b: string;
}

const FILES: [string, string][] = [
  ['server', 'agentops-server/src/main.rs'],
  ['scanner', 'agentops-scanner/src/lib.rs'],
  ['graph', 'agentops-graph/src/lib.rs'],
  ['retrieval', 'agentops-retrieval/src/lib.rs'],
  ['mcp', 'agentops-mcp/src/protocol.rs'],
  ['notes', 'agentops-notes/src/vault.rs'],
  ['embeddings', 'agentops-embeddings/src/lib.rs'],
  ['consolidate', 'agentops-heavy-consolidate/src/lib.rs'],
  ['cli', 'agentops-cli/src/main.rs'],
  ['security', 'agentops-security/src/lib.rs'],
  ['ruler', 'agentops-ruler-bridge/src/lib.rs'],
  ['docgen', 'agentops-docgen/src/lib.rs'],
  ['docbrain', 'docbrain-core/crates/…'],
  ['teams', 'agentops-teams/src/lib.rs'],
];

const SYMS: [string, string][] = [
  ['route_mcp', 'server'],
  ['route_api', 'server'],
  ['scan_repo', 'scanner'],
  ['parse_ast', 'scanner'],
  ['extract_imports', 'scanner'],
  ['GraphStore', 'graph'],
  ['upsert_edges', 'graph'],
  ['neighbors', 'graph'],
  ['hybrid_search', 'retrieval'],
  ['bm25_score', 'retrieval'],
  ['dense_query', 'retrieval'],
  ['exact_name_match', 'retrieval'],
  ['personalized_pagerank', 'retrieval'],
  ['fuse_rankings', 'retrieval'],
  ['gist_then_detail', 'retrieval'],
  ['ToolRegistry', 'mcp'],
  ['list_tools', 'mcp'],
  ['read_only_filter', 'mcp'],
  ['match_note_to_symbol', 'notes'],
  ['ingest_vault', 'notes'],
  ['embed_batch', 'embeddings'],
  ['train_adapter', 'consolidate'],
  ['eval_holdout', 'consolidate'],
  ['gate_release', 'consolidate'],
  ['run_install', 'cli'],
  ['run_connect', 'cli'],
  ['master_key', 'security'],
  ['distribute_agents_md', 'ruler'],
  ['rank_sections', 'docgen'],
  ['scrape_library', 'docbrain'],
  ['search_docs', 'docbrain'],
  ['invite_member', 'teams'],
  ['audit_log', 'teams'],
];

const NOTES: [string, NodeKind, string, string, string?][] = [
  ['g1', 'gotcha', 'Strip self-loops before iterating PPR', 'personalized_pagerank'],
  ['g2', 'gotcha', 'Vault frontmatter dates are local time', 'ingest_vault'],
  ['g3', 'gotcha', 'Held-out set must never overlap training notes', 'eval_holdout'],
  ['g4', 'gotcha', 'A rescan invalidates cached gists', 'gist_then_detail'],
  ['d1', 'decision', 'Filter tools at list time, not call time', 'read_only_filter', 'list_tools'],
  ['d2', 'decision', 'SQLite by default, Postgres opt-in', 'GraphStore'],
  ['d3', 'decision', 'Scanner links no network-capable crates', 'scan_repo'],
  ['d4', 'decision', 'Ship a new model only if eval ≥ current', 'gate_release'],
];

const LINKS: [string, string][] = [
  ['route_mcp', 'ToolRegistry'],
  ['route_api', 'hybrid_search'],
  ['ToolRegistry', 'list_tools'],
  ['list_tools', 'read_only_filter'],
  ['ToolRegistry', 'hybrid_search'],
  ['ToolRegistry', 'search_docs'],
  ['hybrid_search', 'bm25_score'],
  ['hybrid_search', 'dense_query'],
  ['hybrid_search', 'exact_name_match'],
  ['hybrid_search', 'personalized_pagerank'],
  ['hybrid_search', 'fuse_rankings'],
  ['hybrid_search', 'gist_then_detail'],
  ['hybrid_search', 'neighbors'],
  ['dense_query', 'embed_batch'],
  ['personalized_pagerank', 'neighbors'],
  ['neighbors', 'GraphStore'],
  ['upsert_edges', 'GraphStore'],
  ['scan_repo', 'parse_ast'],
  ['scan_repo', 'extract_imports'],
  ['scan_repo', 'upsert_edges'],
  ['run_install', 'scan_repo'],
  ['run_connect', 'distribute_agents_md'],
  ['master_key', 'route_api'],
  ['match_note_to_symbol', 'neighbors'],
  ['ingest_vault', 'match_note_to_symbol'],
  ['train_adapter', 'embed_batch'],
  ['gate_release', 'eval_holdout'],
  ['gate_release', 'train_adapter'],
  ['eval_holdout', 'hybrid_search'],
  ['rank_sections', 'personalized_pagerank'],
  ['rank_sections', 'GraphStore'],
  ['scrape_library', 'embed_batch'],
  ['search_docs', 'bm25_score'],
  ['invite_member', 'audit_log'],
  ['route_api', 'invite_member'],
  ['run_install', 'rank_sections'],
];

export function buildDemoGraphData(): { nodes: DemoNode[]; edges: DemoEdge[] } {
  const nodes: DemoNode[] = [];
  const edges: DemoEdge[] = [];
  const parentOf: Record<string, string> = {};

  for (const [id, path] of FILES) {
    nodes.push({ id, label: id, kind: 'file', path });
  }
  for (const [id, parent] of SYMS) {
    const parentFile = FILES.find(([fid]) => fid === parent);
    nodes.push({ id, label: id, kind: 'symbol', path: parentFile?.[1] ?? '' });
    edges.push({ a: parent, b: id });
    parentOf[id] = parent;
  }
  for (const [id, kind, label, s, s2] of NOTES) {
    nodes.push({ id, label, kind, path: `${kind === 'gotcha' ? 'gotcha' : 'decision'} → ${s}` });
    edges.push({ a: s, b: id });
    if (s2) edges.push({ a: s2, b: id });
  }
  for (const [a, b] of LINKS) {
    edges.push({ a, b });
  }

  return { nodes, edges };
}
