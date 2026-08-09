//! On-demand LLM interpretation of scanned code (Anthropic Messages API),
//! stored as `Definition` nodes edge-connected to the symbol they explain
//! via `EdgeRelation::Documents`. Deliberately never called automatically
//! during a scan — `explain_symbol` is opt-in, triggered per-symbol by an
//! agent/CLI, not a step `agentops-mcp::scan::persist` runs on every file.
//!
//! Lives in the root (light-tier) workspace as a sibling to `agentops-notes`
//! rather than `agentops-heavy`: the root `deny.toml`'s zero-runtime-network
//! ban ("agentops-scanner must never gain a RUNTIME HTTP/networking
//! dependency") is scoped to the scanner specifically, not this workspace as
//! a whole — `docbrain-ingest` already makes real runtime network calls
//! (scraping public library docs) from inside this same workspace using
//! `ureq`, which is not on the ban list. This crate follows that precedent.

use std::path::Path;

use agentops_graph::{EdgeRelation, GraphStore, NewNode, Node, NodeKind};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

const API_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_MODEL: &str = "claude-sonnet-5";
const DEFAULT_MAX_TOKENS: u32 = 1024;

/// Anthropic API configuration — `AGENTOPS_ANTHROPIC_API_KEY` follows the
/// existing `AGENTOPS_<SUBSYSTEM>_<PURPOSE>` env var convention
/// (`AGENTOPS_LICENSE_KEY`, `AGENTOPS_QDRANT_URL`, `AGENTOPS_SECRETS_MASTER_KEY`).
#[derive(Debug, Clone)]
pub struct AnthropicConfig {
    pub api_key: String,
    pub model: String,
    pub max_tokens: u32,
    /// Overridable so tests can point at a `wiremock` server instead of the
    /// real API — not meant to be set outside tests.
    pub api_url: String,
}

impl AnthropicConfig {
    /// Reads `AGENTOPS_ANTHROPIC_API_KEY` from the environment. Returns a
    /// clear, typed error (never a silent no-op) when it's unset — callers
    /// should surface this as "not configured", matching
    /// `agentops-heavy-api`'s existing 402-style pattern for other
    /// paid/optional features.
    pub fn from_env() -> Result<Self> {
        let api_key = std::env::var("AGENTOPS_ANTHROPIC_API_KEY").context("AGENTOPS_ANTHROPIC_API_KEY is not set — code interpretation is opt-in and requires your own Anthropic API key")?;
        Ok(Self { api_key, model: DEFAULT_MODEL.to_string(), max_tokens: DEFAULT_MAX_TOKENS, api_url: API_URL.to_string() })
    }
}

#[derive(Serialize)]
struct MessagesRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    messages: Vec<MessageIn<'a>>,
}

#[derive(Serialize)]
struct MessageIn<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Deserialize)]
struct MessagesResponse {
    content: Vec<ContentBlock>,
    usage: Usage,
}

#[derive(Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    kind: String,
    text: Option<String>,
}

#[derive(Deserialize)]
struct Usage {
    input_tokens: u64,
    output_tokens: u64,
}

