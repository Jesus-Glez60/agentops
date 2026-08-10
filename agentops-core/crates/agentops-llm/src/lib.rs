//! On-demand LLM interpretation of scanned code (Anthropic Messages API),
//! stored as `Definition` nodes edge-connected to the symbol they explain
//! via `EdgeRelation::Documents`. Deliberately never called automatically
//! during a scan — `explain_symbol` is opt-in, triggered per-symbol by an
//! agent/CLI, not a step `scan_and_persist` runs on every file.
//!
//! Also owns the two LLM-assisted adapters for `agentops-notes`' ports:
//! `LlmAssistedMatcher` (`SymbolMatcher`) and `LlmAssistedClassifier`
//! (`NoteClassifier`) — both live here rather than in `agentops-notes`
//! itself so that crate never gains a network dependency.

use std::path::Path;

use agentops_graph::{upsert_node, EdgeRelation, GraphStore, NewNode, Node, NodeKind};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

const API_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_MODEL: &str = "claude-sonnet-5";
const DEFAULT_MAX_TOKENS: u32 = 1024;

/// Anthropic API configuration — `AGENTOPS_ANTHROPIC_API_KEY` follows the
/// `AGENTOPS_<SUBSYSTEM>_<PURPOSE>` env var convention.
#[derive(Debug, Clone)]
pub struct AnthropicConfig {
    pub api_key: String,
    pub model: String,
    pub max_tokens: u32,
    /// Overridable so tests can point at a `wiremock` server instead of the
    /// real API.
    pub api_url: String,
}

impl AnthropicConfig {
    /// Reads `AGENTOPS_ANTHROPIC_API_KEY` from the environment. Returns a
    /// clear, typed error when it's unset — callers should surface this as
    /// "not configured", not a silent no-op.
    pub fn from_env() -> Result<Self> {
        let api_key = std::env::var("AGENTOPS_ANTHROPIC_API_KEY")
            .context("AGENTOPS_ANTHROPIC_API_KEY is not set — code interpretation is opt-in and requires your own Anthropic API key")?;
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

/// One Anthropic API call's result — token counts are exposed so callers
/// can log real cost rather than this crate silently discarding them.
#[derive(Debug, Clone)]
pub struct LlmCallResult {
    pub text: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// Calls the Anthropic Messages API with a single user-role `prompt`. Maps
/// 401 (bad/missing key) and 429 (rate limit) to distinct, clearly-labeled
/// errors rather than a generic transport failure.
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
    let text =
        parsed.content.into_iter().find(|c| c.kind == "text").and_then(|c| c.text).ok_or_else(|| anyhow::anyhow!("Anthropic response had no text content block"))?;

    Ok(LlmCallResult { text, input_tokens: parsed.usage.input_tokens, output_tokens: parsed.usage.output_tokens })
}

/// Builds the prompt for explaining `symbol` — its full source, plus
/// light, cheap context: the symbol's file's `DependsOn` targets
/// (file-level, not a transitive closure) and any `Gotcha`/`Decision` notes
/// already `Affects`-connected to it. Does NOT include full file content or
/// sibling symbols' source — both cost and prompt-injection surface scale
/// with what's included here.
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
            prompt.push_str(&format!("- [{kind:?}] {title}: {text}\n"));
        }
    }

    prompt.push_str("\nRespond with the explanation only, no preamble.");
    prompt
}

