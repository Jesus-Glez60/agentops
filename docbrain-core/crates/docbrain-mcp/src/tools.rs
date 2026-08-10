//! Tool definitions and dispatch — driven from **one table**
//! ([`tool_specs`]), not three independently-maintained lists. `main` had a
//! confirmed gap here: `list_tools()` and `call_tool()`'s dispatch `match`
//! were maintained separately, and its own tests only asserted tool
//! *count*, not that every listed tool was actually dispatchable — a tool
//! added to one and forgotten in the other would fail silently at call
//! time, not at build/test time. Here, `list_tools()` and `call_tool()`
//! both derive from the same `tool_specs()`, so that class of bug can't
//! exist: a tool is either in the table (and therefore both listed and
//! dispatchable) or it doesn't exist.

use std::path::{Path, PathBuf};

use anyhow::Result;
use docbrain_graph::{DocbrainStore, JobStatus, SqliteDocbrainStore};
use docbrain_ingest::{ChangelogSync, LibraryDiscovery};
use serde_json::{json, Value};

use crate::protocol::{CallToolResult, ToolDefinition};

type Handler = fn(&dyn DocbrainStore, &Path, &Value) -> Result<String>;

struct ToolSpec {
    name: &'static str,
    description: &'static str,
    input_schema: fn() -> Value,
    handler: Handler,
}

fn tool_specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "list_libraries",
            description: "List every registered library.",
            input_schema: || json!({ "type": "object", "properties": {} }),
            handler: tool_list_libraries,
        },
        ToolSpec {
            name: "get_library",
            description: "Get metadata for a library by slug.",
            input_schema: || json!({ "type": "object", "properties": { "slug": { "type": "string" } }, "required": ["slug"] }),
            handler: tool_get_library,
        },
        ToolSpec {
            name: "discover_library",
            description: "On-demand discovery: looks up a package's docs/repo URLs via its registry (npm or pypi) and registers it. Falls through to 'not found' if the registry has no such package, for the caller to try GitHub search or ask the user next.",
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": { "ecosystem": { "type": "string", "enum": ["npm", "pypi"] }, "name": { "type": "string" } },
                    "required": ["ecosystem", "name"],
                })
            },
            handler: tool_discover_library,
        },
        ToolSpec {
            name: "scrape_library",
            description: "Fetch a library's docs page (registered via discover_library) and persist it as token-bounded, deduplicated, edge-linked doc nodes at the given version. Set max_pages > 1 to also follow same-origin, same-path-prefix links (shallow, bounded crawl). Set background: true to run as a background job and poll get_job_status.",
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "slug": { "type": "string" },
                        "version": { "type": "string" },
                        "max_pages": { "type": "integer", "description": "Defaults to 1 (landing page only)." },
                        "background": { "type": "boolean", "description": "Defaults to false." },
                    },
                    "required": ["slug", "version"],
                })
            },
            handler: tool_scrape_library,
        },
        ToolSpec {
            name: "sync_changelogs",
            description: "Fetches a library's GitHub releases and persists consecutive-version changelog entries (idempotent — re-syncing never duplicates a row). Set background: true to run as a background job and poll get_job_status.",
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "slug": { "type": "string" },
                        "github_token": { "type": "string", "description": "Optional — raises GitHub's rate limit past the unauthenticated default." },
                        "background": { "type": "boolean" },
                    },
                    "required": ["slug"],
                })
            },
            handler: tool_sync_changelogs,
        },
        ToolSpec {
            name: "get_docs",
            description: "Get doc nodes for a library at an exact version, up to a token budget (never an unbounded full dump). Excludes code-example nodes by default (planning-stage use) — pass include_examples: true, or call get_code_examples on a specific node, when you're ready to implement.",
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "slug": { "type": "string" },
                        "version": { "type": "string" },
                        "max_tokens": { "type": "integer", "description": "Defaults to 2000." },
                        "include_examples": { "type": "boolean", "description": "Defaults to false. Set true to include code-example nodes in the dump." },
                    },
                    "required": ["slug", "version"],
                })
            },
            handler: tool_get_docs,
        },
        ToolSpec {
            name: "search_docs",
            description: "Semantic search over the doc-node graph: embeds the query, finds the nearest nodes, and walks each hit's synapse edges (sequence/parent-section) to pull directly-connected context — never a whole page, only what fits the token budget. Excludes code-example nodes by default — this is meant for the planning stage (understand the API/concept first); once you know which node's code you need, call get_code_examples with its node id, or re-run this with include_examples: true.",
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string" },
                        "slug": { "type": "string", "description": "Optional — scope search to one library." },
                        "top_k": { "type": "integer", "description": "Defaults to 5." },
                        "max_tokens": { "type": "integer", "description": "Defaults to 1500." },
                        "include_examples": { "type": "boolean", "description": "Defaults to false. Set true to also surface code-example nodes as hits/related context." },
                    },
                    "required": ["query"],
                })
            },
            handler: tool_search_docs,
        },
        ToolSpec {
            name: "get_code_examples",
            description: "Fetches the code example(s) linked to a specific doc node (found via list_libraries/get_docs/search_docs's node ids) — the implementation-stage counterpart to a planning-stage search_docs call.",
            input_schema: || json!({ "type": "object", "properties": { "node_id": { "type": "integer" } }, "required": ["node_id"] }),
            handler: tool_get_code_examples,
        },
        ToolSpec {
            name: "get_changelog",
            description: "Get changelog entries between two exact versions.",
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": { "slug": { "type": "string" }, "from_version": { "type": "string" }, "to_version": { "type": "string" } },
                    "required": ["slug", "from_version", "to_version"],
                })
            },
            handler: tool_get_changelog,
        },
        ToolSpec {
            name: "get_job_status",
            description: "Polls a background scrape_library/sync_changelogs job's status and result message.",
            input_schema: || json!({ "type": "object", "properties": { "job_id": { "type": "integer" } }, "required": ["job_id"] }),
            handler: tool_get_job_status,
        },
    ]
}

