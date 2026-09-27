# AgentOps Marketing Page — Brief

*Written for: a design-focused session building the public marketing site (planned in Astro, separate app, living at the site root `/`). This brief is factual positioning + content input, not a visual spec — the design itself should be original and should NOT default to generic SaaS/AI-tool visual patterns (see "Design direction" at the end).*

---

## One-line pitch

> **Turn your codebase into a knowledge graph AI coding agents can actually query — scanning, hybrid search, curated gotchas/decisions, and 35 MCP tools, all self-hosted.**

(Source: project README / `llms.txt` — this is the canonical line, not copy invented for this brief.)

## The problem, in plain terms

AI coding agents are powerful, but every session starts from zero: they re-read files to figure out what a codebase does, guess at things they can't see, forget what was already tried and fixed, and burn tokens re-deriving context that already existed five sessions ago. That costs three things every day: **money** (re-reading the same code repeatedly), **trust** (confident hallucination with nothing to check it against), and **institutional knowledge** (the same bug gets rediscovered, or reintroduced, because nothing tied the lesson to the code).

## What AgentOps actually is

AgentOps scans a codebase into a **persistent, queryable knowledge graph** — files, symbols, dependencies, and how they relate — instead of leaving an agent to re-derive context from scratch. On top of that graph:

- **Hybrid search** — dense embeddings + lexical/BM25 + exact-name matching + personalized-PageRank graph expansion, plus a two-tier "gist then detail" retrieval mode.
- **Curated Gotchas & Decisions** — the tribal knowledge that never makes it into code comments, tied directly to the exact symbol it's about via a real graph edge, so it surfaces automatically instead of depending on someone remembering to mention it.
- **CLS-inspired local consolidation** — the system can locally fine-tune a small model on a repo's own curated notes, gated so a new version only ships if it doesn't regress retrieval quality against a held-out eval. The tool genuinely gets better at *your* repo over time — a real technical claim, not marketing fluff.
- **35 MCP tools** across three servers, giving any MCP-speaking coding agent structured access to all of the above.

It's a single open-core product: one Rust server process, one Next.js frontend, no price gate, minimal required config (one env var to self-host).

## Strong points to lead with

1. **Hallucination prevention** — agents get exact, real source for a symbol plus every known gotcha attached to it, instead of guessing from a function's name and hoping.
2. **Token / cost savings** — precise lookups replace bulk file reads; a persistent graph means understanding from one session isn't re-derived (and re-paid-for) in the next.
3. **Faster onboarding** — an auto-generated, dependency-ranked engineering overview (not an arbitrary file listing) gets new team members — human or AI — oriented fast, even on repos with 1000+ symbols.
4. **Institutional knowledge that doesn't evaporate** — gotchas/decisions live on the graph, attached to code, not filed in a wiki nobody reads before touching that code again; an existing notes vault can be ingested wholesale.
5. **External library awareness, versioned correctly** — `docbrain` discovers and tracks third-party library docs per the exact version installed, so an agent isn't guided by the wrong version's docs.
6. **Two-command onboarding, precisely stated**: client-side setup really is `npx agentops-cli install` then `connect` — two commands. Self-hosting the server is a separate one-time step (needs at minimum a master key). State both; don't blur them into a single "instant setup" claim.
7. **Team mode without everyone re-scanning locally**: `connect --remote <url>` points the same CLI at a shared, server-indexed instance.
8. **Dogfooding proof point**: AgentOps registers itself as a docbrain library on first boot — it documents itself using its own machinery.
9. **Safe by construction**: in read-only mode, write-capable tools don't just get "asked nicely" not to run — they don't appear in what the agent is told is available. A structural guarantee, not a prompt.

## Required page elements (non-negotiable)

- A **persistent header/nav**, visible on every page, with exactly two always-visible links:
  - **Documentation** — should point at the project's public API docs (`docs/` — Redoc-style OpenAPI viewer), *not* the in-app "Documentation Viewer" screen (see naming note below, which is an authenticated per-repo feature, not a public docs page).
  - **Login / Register** — routes into the AgentOps suite's auth entry point (the existing Next.js app, moving to `/suite` — link to `/suite/login`).