/// Explains `symbol_id` (must be a `NodeKind::Symbol` node in `repo`) via
/// the Anthropic API and persists the result as a `NodeKind::Definition`
/// node connected via `EdgeRelation::Documents`. Uses `upsert_node`'s
/// natural key so re-running this on an unchanged symbol updates the
/// existing `Definition` in place instead of duplicating it. Returns the
/// `Definition` node's id.
pub fn explain_symbol(store: &dyn GraphStore, config: &AnthropicConfig, repo: &str, symbol_id: i64) -> Result<i64> {
    let symbol = store.get_node(repo, symbol_id)?.ok_or_else(|| anyhow::anyhow!("no node #{symbol_id} in repo {repo:?}"))?;
    if symbol.kind != NodeKind::Symbol {
        anyhow::bail!("node #{symbol_id} is a {:?}, not a Symbol", symbol.kind);
    }

    let dep_paths = symbol
        .path
        .as_deref()
        .and_then(|path| store.find_node(repo, NodeKind::File, Some(path), None, None).ok().flatten())
        .map(|file_node| {
            store
                .edges_from(repo, file_node.id)
                .unwrap_or_default()
                .into_iter()
                .filter(|e| e.relation == EdgeRelation::DependsOn)
                .filter_map(|e| store.get_node(repo, e.dst_id).ok().flatten())
                .filter_map(|n| n.path)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let existing_notes: Vec<(NodeKind, String, String)> = store
        .edges_to(repo, symbol_id)?
        .into_iter()
        .filter(|e| e.relation == EdgeRelation::Affects)
        .filter_map(|e| store.get_node(repo, e.src_id).ok().flatten())
        .filter(|n| matches!(n.kind, NodeKind::Gotcha | NodeKind::Decision))
        .map(|n| (n.kind, n.name.clone().unwrap_or_default(), n.content.clone().unwrap_or_default()))
        .collect();

    let prompt = build_prompt(&symbol, &dep_paths, &existing_notes);
    let result = call_anthropic(config, &prompt)?;

    let definition_id = upsert_node(
        store,
        NewNode {
            kind: NodeKind::Definition,
            repo: symbol.repo.clone(),
            path: symbol.path.clone(),
            name: symbol.name.clone(),
            container: symbol.container.clone(),
            start_line: None,
            end_line: None,
            content: Some(result.text),
        },
    )?;

    // `add_edge` has no upsert semantics (unlike nodes) — guard against a
    // duplicate `Documents` edge on every re-run of an unchanged symbol.
    let already_connected = store.edges_from(repo, definition_id)?.iter().any(|e| e.dst_id == symbol_id && e.relation == EdgeRelation::Documents);
    if !already_connected {
        store.add_edge(repo, definition_id, symbol_id, EdgeRelation::Documents)?;
    }

    Ok(definition_id)
}

/// Looks up a symbol by name, optionally narrowed by file path. `name` may
/// be qualified as `Container::bare_name` (e.g. `NodeKind::as_db_str`) to
/// disambiguate symbols that share a bare name within one file (different
/// `impl` blocks/classes reusing a method name — a confirmed real
/// collision, not hypothetical) — plain `find_node` can't be used directly
/// here since a caller (a human or an agent) types a bare name without
/// knowing a symbol's `container`, so this always searches by name first
/// and only errors out on genuine ambiguity, naming the fix (qualify with
/// `Container::name`) in the message.
pub fn find_symbol_by_name(store: &dyn GraphStore, repo: &str, name: &str, path: Option<&Path>) -> Result<i64> {
    let (container_filter, bare_name) = match name.rsplit_once("::") {
        Some((container, rest)) => (Some(container), rest),
        None => (None, name),
    };
    let path_str = path.map(|p| p.to_string_lossy().into_owned());

    let matches: Vec<Node> = store
        .nodes_by_kind(repo, NodeKind::Symbol)?
        .into_iter()
        .filter(|n| n.name.as_deref() == Some(bare_name))
        .filter(|n| path_str.is_none() || n.path.as_deref() == path_str.as_deref())
        .filter(|n| container_filter.is_none() || n.container.as_deref() == container_filter)
        .collect();

    let location = path.map(|p| format!(" in {}", p.display())).unwrap_or_else(|| " in this repo".to_string());
    match matches.len() {
        0 => anyhow::bail!("no symbol named {name:?} found{location}"),
        1 => Ok(matches[0].id),
        n => anyhow::bail!("{n} symbols named {bare_name:?} found{location} — qualify with its container, e.g. \"Container::{bare_name}\""),
    }
}

/// Re-ranks `agentops_notes::match_symbols`' cheap, already-narrowed
/// shortlist with one Anthropic API call per candidate-bearing note — never
/// the whole repo's symbol table.
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
        let named: Vec<(i64, String)> =
            candidates.into_iter().filter_map(|(id, _)| store.get_node(repo, id).ok().flatten().and_then(|n| n.name).map(|name| (id, name))).collect();
        let list = named.iter().map(|(_, n)| n.as_str()).collect::<Vec<_>>().join(", ");
        let prompt = format!(
            "A project note says:\n\n{note_body}\n\nWhich of these candidate code symbol names, if any, does this note actually describe? Candidates: {list}\n\nReply with ONLY a comma-separated list of the matching names exactly as given, or NONE if none apply. No other text."
        );
        let result = call_anthropic(self.config, &prompt)?;
        let picked: std::collections::HashSet<String> = result.text.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty() && s.to_uppercase() != "NONE").collect();
        Ok(named.into_iter().filter(|(_, name)| picked.contains(name)).map(|(id, _)| id).collect())
    }
}

/// The `NoteClassifier` adapter for genuinely ambiguous freeform notes.
/// First runs `agentops_notes::heuristic_score` — if it's confident, this
/// returns immediately with zero API calls; only a real tie (including "no
/// keywords at all") costs one bounded Anthropic call.
pub struct LlmAssistedClassifier<'a> {
    pub config: &'a AnthropicConfig,
}