pub fn list_tools() -> Vec<ToolDefinition> {
    tool_specs().into_iter().map(|s| ToolDefinition { name: s.name, description: s.description, input_schema: (s.input_schema)() }).collect()
}

/// `db_path` is only used by tools that can run in the background
/// (`scrape_library`/`sync_changelogs` with `background: true`) — a
/// background job opens its own `SqliteDocbrainStore` connection inside the
/// spawned thread (`rusqlite::Connection` isn't `Sync`), which requires
/// knowing the path it was opened from.
pub fn call_tool(store: &dyn DocbrainStore, db_path: &Path, name: &str, args: &Value) -> Result<CallToolResult, String> {
    let specs = tool_specs();
    let Some(spec) = specs.iter().find(|s| s.name == name) else {
        return Err(format!("unknown tool '{name}'"));
    };
    Ok(match (spec.handler)(store, db_path, args) {
        Ok(text) => CallToolResult::success(text),
        Err(e) => CallToolResult::error(e.to_string()),
    })
}

fn get_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(|v| v.as_str())
}

fn tool_list_libraries(store: &dyn DocbrainStore, _db_path: &Path, _args: &Value) -> Result<String> {
    let libs = store.list_libraries()?;
    if libs.is_empty() {
        return Ok("No libraries registered.".to_string());
    }
    Ok(libs.iter().map(|l| format!("- {} ({})", l.slug, l.name)).collect::<Vec<_>>().join("\n"))
}

fn tool_get_library(store: &dyn DocbrainStore, _db_path: &Path, args: &Value) -> Result<String> {
    let slug = get_str(args, "slug").ok_or_else(|| anyhow::anyhow!("missing required 'slug'"))?;
    match store.get_library(slug)? {
        Some(l) => Ok(format!(
            "{} ({})\nrepo: {}\ndocs: {}\nversions: {:?}\ntotal nodes: {}",
            l.slug,
            l.name,
            l.github_repo.as_deref().unwrap_or("-"),
            l.docs_url.as_deref().unwrap_or("-"),
            l.versions,
            l.total_nodes
        )),
        None => anyhow::bail!("no library '{slug}' registered"),
    }
}

