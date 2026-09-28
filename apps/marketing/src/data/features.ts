export interface FeaturePoint {
  i: string;
  t: string;
}

export interface Feature {
  id: string;
  n: string;
  name: string;
  hook: string;
  line: string;
  points: string[];
  cmds: string[];
  cmdNote: string;
  color: string;
}

const COLORS = ['#cba6f7', '#fab387', '#89b4fa', '#f9e2af', '#94e2d5', '#f5c2e7', '#a6e3a1'];

const RAW: Omit<Feature, 'color'>[] = [
  {
    id: 'search',
    n: '01',
    name: 'Search',
    hook: 'Answers grounded in your real code.',
    line: 'Semantic + full-text query UI over a repo’s graph — ask things like “how does authentication work here” or “what gotchas exist around database migrations” and get grounded answers.',
    points: [
      'Four signals fused into one ranking: dense embeddings, BM25, exact-name matching, and personalized-PageRank expansion over the graph.',
      'Two-tier retrieval: a gist first, full detail only when the agent asks. Cheaper reads, same precision.',
      'Gotchas and decisions attached to a matched symbol come back in the same result.',
    ],
    cmds: ['agentops search "how does authentication work here"'],
    cmdNote: 'Agents get the same search over MCP; humans get this screen.',
  },
  {
    id: 'graph',
    n: '02',
    name: 'Knowledge Graph',
    hook: 'See what’s load-bearing, not just what’s big.',
    line: 'Interactive node/edge graph of the whole repo, filterable by node kind and traversal depth, with a hotspot overlay that colors nodes by graph centrality — see what’s structurally load-bearing, not just what’s big.',
    points: [
      'Filter by node kind: files, symbols, gotchas, decisions, notes.',
      'Limit traversal depth from any node to see its real neighborhood.',
      'Hotspot overlay ranks by centrality, so a 40-line function everything depends on outranks a 4,000-line file nothing does.',
    ],
    cmds: ['agentops install', 'agentops watch'],
    cmdNote: 'install builds the graph; watch rescans on file changes. The figure above is live: drag it, toggle it.',
  },
  {
    id: 'docs-viewer',
    n: '03',
    name: 'Documentation Viewer',
    hook: 'Onboarding that ranks what matters first.',
    line: 'Auto-generated, navigable onboarding document per repo, built to stay usable even on repos with 1000+ symbols — ranked by what actually matters, not an arbitrary file tree.',
    points: [
      'Sections ordered by dependency centrality, not directory order.',
      'Stays navigable on repos with 1000+ symbols.',
      'Same generator as agentops docgen. The repo-map.md in AgentOps’s own repository was produced by AgentOps scanning itself.',
    ],
    cmds: ['agentops docgen'],
    cmdNote: 'Writes repo-map.md from a scanned repo. This screen is authenticated and per-repo; the public API docs are linked in the header.',
  },
  {
    id: 'gotchas',
    n: '04',
    name: 'Gotchas',
    hook: 'Lessons that stay attached to the code.',
    line: 'A curation workspace for the bugs and lessons that never make it into code comments — tabs for needs-curation / pinned / kept / reduced, tied directly to the code they’re about.',
    points: [
      'Four tabs: needs curation, pinned, kept, reduced.',
      'Each note is matched to a symbol by a real graph edge, so it surfaces whenever that symbol does. Nobody has to remember to mention it.',
      'Already have a notes vault? ingest-notes walks the folder recursively and symbol-matches what it finds.',
    ],
    cmds: ['agentops note', 'agentops ingest-notes <folder>'],
    cmdNote: 'Curated notes are also the training input for local consolidation.',
  },
  {
    id: 'libraries',
    n: '05',
    name: 'Libraries',
    hook: 'Docs for the version you actually installed.',
    line: 'Tracks the docs of every third-party dependency your code actually uses, at the version you actually have installed — register, scrape, and search them from one screen.',
    points: [
      'Versions come from what’s installed, not what’s newest.',
      'sync-docs scans your dependencies and registers anything docbrain doesn’t know yet.',
      'AgentOps registers itself as a library on first boot. Scrape slug agentops and its own docs become searchable.',
    ],
    cmds: ['agentops sync-docs', 'agentops docbrain-serve'],
    cmdNote: 'Agents query through the docbrain MCP server with search_docs and get_docs.',
  },
  {
    id: 'connect',
    n: '06',
    name: 'Repository Connection',
    hook: 'Index once. Everyone connects.',
    line: 'Connect a repo three ways: plain local install (no remote needed), SSH deploy key, or GitHub App (OAuth, no SSH key needed) — for teams sharing one indexed instance.',
    points: [
      'Local install: no remote, no credentials.',
      'SSH deploy key: read access to one repo, nothing else.',
      'GitHub App: OAuth, no SSH key to manage. Teammates then connect with --remote and a personal API key.',
    ],
    cmds: ['npx agentops-cli connect --remote <server-url>'],
    cmdNote: 'Omit --remote interactively and connect asks which case you’re in before doing anything.',
  },
  {
    id: 'team',
    n: '07',
    name: 'Team & Settings',
    hook: 'Know who, and which agent, did what.',
    line: 'Roles, invites, and an audit log — because more than one person (or agent) usually needs access.',
    points: [
      'The first visitor to /login sets up the org and becomes Owner. Signup is invite-only after that.',
      'No external identity provider required.',
      'Personal API keys live under Settings → API Keys, for remote MCP connections.',
    ],
    cmds: ['agentops api-key'],
    cmdNote: 'Generates a key for the REST API’s optional auth.',
  },
];

export const FEATURES: Feature[] = RAW.map((f, i) => ({ ...f, color: COLORS[i] }));

export function getFeature(id: string): Feature | undefined {
  return FEATURES.find((f) => f.id === id);
}