- A landing page structure that supports **one section/sub-page per feature**, not a single flat scroll — see feature list below.

### ⚠️ Naming collision to resolve before wiring the Documentation link

There are three different things that could be called "docs":
1. The in-app **Documentation Viewer** screen (`/suite/docs` after the routing move) — an authenticated, per-connected-repo generated engineering doc. Not for prospective visitors.
2. The existing **static API docs** (`docs/index.html` + `openapi.yaml`, Redoc-style, served separately from the app) — this is the natural target for the marketing page's "Documentation" link today.
3. A possible **new marketing-site docs/guide page** if the Astro site later wants its own docs section — decide this later, don't conflate it with #2 now.

## Feature landing sections (one per product surface, factual descriptions only)

| Feature | What it does |
|---|---|
| **Search** | Semantic + full-text query UI over a repo's graph — ask things like "how does authentication work here" or "what gotchas exist around database migrations" and get grounded answers. |
| **Knowledge Graph** | Interactive node/edge graph of the whole repo, filterable by node kind and traversal depth, with a hotspot overlay that colors nodes by graph centrality — see what's structurally load-bearing, not just what's big. |
| **Documentation Viewer** | Auto-generated, navigable onboarding document per repo, built to stay usable even on repos with 1000+ symbols — ranked by what actually matters, not an arbitrary file tree. |
| **Gotchas** | A curation workspace for the bugs and lessons that never make it into code comments — tabs for needs-curation / pinned / kept / reduced, tied directly to the code they're about. |
| **Libraries** | Tracks the docs of every third-party dependency your code actually uses, at the version you actually have installed — register, scrape, and search them from one screen. |
| **Repository Connection** | Connect a repo three ways: plain local install (no remote needed), SSH deploy key, or GitHub App (OAuth, no SSH key needed) — for teams sharing one indexed instance. |
| **Team & Settings** | Roles, invites, and an audit log — because more than one person (or agent) usually needs access. |

## Integrations (only what's real — don't overstate)

- Interactive picker supports: **Claude Code, Cursor, Codex CLI, Gemini CLI**.
- Broader support via **Ruler** under the hood: Copilot, Windsurf, Aider, Zed, and more, via `--agents <id>`.
- Repo connection: **SSH deploy key** or **GitHub App** (OAuth).
- **Linear** task sync (native tasks can push to Linear).
- Any MCP-speaking agent, generally — MCP is the protocol, not a specific vendor integration.

## Differentiators (why this, not something else)

- **CLS-inspired retrieval + local consolidation** — a repo-specific model that improves itself over time, with regression gating. Real, not a slide.
- **Multi-signal hybrid search** — dense + lexical + exact-name + graph-expansion fusion, plus two-tier retrieval.
- **Self-hosted, single open-core product, no price gate.**
- **One CLI, two modes** — fully local, or pointed at a shared team server — same commands either way.
- **Structural safety in read-only mode**, not policy-based safety.
- **Enterprise-friendly deployability**: the core scanning engine has a build-time-verified zero-network-access guarantee; richer hosted capabilities (semantic search, AI interpretation, multi-team support) live in a separately-bounded tier.

## Design direction (for the design session to run with, not a spec being dictated)

- Actively avoid what people now recognize as generic "AI tool slop": stock gradient blobs, default shadcn-look bento grids, cookie-cutter hero-plus-three-icons layouts, purple-to-blue gradients as a crutch. The page should look intentionally designed, not template-assembled.
- The subject matter — a knowledge **graph**, nodes, edges, hotspots, a codebase's structure — is a strong, literal hook for a distinctive visual identity (network/graph motifs, structural/architectural visual language) rather than generic "AI sparkle" iconography.
- Convert visitors: the page needs to move a skeptical technical visitor from "another AI wrapper" to "I want to try this on my own repo" — lead with concrete, falsifiable claims (real tool counts, real architecture, real gating on model updates) over vague superlatives.