fn tool_discover_library(store: &dyn DocbrainStore, _db_path: &Path, args: &Value) -> Result<String> {
    let ecosystem_str = get_str(args, "ecosystem").ok_or_else(|| anyhow::anyhow!("missing required 'ecosystem'"))?;
    let name = get_str(args, "name").ok_or_else(|| anyhow::anyhow!("missing required 'name'"))?;
    let ecosystem = match ecosystem_str {
        "npm" => docbrain_ingest::Ecosystem::Npm,
        "pypi" => docbrain_ingest::Ecosystem::PyPi,
        other => anyhow::bail!("invalid ecosystem '{other}', expected 'npm' or 'pypi'"),
    };

    match docbrain_ingest::RegistryDiscovery.discover(ecosystem, name)? {
        Some(found) => {
            store.add_library(name, name, found.repo_url.as_deref(), found.docs_url.as_deref())?;
            Ok(format!(
                "Discovered and registered '{name}'. docs: {} repo: {}",
                found.docs_url.as_deref().unwrap_or("(none published)"),
                found.repo_url.as_deref().unwrap_or("(none published)"),
            ))
        }
        None => Ok(format!("'{name}' not found on {ecosystem_str}. Try discover_library's GitHub-search fallback or ask the user for a docs URL.")),
    }
}

fn tool_scrape_library(store: &dyn DocbrainStore, db_path: &Path, args: &Value) -> Result<String> {
    let slug = get_str(args, "slug").ok_or_else(|| anyhow::anyhow!("missing required 'slug'"))?.to_string();
    let version = get_str(args, "version").ok_or_else(|| anyhow::anyhow!("missing required 'version'"))?.to_string();
    let max_pages = args.get("max_pages").and_then(|v| v.as_u64()).unwrap_or(1) as usize;
    let background = args.get("background").and_then(|v| v.as_bool()).unwrap_or(false);

    let library = store
        .get_library(&slug)?
        .ok_or_else(|| anyhow::anyhow!("no library '{slug}' registered — register it first via discover_library"))?;
    let docs_url = library.docs_url.clone().ok_or_else(|| anyhow::anyhow!("library '{slug}' has no docs_url registered — nothing to scrape"))?;

    if !background {
        let outcomes = docbrain_ingest::scrape_chunk_and_store(store, &slug, &version, &docs_url, max_pages)?;
        store.add_doc_snapshot(&slug, &version)?;
        let inserted = outcomes.iter().filter(|o| o.inserted).count();
        return Ok(format!("Scraped {docs_url} -> {} chunk(s), {inserted} new, persisted as {slug}@{version}.", outcomes.len()));
    }

    let job_id = store.create_job("scrape_library", &slug)?;
    let db_path = db_path.to_path_buf();
    std::thread::spawn(move || run_scrape_job(db_path, slug, version, docs_url, max_pages, job_id));
    Ok(format!("Started background scrape job {job_id}. Poll get_job_status with job_id: {job_id}."))
}

fn run_scrape_job(db_path: PathBuf, slug: String, version: String, docs_url: String, max_pages: usize, job_id: i64) {
    let Ok(worker_store) = SqliteDocbrainStore::open(&db_path) else { return };
    let outcome: Result<String> = (|| {
        let outcomes = docbrain_ingest::scrape_chunk_and_store(&worker_store, &slug, &version, &docs_url, max_pages)?;
        worker_store.add_doc_snapshot(&slug, &version)?;
        let inserted = outcomes.iter().filter(|o| o.inserted).count();
        Ok(format!("Scraped {docs_url} -> {} chunk(s), {inserted} new, persisted as {slug}@{version}.", outcomes.len()))
    })();
    match outcome {
        Ok(msg) => {
            let _ = worker_store.complete_job(job_id, JobStatus::Succeeded, &msg);
        }
        Err(e) => {
            let _ = worker_store.complete_job(job_id, JobStatus::Failed, &e.to_string());
        }
    }
}