impl agentops_notes::NoteClassifier for LlmAssistedClassifier<'_> {
    fn classify(&self, note_body: &str) -> Result<agentops_notes::NoteType> {
        let (heuristic_type, confident) = agentops_notes::heuristic_score(note_body);
        if confident {
            return Ok(heuristic_type);
        }

        let prompt = format!(
            "Classify the following project note as exactly one of: gotcha, decision, knowledge.\n\
             - gotcha: a workaround, bug, or known issue with existing code/behavior.\n\
             - decision: a record of a choice made and why (an alternative considered or rejected).\n\
             - knowledge: general context or documentation that is neither of the above.\n\n\
             Note:\n{note_body}\n\n\
             Reply with exactly one word: gotcha, decision, or knowledge."
        );
        let result = call_anthropic(self.config, &prompt)?;
        let answer = result.text.trim().to_lowercase();
        Ok(if answer.contains("gotcha") {
            agentops_notes::NoteType::Gotcha
        } else if answer.contains("decision") {
            agentops_notes::NoteType::Decision
        } else {
            agentops_notes::NoteType::Knowledge
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentops_graph::SqliteGraphStore;
    use agentops_notes::NoteClassifier;

    fn symbol_node(store: &dyn GraphStore, repo: &str, path: &str, name: &str, source: &str) -> i64 {
        upsert_node(store, NewNode { kind: NodeKind::Symbol, repo: repo.into(), path: Some(path.into()), name: Some(name.into()), container: None, start_line: Some(1), end_line: Some(2), content: Some(source.into()) })
            .unwrap()
    }

    fn mock_config(base_url: &str) -> AnthropicConfig {
        AnthropicConfig { api_key: "test-key".into(), model: "claude-sonnet-5".into(), max_tokens: 1024, api_url: format!("{base_url}/v1/messages") }
    }

    #[test]
    fn build_prompt_includes_source_deps_and_existing_notes() {
        let node = Node {
            id: 1,
            kind: NodeKind::Symbol,
            repo: "demo".into(),
            path: Some("src/auth.rs".into()),
            name: Some("verify_token".into()),
            container: None,
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
        let node = store.get_node("demo", id).unwrap().unwrap();
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
        assert!(err.to_string().to_lowercase().contains("rate limit"));
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

        let definition_id = explain_symbol(&store, &config, "demo", symbol_id).unwrap();
        let definition = store.get_node("demo", definition_id).unwrap().unwrap();
        assert_eq!(definition.kind, NodeKind::Definition);
        assert_eq!(definition.content.as_deref(), Some("Verifies a bearer token against the auth service."));

        let edges = store.edges_from("demo", definition_id).unwrap();
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

        let first_id = explain_symbol(&store, &config, "demo", symbol_id).unwrap();
        let second_id = explain_symbol(&store, &config, "demo", symbol_id).unwrap();

        assert_eq!(first_id, second_id, "re-running on an unchanged symbol must reuse the Definition node's id");
        assert_eq!(store.nodes_by_kind("demo", NodeKind::Definition).unwrap().len(), 1);
        assert_eq!(store.edges_from("demo", first_id).unwrap().len(), 1, "must not duplicate the Documents edge on re-run");
    }

    #[tokio::test]
    async fn llm_assisted_classifier_skips_the_api_call_when_the_heuristic_is_confident() {
        // No mock server mounted at all — if this made an API call, it
        // would fail to connect and the test would error out.
        let config = AnthropicConfig { api_key: "unused".into(), model: "claude-sonnet-5".into(), max_tokens: 1024, api_url: "http://127.0.0.1:1/v1/messages".into() };
        let classifier = LlmAssistedClassifier { config: &config };

        let result = classifier.classify("There's a known workaround for this bug that fails when the cache is cold.").unwrap();
        assert_eq!(result, agentops_notes::NoteType::Gotcha, "a confident heuristic match must never reach the network");
    }

    #[tokio::test]
    async fn llm_assisted_classifier_calls_the_api_only_when_the_heuristic_is_inconclusive() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "content": [{"type": "text", "text": "decision"}],
                "usage": {"input_tokens": 5, "output_tokens": 1}
            })))
            .mount(&server)
            .await;

        let config = mock_config(&server.uri());
        let classifier = LlmAssistedClassifier { config: &config };

        // No gotcha/decision keywords at all -- genuinely ambiguous.
        let result = classifier.classify("The onboarding flow now supports three languages.").unwrap();
        assert_eq!(result, agentops_notes::NoteType::Decision);
    }
}