/// One Anthropic API call's result — `input_tokens`/`output_tokens` are
/// exposed so callers can log real cost (see the plan's "cost/rate-limiting
/// at volume" risk) rather than this crate silently discarding them.
#[derive(Debug, Clone)]
pub struct LlmCallResult {
    pub text: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// Calls the Anthropic Messages API with a single user-role `prompt`. Maps
/// 401 (bad/missing key) and 429 (rate limit) to distinct, clearly-labeled
/// errors rather than a generic transport failure — both are realistic,
/// actionable outcomes for a caller looping this over many symbols.
pub fn call_anthropic(config: &AnthropicConfig, prompt: &str) -> Result<LlmCallResult> {
    let request = MessagesRequest { model: &config.model, max_tokens: config.max_tokens, messages: vec![MessageIn { role: "user", content: prompt }] };

    let mut response = ureq::post(&config.api_url)
        .header("x-api-key", &config.api_key)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .config()
        .http_status_as_error(false)
        .build()
        .send_json(&request)
        .context("calling the Anthropic Messages API")?;

    let status = response.status();
    if status.as_u16() == 401 {
        anyhow::bail!("Anthropic API rejected the key (401) — check AGENTOPS_ANTHROPIC_API_KEY");
    }
    if status.as_u16() == 429 {
        anyhow::bail!("Anthropic API rate limit hit (429) — retry later");
    }
    if !status.is_success() {
        let body = response.body_mut().read_to_string().unwrap_or_default();
        anyhow::bail!("Anthropic API returned {status}: {body}");
    }

    let parsed: MessagesResponse = response.body_mut().read_json().context("parsing Anthropic API response")?;
    let text = parsed
        .content
        .into_iter()
        .find(|c| c.kind == "text")
        .and_then(|c| c.text)
        .ok_or_else(|| anyhow::anyhow!("Anthropic response had no text content block"))?;

    Ok(LlmCallResult { text, input_tokens: parsed.usage.input_tokens, output_tokens: parsed.usage.output_tokens })
}

/// Builds the prompt for explaining `symbol` — its full source (already
/// captured by `agentops-scanner`, not truncated), plus light, cheap
/// context: the symbol's file's `DependsOn` targets (file-level, not a full
/// transitive closure) and any `Gotcha`/`Decision` notes already `Affects`-
/// connected to it (already-stored short text, no extra API call). Does
/// NOT include full file content or sibling symbols' source by default —
/// both cost and prompt-injection surface scale with what's included here.
fn build_prompt(symbol: &Node, dep_paths: &[String], existing_notes: &[(NodeKind, String, String)]) -> String {
    let mut prompt = format!(
        "You are documenting a codebase. Explain concisely what this symbol does and why it might exist, for a developer or AI agent reading the code for the first time.\n\n\
         Symbol: {}\nFile: {}\n\n```\n{}\n```\n",
        symbol.name.as_deref().unwrap_or("<unnamed>"),
        symbol.path.as_deref().unwrap_or("<unknown>"),
        symbol.content.as_deref().unwrap_or(""),
    );

    if !dep_paths.is_empty() {
        prompt.push_str(&format!("\nThis file depends on: {}\n", dep_paths.join(", ")));
    }

    if !existing_notes.is_empty() {
        prompt.push_str("\nKnown notes already recorded against this symbol (acknowledge these if relevant, don't contradict them):\n");
        for (kind, title, text) in existing_notes {
            prompt.push_str(&format!("- [{}] {title}: {text}\n", kind.as_str()));
        }
    }

    prompt.push_str("\nRespond with the explanation only, no preamble.");
    prompt
}

/// Explains `symbol_id` (must be a `NodeKind::Symbol` node) via the
/// Anthropic API and persists the result as a `NodeKind::Definition` node
/// connected via `EdgeRelation::Documents`. Uses `upsert_node`'s natural key
/// (`repo, kind, path, name`) so re-running this on an unchanged symbol
/// updates the existing `Definition` in place instead of duplicating it —
/// the same rescan-safety `agentops-mcp::scan::persist` relies on for
/// File/Symbol nodes. Returns the `Definition` node's id.
pub fn explain_symbol(store: &dyn GraphStore, config: &AnthropicConfig, symbol_id: i64) -> Result<i64> {
    let symbol = store.get_node(symbol_id)?.ok_or_else(|| anyhow::anyhow!("no node #{symbol_id}"))?;
    if symbol.kind != NodeKind::Symbol {
        anyhow::bail!("node #{symbol_id} is a {:?}, not a Symbol", symbol.kind);
    }

    let dep_paths = symbol
        .path
        .as_deref()
        .and_then(|path| store.find_node(&symbol.repo, NodeKind::File, Some(path), None).ok().flatten())
        .map(|file_node| {
            store
                .edges_from(file_node.id)
                .unwrap_or_default()
                .into_iter()
                .filter(|e| e.relation == EdgeRelation::DependsOn)
                .filter_map(|e| store.get_node(e.dst_id).ok().flatten())
                .filter_map(|n| n.path)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let existing_notes: Vec<(NodeKind, String, String)> = store
        .edges_to(symbol_id)?
        .into_iter()
        .filter(|e| e.relation == EdgeRelation::Affects)
        .filter_map(|e| store.get_node(e.src_id).ok().flatten())
        .filter(|n| matches!(n.kind, NodeKind::Gotcha | NodeKind::Decision))
        .map(|n| (n.kind, n.name.clone().unwrap_or_default(), n.content.clone().unwrap_or_default()))
        .collect();

    let prompt = build_prompt(&symbol, &dep_paths, &existing_notes);
    let result = call_anthropic(config, &prompt)?;

    let definition_id = agentops_graph::upsert_node(
        store,
        NewNode {
            kind: NodeKind::Definition,
            repo: symbol.repo.clone(),
            path: symbol.path.clone(),
            name: symbol.name.clone(),
            start_line: None,
            end_line: None,
            content: Some(result.text),
        },
    )?;

    // `add_edge` has no upsert semantics (unlike nodes) -- guard against
    // adding a duplicate `Documents` edge on every re-run of an unchanged
    // symbol, which `upsert_node` alone wouldn't prevent.
    let already_connected = store.edges_from(definition_id)?.iter().any(|e| e.dst_id == symbol_id && e.relation == EdgeRelation::Documents);
    if !already_connected {
        store.add_edge(definition_id, symbol_id, EdgeRelation::Documents)?;
    }

    Ok(definition_id)
}

/// Looks up a symbol by name within one file, the way `explain_symbol`'s
/// MCP/CLI callers resolve a caller-given name+path into an id before
/// calling `explain_symbol` itself. Deliberately NOT
/// `agentops_notes::resolve_symbol_by_name` — that function has no `repo`
/// filter at all and silently first-match-wins across an ambiguous name;
/// this is a single, exact `(repo, kind, path, name)` lookup via
/// `GraphStore::find_node`, erroring clearly if `path` is omitted and the
/// name isn't unique within the repo, rather than guessing.
pub fn find_symbol_by_name(store: &dyn GraphStore, repo: &str, name: &str, path: Option<&Path>) -> Result<i64> {
    if let Some(path) = path {
        let path_str = path.to_string_lossy();
        let node = store
            .find_node(repo, NodeKind::Symbol, Some(&path_str), Some(name))?
            .ok_or_else(|| anyhow::anyhow!("no symbol named {name:?} found in {path_str}"))?;
        return Ok(node.id);
    }

    let matches: Vec<Node> = store.nodes_by_kind(NodeKind::Symbol)?.into_iter().filter(|n| n.repo == repo && n.name.as_deref() == Some(name)).collect();
    match matches.len() {
        0 => anyhow::bail!("no symbol named {name:?} found in this repo"),
        1 => Ok(matches[0].id),
        n => anyhow::bail!("{n} symbols named {name:?} found in this repo — pass --file to disambiguate"),
    }
}

/// Re-ranks `agentops_notes::match_symbols`' cheap, already-narrowed
/// shortlist with one Anthropic API call per candidate-bearing note — never
/// the whole repo's symbol table (cost + prompt-injection surface both
/// scale with what's included). This is the `--llm-match` seam
/// `agentops_notes::SymbolMatcher` exists for; lives here rather than in
/// `agentops-notes` so that crate never gains a network dependency, and
/// here rather than duplicated per-caller (CLI/MCP) so there's exactly one
/// implementation of "ask the LLM which shortlisted candidate a note means."
pub struct LlmAssistedMatcher<'a> {
    pub config: &'a AnthropicConfig,
    pub min_name_len: usize,
}

impl agentops_notes::SymbolMatcher for LlmAssistedMatcher<'_> {
    fn match_symbols(&self, store: &dyn GraphStore, repo: &str, note_body: &str) -> Result<Vec<i64>> {
        let candidates = agentops_notes::match_symbols(store, repo, note_body, self.min_name_len)?;
        if candidates.is_empty() {
            return Ok(vec![]);
        }
        let named: Vec<(i64, String)> = candidates
            .into_iter()
            .filter_map(|(id, _)| store.get_node(id).ok().flatten().and_then(|n| n.name).map(|name| (id, name)))
            .collect();
        let list = named.iter().map(|(_, n)| n.as_str()).collect::<Vec<_>>().join(", ");
        let prompt = format!(
            "A project note says:\n\n{note_body}\n\nWhich of these candidate code symbol names, if any, does this note actually describe? Candidates: {list}\n\nReply with ONLY a comma-separated list of the matching names exactly as given, or NONE if none apply. No other text."
        );
        let result = call_anthropic(self.config, &prompt)?;
        let picked: std::collections::HashSet<String> = result.text.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty() && s.to_uppercase() != "NONE").collect();
        Ok(named.into_iter().filter(|(_, name)| picked.contains(name)).map(|(id, _)| id).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentops_graph::SqliteGraphStore;

    fn symbol_node(store: &dyn GraphStore, repo: &str, path: &str, name: &str, source: &str) -> i64 {
        agentops_graph::upsert_node(
            store,
            NewNode {
                kind: NodeKind::Symbol,
                repo: repo.into(),
                path: Some(path.into()),
                name: Some(name.into()),
                start_line: Some(1),
                end_line: Some(2),
                content: Some(source.into()),
            },
        )
        .unwrap()
    }

    #[test]
    fn build_prompt_includes_source_deps_and_existing_notes() {
        let node = Node {
            id: 1,
            kind: NodeKind::Symbol,
            repo: "demo".into(),
            path: Some("src/auth.rs".into()),
            name: Some("verify_token".into()),
            start_line: Some(1),
            end_line: Some(5),
            content: Some("fn verify_token() {}".into()),
        };
        let prompt = build_prompt(&node, &["src/config.rs".to_string()], &[(NodeKind::Gotcha, "off-by-one".into(), "expiry bug".into())]);
        assert!(prompt.contains("verify_token"));
        assert!(prompt.contains("fn verify_token() {}"));
        assert!(prompt.contains("src/config.rs"));
        assert!(prompt.contains("off-by-one"));
        assert!(prompt.contains("expiry bug"));
    }

    #[test]
    fn find_symbol_by_name_disambiguates_by_path_when_given() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        symbol_node(&store, "demo", "a.rs", "run", "fn run() {}");
        symbol_node(&store, "demo", "b.rs", "run", "fn run() {}");

        let id = find_symbol_by_name(&store, "demo", "run", Some(Path::new("a.rs"))).unwrap();
        let node = store.get_node(id).unwrap().unwrap();
        assert_eq!(node.path.as_deref(), Some("a.rs"));
    }

    #[test]
    fn find_symbol_by_name_errors_clearly_on_ambiguity_without_a_path() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        symbol_node(&store, "demo", "a.rs", "run", "fn run() {}");
        symbol_node(&store, "demo", "b.rs", "run", "fn run() {}");

        let result = find_symbol_by_name(&store, "demo", "run", None);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("2 symbols"));
    }

    #[test]
    fn find_symbol_by_name_never_matches_a_different_repo() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        symbol_node(&store, "other-repo", "a.rs", "run", "fn run() {}");

        let result = find_symbol_by_name(&store, "demo", "run", None);
        assert!(result.is_err(), "a same-named symbol in a different repo must not match");
    }

    fn mock_config(base_url: &str) -> AnthropicConfig {
        AnthropicConfig { api_key: "test-key".into(), model: "claude-sonnet-5".into(), max_tokens: 1024, api_url: format!("{base_url}/v1/messages") }
    }

    #[tokio::test]
    async fn call_anthropic_parses_a_successful_response() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/v1/messages"))
            .and(wiremock::matchers::header("x-api-key", "test-key"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "content": [{"type": "text", "text": "This function verifies a token."}],
                "usage": {"input_tokens": 42, "output_tokens": 7}
            })))
            .mount(&server)
            .await;

        let result = call_anthropic(&mock_config(&server.uri()), "explain this").unwrap();
        assert_eq!(result.text, "This function verifies a token.");
        assert_eq!(result.input_tokens, 42);
        assert_eq!(result.output_tokens, 7);
    }

    #[tokio::test]
    async fn call_anthropic_maps_401_to_a_clear_key_error() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST")).respond_with(wiremock::ResponseTemplate::new(401)).mount(&server).await;

        let err = call_anthropic(&mock_config(&server.uri()), "x").unwrap_err();
        assert!(err.to_string().contains("401"));
        assert!(err.to_string().contains("AGENTOPS_ANTHROPIC_API_KEY"));
    }

    #[tokio::test]
    async fn call_anthropic_maps_429_to_a_distinct_rate_limit_error() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST")).respond_with(wiremock::ResponseTemplate::new(429)).mount(&server).await;

        let err = call_anthropic(&mock_config(&server.uri()), "x").unwrap_err();
        assert!(err.to_string().contains("429"));
        assert!(err.to_string().to_lowercase().contains("rate limit"), "429 must be distinguishable from a generic failure: {err}");
    }

    #[tokio::test]
    async fn explain_symbol_creates_a_definition_node_connected_via_documents() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "content": [{"type": "text", "text": "Verifies a bearer token against the auth service."}],
                "usage": {"input_tokens": 10, "output_tokens": 5}
            })))
            .mount(&server)
            .await;

        let store = SqliteGraphStore::open_in_memory().unwrap();
        let symbol_id = symbol_node(&store, "demo", "src/auth.rs", "verify_token", "fn verify_token() {}");
        let config = mock_config(&server.uri());

        let definition_id = explain_symbol(&store, &config, symbol_id).unwrap();
        let definition = store.get_node(definition_id).unwrap().unwrap();
        assert_eq!(definition.kind, NodeKind::Definition);
        assert_eq!(definition.content.as_deref(), Some("Verifies a bearer token against the auth service."));

        let edges = store.edges_from(definition_id).unwrap();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].dst_id, symbol_id);
        assert_eq!(edges[0].relation, EdgeRelation::Documents);
    }

    #[tokio::test]
    async fn re_explaining_the_same_symbol_updates_in_place_without_duplicating_the_edge() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "content": [{"type": "text", "text": "updated explanation"}],
                "usage": {"input_tokens": 1, "output_tokens": 1}
            })))
            .mount(&server)
            .await;

        let store = SqliteGraphStore::open_in_memory().unwrap();
        let symbol_id = symbol_node(&store, "demo", "src/auth.rs", "verify_token", "fn verify_token() {}");
        let config = mock_config(&server.uri());

        let first_id = explain_symbol(&store, &config, symbol_id).unwrap();
        let second_id = explain_symbol(&store, &config, symbol_id).unwrap();

        assert_eq!(first_id, second_id, "re-running on an unchanged symbol must reuse the Definition node's id");
        assert_eq!(store.nodes_by_kind(NodeKind::Definition).unwrap().len(), 1, "must not duplicate the Definition node");
        assert_eq!(store.edges_from(first_id).unwrap().len(), 1, "must not duplicate the Documents edge on re-run");
    }

    /// Only runs against the real Anthropic API when a key is actually
    /// configured -- skipped (not failed) otherwise, the same
    /// expensive/external-and-opt-in-in-CI pattern `agentops-embeddings`
    /// already uses via `AGENTOPS_TEST_QDRANT_URL`. This is what actually
    /// catches drift from Anthropic's real API shape, which the wiremock
    /// suite above can't (it only proves this crate matches the shape it
    /// was told to expect).
    #[test]
    fn call_anthropic_against_the_real_api_when_a_key_is_configured() {
        let Ok(api_key) = std::env::var("AGENTOPS_ANTHROPIC_API_KEY") else {
            eprintln!("skipping call_anthropic_against_the_real_api_when_a_key_is_configured: AGENTOPS_ANTHROPIC_API_KEY not set");
            return;
        };
        let config = AnthropicConfig { api_key, model: DEFAULT_MODEL.to_string(), max_tokens: 32, api_url: API_URL.to_string() };
        let result = call_anthropic(&config, "Reply with exactly one word: hello").unwrap();
        assert!(!result.text.trim().is_empty());
        assert!(result.input_tokens > 0);
    }
}