fn tool_sync_changelogs(store: &dyn DocbrainStore, db_path: &Path, args: &Value) -> Result<String> {
    let slug = get_str(args, "slug").ok_or_else(|| anyhow::anyhow!("missing required 'slug'"))?.to_string();
    let token = get_str(args, "github_token").map(|s| s.to_string());
    let background = args.get("background").and_then(|v| v.as_bool()).unwrap_or(false);

    let library = store.get_library(&slug)?.ok_or_else(|| anyhow::anyhow!("no library '{slug}' registered"))?;
    let repo = library.github_repo.clone().ok_or_else(|| anyhow::anyhow!("library '{slug}' has no github_repo registered"))?;
    let (owner, name) = parse_owner_repo(&repo).ok_or_else(|| anyhow::anyhow!("could not parse owner/repo out of '{repo}'"))?;

    if !background {
        let entries = docbrain_ingest::GitHubReleasesSync.sync(&owner, &name, token.as_deref())?;
        for e in &entries {
            store.add_changelog_entry(&slug, &e.from_version, &e.to_version, &e.entry, e.breaking)?;
        }
        return Ok(format!("Synced {} changelog entr{} for {slug} from {owner}/{name}.", entries.len(), if entries.len() == 1 { "y" } else { "ies" }));
    }

    let job_id = store.create_job("sync_changelogs", &slug)?;
    let db_path = db_path.to_path_buf();
    std::thread::spawn(move || run_sync_changelogs_job(db_path, slug, owner, name, token, job_id));
    Ok(format!("Started background changelog sync job {job_id}. Poll get_job_status with job_id: {job_id}."))
}

fn run_sync_changelogs_job(db_path: PathBuf, slug: String, owner: String, name: String, token: Option<String>, job_id: i64) {
    let Ok(worker_store) = SqliteDocbrainStore::open(&db_path) else { return };
    let outcome: Result<String> = (|| {
        let entries = docbrain_ingest::GitHubReleasesSync.sync(&owner, &name, token.as_deref())?;
        for e in &entries {
            worker_store.add_changelog_entry(&slug, &e.from_version, &e.to_version, &e.entry, e.breaking)?;
        }
        Ok(format!("Synced {} changelog entr{} for {slug} from {owner}/{name}.", entries.len(), if entries.len() == 1 { "y" } else { "ies" }))
    })();
    match outcome {
        Ok(msg) => {
            let _ = worker_store.complete_job(job_id, JobStatus::Succeeded, &msg);
        }
        Err(e) => {
            let _ = worker_store.complete_job(job_id, JobStatus::Failed, &e.to_string());
        }
    }
}

fn parse_owner_repo(repo: &str) -> Option<(String, String)> {
    let trimmed = repo.trim_end_matches('/').trim_end_matches(".git");
    let after_host = trimmed.rsplit_once("github.com/").map(|(_, rest)| rest).unwrap_or(trimmed);
    let mut parts = after_host.trim_start_matches('/').splitn(2, '/');
    let owner = parts.next()?;
    let name = parts.next()?;
    if owner.is_empty() || name.is_empty() {
        return None;
    }
    Some((owner.to_string(), name.to_string()))
}

fn tool_get_docs(store: &dyn DocbrainStore, _db_path: &Path, args: &Value) -> Result<String> {
    let slug = get_str(args, "slug").ok_or_else(|| anyhow::anyhow!("missing required 'slug'"))?;
    let version = get_str(args, "version").ok_or_else(|| anyhow::anyhow!("missing required 'version'"))?;
    let max_tokens = args.get("max_tokens").and_then(|v| v.as_u64()).unwrap_or(2000) as usize;
    let include_examples = args.get("include_examples").and_then(|v| v.as_bool()).unwrap_or(false);

    match docbrain_ingest::retrieve::get_docs(store, slug, version, max_tokens, include_examples)? {
        docbrain_ingest::retrieve::GetDocsOutcome::NotFound { available_versions } => {
            Ok(format!("No docs found for {slug}@{version}. Available versions: {available_versions:?}"))
        }
        docbrain_ingest::retrieve::GetDocsOutcome::Found { docs, hidden_examples } => Ok(format_doc_results(&docs, hidden_examples)),
    }
}

fn tool_search_docs(store: &dyn DocbrainStore, _db_path: &Path, args: &Value) -> Result<String> {
    let query = get_str(args, "query").ok_or_else(|| anyhow::anyhow!("missing required 'query'"))?;
    let slug = get_str(args, "slug");
    let top_k = args.get("top_k").and_then(|v| v.as_u64()).unwrap_or(5) as usize;
    let max_tokens = args.get("max_tokens").and_then(|v| v.as_u64()).unwrap_or(1500) as usize;
    let include_examples = args.get("include_examples").and_then(|v| v.as_bool()).unwrap_or(false);

    let docs = docbrain_ingest::retrieve::search_docs(store, &docbrain_ingest::LocalEmbedder, query, slug, top_k, max_tokens, include_examples)?;
    if docs.is_empty() {
        return Ok("No matching documentation found.".to_string());
    }
    Ok(format_doc_results(&docs, 0))
}

fn tool_get_code_examples(store: &dyn DocbrainStore, _db_path: &Path, args: &Value) -> Result<String> {
    let node_id = args.get("node_id").and_then(|v| v.as_i64()).ok_or_else(|| anyhow::anyhow!("missing required 'node_id'"))?;
    let examples = docbrain_ingest::retrieve::get_code_examples(store, node_id)?;
    if examples.is_empty() {
        return Ok(format!("No code examples linked to node {node_id}."));
    }
    Ok(examples.iter().map(|n| format!("## {}\n{}", n.topic, n.content)).collect::<Vec<_>>().join("\n\n"))
}

/// Formats one `docbrain_ingest::retrieve::DocResult` per the kind of
/// result it is (a direct hit with a distance, an edge-walked related
/// node, or a plain dump entry with neither) — display-only, no retrieval
/// logic of its own.
fn format_doc_results(docs: &[docbrain_ingest::retrieve::DocResult], hidden_examples: usize) -> String {
    let mut out = docs
        .iter()
        .map(|d| match (d.distance, d.related_via) {
            (Some(distance), _) => format!("## {} (node {}, distance {distance:.3})\n{}", d.node.topic, d.node.id, d.node.content),
            (None, Some(relation)) => format!("### (node {}, related via {relation:?}) {}\n{}", d.node.id, d.node.topic, d.node.content),
            (None, None) => format!("## {} (node {})\n{}", d.node.topic, d.node.id, d.node.content),
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    if hidden_examples > 0 {
        out.push_str(&format!(
            "\n\n({hidden_examples} code example node(s) hidden — call get_code_examples on a node above, or re-run with include_examples: true.)"
        ));
    }
    out
}

fn tool_get_changelog(store: &dyn DocbrainStore, _db_path: &Path, args: &Value) -> Result<String> {
    let slug = get_str(args, "slug").ok_or_else(|| anyhow::anyhow!("missing required 'slug'"))?;
    let from = get_str(args, "from_version").ok_or_else(|| anyhow::anyhow!("missing required 'from_version'"))?;
    let to = get_str(args, "to_version").ok_or_else(|| anyhow::anyhow!("missing required 'to_version'"))?;
    let entries = store.get_changelog(slug, from, to)?;
    if entries.is_empty() {
        return Ok(format!("No changelog entries recorded between {from} and {to} for {slug}."));
    }
    Ok(entries.iter().map(|e| format!("{}{}", if e.breaking { "[BREAKING] " } else { "" }, e.entry)).collect::<Vec<_>>().join("\n"))
}

fn tool_get_job_status(store: &dyn DocbrainStore, _db_path: &Path, args: &Value) -> Result<String> {
    let job_id = args.get("job_id").and_then(|v| v.as_i64()).ok_or_else(|| anyhow::anyhow!("missing required 'job_id'"))?;
    let job = store.get_job(job_id)?.ok_or_else(|| anyhow::anyhow!("no job {job_id}"))?;
    let status_str = match job.status {
        JobStatus::Running => "running",
        JobStatus::Succeeded => "succeeded",
        JobStatus::Failed => "failed",
    };
    Ok(match job.message {
        Some(msg) => format!("job {} [{}] {} — {status_str}\n{msg}", job.id, job.job_type, job.slug),
        None => format!("job {} [{}] {} — {status_str}", job.id, job.job_type, job.slug),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use docbrain_graph::{NodeKind, SqliteDocbrainStore};
    use docbrain_ingest::Embedder;

    /// Closes the exact gap `main` left open: every tool `list_tools()`
    /// advertises must actually be dispatchable, not just present in the
    /// list.
    #[test]
    fn every_listed_tool_is_dispatchable() {
        let store = SqliteDocbrainStore::open_in_memory().unwrap();
        for tool in list_tools() {
            let result = call_tool(&store, Path::new("unused.db"), tool.name, &json!({}));
            assert!(result.is_ok(), "tool '{}' must be dispatchable (missing-args errors are fine, 'unknown tool' is not)", tool.name);
        }
    }

    #[test]
    fn unknown_tool_name_is_rejected_before_dispatch() {
        let store = SqliteDocbrainStore::open_in_memory().unwrap();
        let err = call_tool(&store, Path::new("unused.db"), "totally_made_up_tool", &json!({})).unwrap_err();
        assert!(err.contains("unknown tool"));
    }

    #[test]
    fn get_docs_stops_before_exceeding_the_token_budget() {
        let store = SqliteDocbrainStore::open_in_memory().unwrap();
        store.add_library("next", "Next.js", None, None).unwrap();
        for i in 0..5 {
            let content = format!("chunk {i} content");
            let hash = docbrain_graph::content_hash(&content);
            store
                .upsert_doc_node(
                    "next",
                    docbrain_graph::NewDocNode { version: "1.0", topic: &format!("topic {i}"), content: &content, content_hash: &hash, token_count: 100, embedding: None, kind: NodeKind::Prose },
                )
                .unwrap();
        }

        let result = tool_get_docs(&store, Path::new("unused.db"), &json!({ "slug": "next", "version": "1.0", "max_tokens": 250 })).unwrap();
        let sections = result.matches("## topic").count();
        assert!(sections >= 1 && sections < 5, "expected the token budget to cut off before all 5 chunks, got {sections}");
    }

    /// End-to-end with the real local embedding model (already
    /// downloaded/cached by `docbrain-ingest`'s own tests): a semantic
    /// query should surface its nearest node plus its edge-connected
    /// neighbor, and the whole assembled response should respect the
    /// token budget.
    #[test]
    fn search_docs_returns_a_hit_plus_its_edge_connected_context_within_budget() {
        let store = SqliteDocbrainStore::open_in_memory().unwrap();
        store.add_library("next", "Next.js", None, None).unwrap();

        let main_text = "How do I configure the API key for authentication?";
        let neighbor_text = "Set the API_KEY environment variable before starting the server.";
        let unrelated_text = "The changelog for version 2.0 includes performance improvements.";

        let main_embedding = docbrain_ingest::LocalEmbedder.embed(main_text).unwrap();
        let neighbor_embedding = docbrain_ingest::LocalEmbedder.embed(neighbor_text).unwrap();
        let unrelated_embedding = docbrain_ingest::LocalEmbedder.embed(unrelated_text).unwrap();

        let main_hash = docbrain_graph::content_hash(main_text);
        let neighbor_hash = docbrain_graph::content_hash(neighbor_text);
        let unrelated_hash = docbrain_graph::content_hash(unrelated_text);

        let main_id = store
            .upsert_doc_node(
                "next",
                docbrain_graph::NewDocNode { version: "1.0", topic: "Auth", content: main_text, content_hash: &main_hash, token_count: 12, embedding: Some(&main_embedding), kind: NodeKind::Prose },
            )
            .unwrap()
            .node_id();
        let neighbor_id = store
            .upsert_doc_node(
                "next",
                docbrain_graph::NewDocNode {
                    version: "1.0",
                    topic: "Auth Setup",
                    content: neighbor_text,
                    content_hash: &neighbor_hash,
                    token_count: 12,
                    embedding: Some(&neighbor_embedding),
                    kind: NodeKind::Prose,
                },
            )
            .unwrap()
            .node_id();
        store
            .upsert_doc_node(
                "next",
                docbrain_graph::NewDocNode {
                    version: "1.0",
                    topic: "Changelog",
                    content: unrelated_text,
                    content_hash: &unrelated_hash,
                    token_count: 12,
                    embedding: Some(&unrelated_embedding),
                    kind: NodeKind::Prose,
                },
            )
            .unwrap();
        store.add_doc_edge(main_id, neighbor_id, docbrain_graph::EdgeRelation::Sequence).unwrap();

        let result =
            tool_search_docs(&store, Path::new("unused.db"), &json!({ "query": "authentication API key setup", "top_k": 1, "max_tokens": 1000 })).unwrap();

        assert!(result.contains("Auth"), "expected the semantically closest node to be returned");
        assert!(result.contains("Auth Setup"), "expected the edge-connected neighbor to be pulled in as related context");
        assert!(!result.contains("Changelog"), "the unrelated node must not appear — search_docs is not a full dump");
    }

    /// The actual planning/implementation split this feature exists for:
    /// a default (planning-stage) `search_docs` call must not leak the
    /// linked code example, either as a hit or via edge-walk — but once the
    /// caller has the prose node's id, `get_code_examples` (the
    /// implementation stage) must return it.
    #[test]
    fn search_docs_excludes_code_examples_by_default_but_get_code_examples_returns_them() {
        let store = SqliteDocbrainStore::open_in_memory().unwrap();
        store.add_library("next", "Next.js", None, None).unwrap();

        let prose_text = "Wrap the segment name in square brackets to create a dynamic route.";
        let code_text = "```tsx\nexport default function BlogPostPage() {}\n```";
        let prose_embedding = docbrain_ingest::LocalEmbedder.embed(prose_text).unwrap();
        let code_embedding = docbrain_ingest::LocalEmbedder.embed(code_text).unwrap();
        let prose_hash = docbrain_graph::content_hash(prose_text);
        let code_hash = docbrain_graph::content_hash(code_text);

        let prose_id = store
            .upsert_doc_node(
                "next",
                docbrain_graph::NewDocNode {
                    version: "1.0",
                    topic: "Dynamic routes",
                    content: prose_text,
                    content_hash: &prose_hash,
                    token_count: 12,
                    embedding: Some(&prose_embedding),
                    kind: NodeKind::Prose,
                },
            )
            .unwrap()
            .node_id();
        let code_id = store
            .upsert_doc_node(
                "next",
                docbrain_graph::NewDocNode {
                    version: "1.0",
                    topic: "Dynamic routes (code example: tsx)",
                    content: code_text,
                    content_hash: &code_hash,
                    token_count: 12,
                    embedding: Some(&code_embedding),
                    kind: NodeKind::CodeExample,
                },
            )
            .unwrap()
            .node_id();
        store.add_doc_edge(prose_id, code_id, docbrain_graph::EdgeRelation::HasExample).unwrap();

        let planning = tool_search_docs(&store, Path::new("unused.db"), &json!({ "query": "how to create a dynamic route segment", "top_k": 5 })).unwrap();
        assert!(planning.contains("Dynamic routes"), "expected the prose node to be found");
        assert!(!planning.contains("BlogPostPage"), "planning-stage search must not leak the linked code example");

        let implementation = tool_get_code_examples(&store, Path::new("unused.db"), &json!({ "node_id": prose_id })).unwrap();
        assert!(implementation.contains("BlogPostPage"), "get_code_examples must return the code linked to the prose node");

        let planning_with_examples =
            tool_search_docs(&store, Path::new("unused.db"), &json!({ "query": "how to create a dynamic route segment", "top_k": 5, "include_examples": true }))
                .unwrap();
        assert!(planning_with_examples.contains("BlogPostPage"), "include_examples: true must surface the code example");
    }
}
