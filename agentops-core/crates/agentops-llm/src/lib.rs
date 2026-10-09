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

pub mod librarian;
pub mod pricing;
mod tokens;
pub mod usage;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use agentops_graph::{upsert_node, EdgeRelation, GraphStore, ModuleLabel, NewNode, Node, NodeKind, NodeProminence};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use tokens::{count_tokens, truncate_to_tokens};
pub use usage::{LlmCallRecord, LlmErrorKind, LlmUsageSink, UsageRecorder};

const API_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_MODEL: &str = "claude-sonnet-5";
/// Cheap tier for automated calls with no direct human reader of the raw
/// output (note classification/symbol matching) — see
/// `AnthropicConfig::from_env_cheap`.
const DEFAULT_MODEL_CHEAP: &str = "claude-haiku-4-5-20251001";
const DEFAULT_MAX_TOKENS: u32 = 1024;
/// How many pattern-completed symbols (Initiative 4) `explain_symbol` pulls
/// in as possibly-related context -- most get filtered out by
/// `build_prompt`'s own "must have at least one note" check anyway, so this
/// is deliberately generous rather than tightly tuned.
const PATTERN_COMPLETE_K: usize = 5;
/// Input-token ceiling for `build_prompt`'s assembled prompt (counted via
/// `tokens::count_tokens`, `cl100k_base` as a practical proxy — see that
/// module's doc comment). A conservative, tunable starting point in the
/// same spirit as `docbrain-ingest/src/chunk.rs`'s `MAX_CHUNK_TOKENS`: not
/// a value with special significance, just comfortably under every current
/// Claude model's context window (including the smallest), so one budget
/// works regardless of which tier `AnthropicConfig` selected.
const MAX_PROMPT_INPUT_TOKENS: usize = 60_000;

/// Which wire protocol a `LlmConfig` speaks. `OpenAiCompatible` covers
/// any `/chat/completions` endpoint (Groq, Gemini's compat endpoint,
/// NVIDIA NIM, OpenRouter, ...); `name` is only the label recorded in the
/// spend ledger, and `api_url` must already be the full
/// `{base_url}/chat/completions` URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Provider {
    Anthropic,
    OpenAiCompatible { name: String, json_mode: JsonMode },
}

impl Provider {
    pub fn name(&self) -> &str {
        match self {
            Provider::Anthropic => "anthropic",
            Provider::OpenAiCompatible { name, .. } => name,
        }
    }
}

/// How an OpenAI-compatible request asks for structured JSON — providers
/// don't share one request shape (see the recorded gotcha on Groq/Gemini/
/// NVIDIA NIM structured output).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonMode {
    /// `response_format: {type: "json_schema", json_schema: {name, strict: true, schema}}` (Groq, select models).
    JsonSchemaStrict,
    /// `response_format: {type: "json_schema", json_schema: {name, schema}}` (Gemini compat, OpenAI-style).
    JsonSchema,
    /// `response_format: {type: "json_object"}` — valid JSON, no schema guarantee; prompt must mention JSON.
    JsonObject,
    /// `guided_json: <schema>` at the request body's top level (NVIDIA NIM).
    GuidedJson,
}

/// LLM API configuration — `AGENTOPS_ANTHROPIC_API_KEY` follows the
/// `AGENTOPS_<SUBSYSTEM>_<PURPOSE>` env var convention. `Debug` is
/// hand-written so the key never reaches a log line.
#[derive(Clone)]
pub struct LlmConfig {
    pub api_key: String,
    pub model: String,
    pub max_tokens: u32,
    /// Overridable so tests can point at a `wiremock` server instead of the
    /// real API. For `Provider::OpenAiCompatible`, the full
    /// `{base_url}/chat/completions` URL.
    pub api_url: String,
    pub provider: Provider,
    /// Receives one `LlmCallRecord` per call, failures included — see
    /// `usage`'s module doc comment.
    pub usage_sink: Option<Arc<dyn LlmUsageSink>>,
}

/// Kept so the many existing `AnthropicConfig` call sites and signatures
/// don't all have to change at once.
pub type AnthropicConfig = LlmConfig;

impl std::fmt::Debug for LlmConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LlmConfig")
            .field("model", &self.model)
            .field("max_tokens", &self.max_tokens)
            .field("api_url", &self.api_url)
            .field("provider", &self.provider)
            .field("usage_sink", &self.usage_sink.is_some())
            .finish_non_exhaustive()
    }
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self { api_key: String::new(), model: DEFAULT_MODEL.to_string(), max_tokens: DEFAULT_MAX_TOKENS, api_url: API_URL.to_string(), provider: Provider::Anthropic, usage_sink: None }
    }
}

impl LlmConfig {
    fn read_api_key() -> Result<String> {
        std::env::var("AGENTOPS_ANTHROPIC_API_KEY")
            .context("AGENTOPS_ANTHROPIC_API_KEY is not set — code interpretation is opt-in and requires your own Anthropic API key")
    }

    /// Reads `AGENTOPS_ANTHROPIC_API_KEY` from the environment. Returns a
    /// clear, typed error when it's unset — callers should surface this as
    /// "not configured", not a silent no-op. Model defaults to
    /// `DEFAULT_MODEL`, overridable via `AGENTOPS_ANTHROPIC_MODEL` — use this
    /// tier for opt-in/interactive calls (e.g. `explain_symbol`) where
    /// quality matters more than cost.
    pub fn from_env() -> Result<Self> {
        let api_key = Self::read_api_key()?;
        let model = std::env::var("AGENTOPS_ANTHROPIC_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string());
        Ok(Self { api_key, model, ..Default::default() })
    }

    /// Same as `from_env`, but selects the cheap tier (`DEFAULT_MODEL_CHEAP`,
    /// overridable via `AGENTOPS_ANTHROPIC_MODEL_CHEAP`) — use this only for
    /// calls whose raw output isn't read directly by a person
    /// (`LlmAssistedClassifier`/`LlmAssistedMatcher`: a re-ranking/
    /// classification signal, not prose a user reads). `group_core_modules`
    /// and `summarize_task_activity` were originally routed here too, but a
    /// wrap-skill council audit (2026-09-28) correctly flagged that as
    /// mis-scoped — their output is read directly (Documentation Viewer
    /// labels; Linear/stdout summaries), the same category as
    /// `explain_symbol`, so both now use `from_env` instead.
    pub fn from_env_cheap() -> Result<Self> {
        let api_key = Self::read_api_key()?;
        let model = std::env::var("AGENTOPS_ANTHROPIC_MODEL_CHEAP").unwrap_or_else(|_| DEFAULT_MODEL_CHEAP.to_string());
        Ok(Self { api_key, model, ..Default::default() })
    }

    pub fn with_usage_sink(mut self, sink: impl LlmUsageSink + 'static) -> Self {
        self.usage_sink = Some(Arc::new(sink));
        self
    }
}

#[derive(Serialize)]
struct MessagesRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    messages: Vec<MessageIn<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output_config: Option<OutputConfig>,
}

/// Anthropic's structured-outputs request shape — `output_config.format:
/// json_schema` guarantees a schema-conforming JSON response, rather than
/// this crate's older delimiter-tagged free-text parsing convention (see
/// `parse_task_summaries`). Only used by `group_core_modules` so far; every
/// other call site still passes `None` and gets free text back, unchanged.
#[derive(Serialize)]
struct OutputConfig {
    format: OutputFormat,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum OutputFormat {
    JsonSchema { schema: serde_json::Value },
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

/// One LLM API call's result — token counts are exposed so callers can log
/// real cost rather than this crate silently discarding them.
#[derive(Debug, Clone)]
pub struct LlmCallResult {
    pub text: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub provider: String,
    pub model: String,
    pub latency_ms: u64,
}

/// Calls the configured provider with a single user-role `prompt`. Maps
/// 401 (bad/missing key) and 429 (rate limit) to distinct, clearly-labeled
/// errors rather than a generic transport failure. `operation` is the
/// spend-ledger label (`explain_symbol`, `classify_note`, ...).
pub fn call_anthropic(config: &LlmConfig, operation: &str, prompt: &str) -> Result<LlmCallResult> {
    send_messages(config, operation, prompt, None)
}

/// Same as `call_anthropic`, but constrains the response to `schema` —
/// Anthropic's `output_config.format: json_schema`, or the provider's
/// `JsonMode` for an OpenAI-compatible config. A response that still isn't
/// valid JSON is recorded as `bad_json` and returned as an error.
pub fn call_json(config: &LlmConfig, operation: &str, prompt: &str, schema: serde_json::Value) -> Result<LlmCallResult> {
    send_messages(config, operation, prompt, Some(schema))
}

fn call_anthropic_json(config: &LlmConfig, operation: &str, prompt: &str, schema: serde_json::Value) -> Result<LlmCallResult> {
    call_json(config, operation, prompt, schema)
}

/// Every LLM call funnels through here, so every exit path — success,
/// HTTP failures, transport errors, and unparseable JSON — is reported to
/// `config.usage_sink` exactly once, before returning or propagating.
fn send_messages(config: &LlmConfig, operation: &str, prompt: &str, json_schema: Option<serde_json::Value>) -> Result<LlmCallResult> {
    let started = Instant::now();
    let report = |input_tokens: u64, output_tokens: u64, error_kind: Option<LlmErrorKind>| {
        if let Some(sink) = &config.usage_sink {
            sink.record(&LlmCallRecord {
                operation: operation.to_string(),
                provider: config.provider.name().to_string(),
                model: config.model.clone(),
                input_tokens,
                output_tokens,
                latency_ms: started.elapsed().as_millis() as u64,
                success: error_kind.is_none(),
                error_kind,
            });
        }
    };

    let wants_json = json_schema.is_some();
    let outcome = match &config.provider {
        Provider::Anthropic => send_anthropic(config, prompt, json_schema),
        Provider::OpenAiCompatible { json_mode, .. } => send_chat_completions(config, prompt, json_schema, *json_mode),
    };

    let (text, input_tokens, output_tokens) = match outcome {
        Ok(ok) => ok,
        Err((kind, err)) => {
            report(0, 0, Some(kind));
            return Err(err);
        }
    };

    if wants_json && serde_json::from_str::<serde_json::Value>(&text).is_err() {
        report(input_tokens, output_tokens, Some(LlmErrorKind::BadJson));
        anyhow::bail!("{} returned non-JSON text for a structured-output request: {text}", config.provider.name());
    }

    report(input_tokens, output_tokens, None);
    Ok(LlmCallResult { text, input_tokens, output_tokens, provider: config.provider.name().to_string(), model: config.model.clone(), latency_ms: started.elapsed().as_millis() as u64 })
}

type SendOutcome = std::result::Result<(String, u64, u64), (LlmErrorKind, anyhow::Error)>;

/// Maps a non-2xx status to its error kind + message — shared by both
/// providers so 401/429 read the same regardless of which one failed.
fn http_failure(provider: &str, key_hint: &str, status: u16, body: String) -> (LlmErrorKind, anyhow::Error) {
    match status {
        401 => (LlmErrorKind::Auth, anyhow::anyhow!("{provider} API rejected the key (401) — check {key_hint}")),
        429 => (LlmErrorKind::RateLimited, anyhow::anyhow!("{provider} API rate limit hit (429) — retry later")),
        _ => (LlmErrorKind::Other, anyhow::anyhow!("{provider} API returned {status}: {body}")),
    }
}

fn send_anthropic(config: &LlmConfig, prompt: &str, json_schema: Option<serde_json::Value>) -> SendOutcome {
    let output_config = json_schema.map(|schema| OutputConfig { format: OutputFormat::JsonSchema { schema } });
    let request = MessagesRequest { model: &config.model, max_tokens: config.max_tokens, messages: vec![MessageIn { role: "user", content: prompt }], output_config };

    let mut response = ureq::post(&config.api_url)
        .header("x-api-key", &config.api_key)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .config()
        .http_status_as_error(false)
        .build()
        .send_json(&request)
        .context("calling the Anthropic Messages API")
        .map_err(|e| (LlmErrorKind::Other, e))?;

    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        let body = response.body_mut().read_to_string().unwrap_or_default();
        return Err(http_failure("Anthropic", "AGENTOPS_ANTHROPIC_API_KEY", status, body));
    }

    let parsed: MessagesResponse = response.body_mut().read_json().context("parsing Anthropic API response").map_err(|e| (LlmErrorKind::Other, e))?;
    let text = parsed
        .content
        .into_iter()
        .find(|c| c.kind == "text")
        .and_then(|c| c.text)
        .ok_or_else(|| (LlmErrorKind::Other, anyhow::anyhow!("Anthropic response had no text content block")))?;

    Ok((text, parsed.usage.input_tokens, parsed.usage.output_tokens))
}

/// Builds an OpenAI-compatible `/chat/completions` body, with structured
/// output requested the way `json_mode` says this provider expects.
fn chat_completions_body(model: &str, max_tokens: u32, prompt: &str, json_schema: Option<serde_json::Value>, json_mode: JsonMode) -> serde_json::Value {
    let mut body = serde_json::json!({
        "model": model,
        "max_tokens": max_tokens,
        "messages": [{ "role": "user", "content": prompt }],
    });
    if let Some(schema) = json_schema {
        match json_mode {
            JsonMode::JsonSchemaStrict => {
                body["response_format"] = serde_json::json!({ "type": "json_schema", "json_schema": { "name": "response", "strict": true, "schema": schema } });
            }
            JsonMode::JsonSchema => {
                body["response_format"] = serde_json::json!({ "type": "json_schema", "json_schema": { "name": "response", "schema": schema } });
            }
            JsonMode::JsonObject => {
                body["response_format"] = serde_json::json!({ "type": "json_object" });
            }
            JsonMode::GuidedJson => {
                body["guided_json"] = schema;
            }
        }
    }
    body
}

#[derive(Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<ChatChoice>,
    usage: Option<ChatUsage>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: ChatMessage,
}

#[derive(Deserialize)]
struct ChatMessage {
    content: Option<String>,
}

#[derive(Deserialize)]
struct ChatUsage {
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
}

fn send_chat_completions(config: &LlmConfig, prompt: &str, json_schema: Option<serde_json::Value>, json_mode: JsonMode) -> SendOutcome {
    let provider = config.provider.name();
    let body = chat_completions_body(&config.model, config.max_tokens, prompt, json_schema, json_mode);

    let mut response = ureq::post(&config.api_url)
        .header("Authorization", &format!("Bearer {}", config.api_key))
        .config()
        .http_status_as_error(false)
        .build()
        .send_json(&body)
        .with_context(|| format!("calling {provider}'s chat completions API"))
        .map_err(|e| (LlmErrorKind::Other, e))?;

    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        let body = response.body_mut().read_to_string().unwrap_or_default();
        return Err(http_failure(provider, "this model's api_key_env", status, body));
    }

    let parsed: ChatCompletionResponse =
        response.body_mut().read_json().with_context(|| format!("parsing {provider}'s chat completions response")).map_err(|e| (LlmErrorKind::Other, e))?;
    let (input_tokens, output_tokens) = parsed.usage.map(|u| (u.prompt_tokens, u.completion_tokens)).unwrap_or((0, 0));
    let text = parsed
        .choices
        .into_iter()
        .next()
        .and_then(|c| c.message.content)
        .ok_or_else(|| (LlmErrorKind::Other, anyhow::anyhow!("{provider} response had no message content")))?;

    Ok((text, input_tokens, output_tokens))
}

/// Builds the prompt for explaining `symbol` — its full source, plus
/// light, cheap context: the symbol's file's `DependsOn` targets
/// (file-level, not a transitive closure), any `Gotcha`/`Decision` notes
/// already `Affects`-connected to it, and — since Initiative 4 of the
/// CLS-inspired retrieval plan — pattern-completed context recombined from
/// similar/graph-connected symbols elsewhere in the repo
/// (`agentops_retrieval::pattern_complete`). Does NOT include full file
/// content or sibling symbols' source directly — both cost and
/// prompt-injection surface scale with what's included here; `related`
/// only ever contributes another symbol's *notes*, never its raw source.
///
/// Token-budgeted against `MAX_PROMPT_INPUT_TOKENS`: `render_symbol_block`
/// is tried first at full size, then with `agentops_scanner::
/// compress_symbol_source` applied (only kept if it actually reduces the
/// token count — a short body with a long doc-comment sometimes doesn't),
/// then with `related_with_notes` entries dropped from the tail (they're
/// already similarity/PageRank-ordered by `pattern_complete`), and finally,
/// if still over budget, the symbol block itself is hard-truncated at the
/// token level. `existing_notes` are normally never reduced -- they're the
/// smallest category here and load-bearing for "don't contradict recorded
/// decisions" -- but this function still *guarantees* its return value
/// never exceeds `MAX_PROMPT_INPUT_TOKENS`: in the rare case where a
/// symbol has accumulated a pathologically large recorded note history
/// large enough that even an emptied-out symbol source doesn't bring the
/// total under budget, `existing_notes` are truncated too as a final
/// fallback rather than the budget being silently violated.
fn build_prompt(
    symbol: &Node,
    dep_paths: &[String],
    existing_notes: &[(NodeKind, String, String, NodeProminence, Option<String>)],
    related: &[agentops_retrieval::PatternCompletionMatch],
) -> String {
    let name = symbol.name.as_deref().unwrap_or("<unnamed>");
    let path = symbol.path.as_deref().unwrap_or("<unknown>");
    let full_source = symbol.content.as_deref().unwrap_or("");

    let dep_section = if dep_paths.is_empty() { String::new() } else { format!("\nThis file depends on: {}\n", dep_paths.join(", ")) };

    let mut notes_section = String::new();
    if !existing_notes.is_empty() {
        notes_section.push_str("\nKnown notes already recorded against this symbol (acknowledge these if relevant, don't contradict them):\n");
        for (kind, title, text, prominence, reason) in existing_notes {
            // A curated-down note is still shown (it may still be
            // relevant), just flagged so the model weighs it as lower
            // confidence and understands why, instead of treating every
            // recorded note as equally authoritative.
            let demoted = if *prominence == NodeProminence::Reduced {
                format!(" [lower confidence -- prominence reduced: {}]", reason.as_deref().unwrap_or("no reason recorded"))
            } else {
                String::new()
            };
            notes_section.push_str(&format!("- [{kind:?}] {title}: {text}{demoted}\n"));
        }
    }

    let related_with_notes: Vec<&agentops_retrieval::PatternCompletionMatch> = related.iter().filter(|m| !m.notes.is_empty()).collect();

    // Step 0: full source, all related context.
    let candidate = render_prompt(name, path, full_source, &dep_section, &notes_section, &related_with_notes);
    if count_tokens(&candidate) <= MAX_PROMPT_INPUT_TOKENS {
        return candidate;
    }

    // Step 1: compress the symbol's source (signature kept, body elided),
    // but only if that actually reduces its token count -- a short body
    // with a long doc-comment sometimes doesn't compress smaller.
    let compressed_source = symbol
        .path
        .as_deref()
        .and_then(|p| Path::new(p).extension())
        .and_then(|ext| ext.to_str())
        .and_then(agentops_scanner::Language::from_extension)
        .map(|lang| agentops_scanner::compress_symbol_source(full_source, lang))
        .filter(|compressed| count_tokens(compressed) < count_tokens(full_source));
    let source = compressed_source.as_deref().unwrap_or(full_source);
    let candidate = render_prompt(name, path, source, &dep_section, &notes_section, &related_with_notes);
    if count_tokens(&candidate) <= MAX_PROMPT_INPUT_TOKENS {
        return candidate;
    }

    // Step 2: drop related-context entries from the tail (already
    // similarity/PageRank-ordered by `pattern_complete`) one at a time.
    for keep in (0..related_with_notes.len()).rev() {
        let candidate = render_prompt(name, path, source, &dep_section, &notes_section, &related_with_notes[..keep]);
        if count_tokens(&candidate) <= MAX_PROMPT_INPUT_TOKENS {
            return candidate;
        }
    }

    // Step 3: every reduction so far exhausted and still over budget --
    // hard-truncate the symbol block itself at the token level as a last
    // resort, keeping everything else (`existing_notes` are never reduced,
    // see the doc comment above; there's no related context left to drop).
    const TRUNCATION_MARKER: &str = "\n... [truncated]";
    // A small fixed safety margin: BPE merges can span the join point
    // between `truncated_source` and the surrounding template text, so the
    // final re-tokenized count isn't guaranteed to equal the sum of its
    // parts' counts exactly.
    const RETOKENIZATION_SAFETY_MARGIN: usize = 64;
    let non_symbol_tokens = count_tokens(&render_prompt(name, path, "", &dep_section, &notes_section, &[])) + count_tokens(TRUNCATION_MARKER) + RETOKENIZATION_SAFETY_MARGIN;
    let budget_for_source = MAX_PROMPT_INPUT_TOKENS.saturating_sub(non_symbol_tokens);
    let truncated_source = truncate_to_tokens(source, budget_for_source);
    let candidate = render_prompt(name, path, &format!("{truncated_source}{TRUNCATION_MARKER}"), &dep_section, &notes_section, &[]);
    if count_tokens(&candidate) <= MAX_PROMPT_INPUT_TOKENS {
        return candidate;
    }

    // Step 4: `existing_notes` alone (never reduced by steps 0-3, see this
    // function's doc comment) are large enough that even an emptied-out
    // symbol source still leaves the prompt over budget -- truncate
    // `notes_section` too, as the true last resort. This is what keeps the
    // "never reduced" claim in the doc comment from being a silent lie: it
    // holds for every symbol except the rare one with a pathologically
    // large recorded note history, and even then the function still
    // guarantees its own contract (staying under `MAX_PROMPT_INPUT_TOKENS`)
    // rather than silently violating it.
    let skeleton_tokens = count_tokens(&render_prompt(name, path, "", &dep_section, "", &[])) + count_tokens(TRUNCATION_MARKER) + RETOKENIZATION_SAFETY_MARGIN;
    let budget_for_notes = MAX_PROMPT_INPUT_TOKENS.saturating_sub(skeleton_tokens);
    let truncated_notes = format!("{}{TRUNCATION_MARKER}", truncate_to_tokens(&notes_section, budget_for_notes));
    render_prompt(name, path, &format!("{truncated_source}{TRUNCATION_MARKER}"), &dep_section, &truncated_notes, &[])
}

fn render_prompt(name: &str, path: &str, source: &str, dep_section: &str, notes_section: &str, related_with_notes: &[&agentops_retrieval::PatternCompletionMatch]) -> String {
    let mut prompt = format!(
        "You are documenting a codebase. Explain concisely what this symbol does and why it might exist, for a developer or AI agent reading the code for the first time.\n\n\
         Symbol: {name}\nFile: {path}\n\n```\n{source}\n```\n"
    );

    prompt.push_str(dep_section);
    prompt.push_str(notes_section);

    if !related_with_notes.is_empty() {
        // Only symbols that actually carry their own notes are worth
        // spending prompt budget on -- a pattern-completed symbol with no
        // recorded knowledge contributes nothing an explanation can use.
        prompt.push_str("\nPossibly related context from similar symbols elsewhere in this repo (may or may not be directly relevant -- weigh accordingly, don't assume it applies here):\n");
        for m in related_with_notes {
            let via = match m.via {
                agentops_retrieval::PatternCompletionSource::Similar(s) => format!("similar, {s:.2} cosine similarity"),
                agentops_retrieval::PatternCompletionSource::Graph(s) => format!("graph-connected, {s:.4} PageRank mass"),
            };
            prompt.push_str(&format!("- {} ({via}):\n", m.node.name.as_deref().unwrap_or("<unnamed>")));
            for (_id, kind, title, text, _, _) in &m.notes {
                prompt.push_str(&format!("  - [{kind:?}] {title}: {text}\n"));
            }
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

    let existing_notes: Vec<(NodeKind, String, String, NodeProminence, Option<String>)> = store
        .edges_to(repo, symbol_id)?
        .into_iter()
        .filter(|e| e.relation == EdgeRelation::Affects)
        .filter_map(|e| store.get_node(repo, e.src_id).ok().flatten())
        .filter(|n| matches!(n.kind, NodeKind::Gotcha | NodeKind::Decision))
        .map(|n| (n.kind, n.name.clone().unwrap_or_default(), n.content.clone().unwrap_or_default(), n.prominence, n.curation_reason.clone()))
        .collect();

    // Pattern-completed context (Initiative 4, CLS-inspired retrieval
    // plan): notes from similar/graph-connected symbols elsewhere in the
    // repo, on top of `existing_notes`' seed-symbol-only view.
    let related = agentops_retrieval::pattern_complete(store, &agentops_embeddings::LocalEmbedder, repo, symbol_id, PATTERN_COMPLETE_K)?;

    let prompt = build_prompt(&symbol, &dep_paths, &existing_notes, &related);
    let result = call_anthropic(config, "explain_symbol", &prompt)?;

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
        let result = call_anthropic(self.config, "match_symbols", &prompt)?;
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
        let result = call_anthropic(self.config, "classify_note", &prompt)?;
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

/// Three audience-tuned work summaries for one task's activity — Phase 6b
/// Extension 2. Delimiter-tagged single-call response (`TECHNICAL:` /
/// `NON_TECHNICAL:` / `CLIENT_FRIENDLY:`), parsed back out below — one
/// Anthropic call is cheaper than three, and this delimiter approach is
/// simple enough to not need a second call as a fallback; revisit if real
/// output ever proves unreliable to parse.
#[derive(Debug, Clone, Default)]
pub struct TaskSummaries {
    pub technical: String,
    pub non_technical: String,
    pub client_friendly: String,
}

const TECHNICAL_TAG: &str = "TECHNICAL:";
const NON_TECHNICAL_TAG: &str = "NON_TECHNICAL:";
const CLIENT_FRIENDLY_TAG: &str = "CLIENT_FRIENDLY:";

/// Renders `activity` using the exact same format `tool_get_task_activity`
/// (`agentops-mcp/src/tools.rs`) already uses for a human/agent-facing
/// activity feed — deliberately not a second, silently-divergent copy of
/// that rendering. Each line is passed through `compress_for_prompt` first,
/// since `description` can carry verbatim tool output (test/build logs).
fn render_activity(activity: &[agentops_graph::SessionEvent]) -> String {
    activity.iter().map(|e| format!("- [{}] {}: {}", e.created_at, e.tool_name, compress_for_prompt(&e.description))).collect::<Vec<_>>().join("\n")
}

/// Longest a real CSI escape sequence's parameter bytes get in practice
/// (e.g. 24-bit color codes) — bounds the terminator search below so a
/// genuinely unterminated `ESC [` (no letter anywhere in the rest of the
/// string) can't consume the rest of the text looking for one.
const MAX_CSI_PARAM_LEN: usize = 32;

/// Lossless prompt-content hygiene for text that may carry raw tool output:
/// strips ANSI CSI escape codes and collapses 3+ consecutive identical lines
/// into one with a repeat count. Deliberately narrow — no filler-word
/// removal or semantic rewriting, which would risk changing meaning in a
/// summary that's read for accuracy.
fn compress_for_prompt(text: &str) -> String {
    let mut no_ansi = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\u{1b}' && chars.get(i + 1) == Some(&'[') {
            let window_end = (i + 2 + MAX_CSI_PARAM_LEN).min(chars.len());
            match chars[i + 2..window_end].iter().position(|c| c.is_ascii_alphabetic()) {
                // A real CSI terminator is any letter (per the ANSI spec, not
                // just 'm') — skip the whole sequence including it.
                Some(offset) => i += 2 + offset + 1,
                // No terminator within the bounded window: not a real escape
                // sequence (or one this function doesn't need to handle) —
                // keep the `ESC` byte itself literal and re-examine the rest
                // character by character, rather than silently consuming it.
                None => {
                    no_ansi.push(chars[i]);
                    i += 1;
                }
            }
        } else {
            no_ansi.push(chars[i]);
            i += 1;
        }
    }

    // Collapse runs of 3+ identical consecutive lines into one + a count.
    let mut out: Vec<String> = Vec::new();
    let lines: Vec<&str> = no_ansi.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let mut j = i + 1;
        while j < lines.len() && lines[j] == lines[i] {
            j += 1;
        }
        let run_len = j - i;
        if run_len >= 3 {
            out.push(format!("{} (repeated {run_len} times)", lines[i]));
        } else {
            out.extend(lines[i..j].iter().map(|s| s.to_string()));
        }
        i = j;
    }
    out.join("\n")
}

/// Generates technical/non-technical/client-friendly summaries of a task's
/// recorded activity (`SessionEvent`s correlated under its `session_id`),
/// for pushing back to Linear as comments and/or printing to stdout.
pub fn summarize_task_activity(config: &AnthropicConfig, task_title: &str, activity: &[agentops_graph::SessionEvent]) -> Result<TaskSummaries> {
    if activity.is_empty() {
        anyhow::bail!("no activity recorded for task {task_title:?} — nothing to summarize");
    }

    let prompt = format!(
        "You are summarizing work done on a task, for three different audiences, based on a raw activity log.\n\n\
         Task: {task_title}\n\nActivity log:\n{}\n\n\
         Write three summaries, each on its own line(s), in exactly this format (including the tags, each on its own line):\n\n\
         {TECHNICAL_TAG}\n<a precise, technical summary for another engineer>\n\n\
         {NON_TECHNICAL_TAG}\n<a summary for a non-technical teammate — what changed and why, no jargon>\n\n\
         {CLIENT_FRIENDLY_TAG}\n<a short summary suitable for sharing with an external client — outcome-focused, no internal implementation detail>\n\n\
         Respond with only those three tagged sections, no preamble.",
        render_activity(activity)
    );

    let result = call_anthropic(config, "summarize_task", &prompt)?;
    parse_task_summaries(&result.text)
}

/// Groups `ranked_paths` into a small number of named, developer-recognizable
/// modules (e.g. "Authentication", "Indexing Engine") for the Documentation
/// Viewer's Core Modules nav section — the labeled counterpart to
/// `agentops-docgen::sectioned`'s directory-name heuristic fallback, which
/// callers should use if this errors (network failure, malformed response,
/// etc.) rather than propagating the failure into doc generation. Uses
/// `output_config: json_schema` rather than the delimiter-tagged parsing
/// `summarize_task_activity` uses, since the whole point of this call is
/// structured `{label, file_paths[]}` data, not prose.
pub fn group_core_modules(config: &AnthropicConfig, repo: &str, ranked_paths: &[PathBuf]) -> Result<Vec<ModuleLabel>> {
    if ranked_paths.is_empty() {
        return Ok(Vec::new());
    }

    let file_list = ranked_paths.iter().map(|p| p.to_string_lossy().into_owned()).collect::<Vec<_>>().join("\n");
    let prompt = format!(
        "You are documenting the architecture of a codebase named {repo}. Below is its file list, ranked by centrality \
         in the dependency graph (most-referenced first).\n\nFiles:\n{file_list}\n\n\
         Group these files into 3-8 named modules a developer or AI agent onboarding to this repo would recognize \
         (e.g. \"Authentication\", \"Indexing Engine\") — meaningful architectural names, not raw directory names. \
         Every file should belong to at most one module; omit files that don't fit a coherent module rather than \
         forcing them into one."
    );

    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "modules": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "label": { "type": "string" },
                        "file_paths": { "type": "array", "items": { "type": "string" } }
                    },
                    "required": ["label", "file_paths"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["modules"],
        "additionalProperties": false
    });

    let result = call_anthropic_json(config, "group_modules", &prompt, schema)?;
    parse_module_groups(&result.text)
}

#[derive(Deserialize)]
struct ModuleGroups {
    modules: Vec<ModuleLabel>,
}

fn parse_module_groups(text: &str) -> Result<Vec<ModuleLabel>> {
    let parsed: ModuleGroups = serde_json::from_str(text).with_context(|| format!("parsing Anthropic module-grouping JSON response: {text}"))?;
    Ok(parsed.modules)
}

fn parse_task_summaries(text: &str) -> Result<TaskSummaries> {
    let extract = |tag: &str, next_tags: &[&str]| -> Option<String> {
        let start = text.find(tag)? + tag.len();
        let rest = &text[start..];
        let end = next_tags.iter().filter_map(|t| rest.find(t)).min().unwrap_or(rest.len());
        Some(rest[..end].trim().to_string())
    };

    let technical = extract(TECHNICAL_TAG, &[NON_TECHNICAL_TAG, CLIENT_FRIENDLY_TAG]).ok_or_else(|| anyhow::anyhow!("Anthropic response missing {TECHNICAL_TAG} section: {text}"))?;
    let non_technical = extract(NON_TECHNICAL_TAG, &[CLIENT_FRIENDLY_TAG]).ok_or_else(|| anyhow::anyhow!("Anthropic response missing {NON_TECHNICAL_TAG} section: {text}"))?;
    let client_friendly = extract(CLIENT_FRIENDLY_TAG, &[]).ok_or_else(|| anyhow::anyhow!("Anthropic response missing {CLIENT_FRIENDLY_TAG} section: {text}"))?;

    Ok(TaskSummaries { technical, non_technical, client_friendly })
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
        AnthropicConfig { api_key: "test-key".into(), model: "claude-sonnet-5".into(), max_tokens: 1024, api_url: format!("{base_url}/v1/messages"), ..Default::default() }
    }

    // Deliberately no test exercises `from_env`/`from_env_cheap`'s env-var
    // reading directly (e.g. via `std::env::set_var`) — per
    // `.agentops/notes/env-set-var-requires-unsafe-block-in-current-rust.md`,
    // mutating process env is only safe single-threaded/pre-runtime, and
    // `cargo test` runs this file's tests in parallel by default. Tier
    // selection is covered instead via `compress_for_prompt` (pure) below
    // and by constructing `AnthropicConfig` directly, matching `mock_config`.

    #[test]
    fn compress_for_prompt_strips_ansi_codes() {
        let input = "\u{1b}[32mok\u{1b}[0m: build passed";
        assert_eq!(compress_for_prompt(input), "ok: build passed");
    }

    #[test]
    fn compress_for_prompt_collapses_long_repeated_lines() {
        let input = "start\nnoisy line\nnoisy line\nnoisy line\nnoisy line\nend";
        assert_eq!(compress_for_prompt(input), "start\nnoisy line (repeated 4 times)\nend");
    }

    #[test]
    fn compress_for_prompt_leaves_short_runs_and_unique_lines_untouched() {
        let input = "a\nb\nb\nc";
        assert_eq!(compress_for_prompt(input), "a\nb\nb\nc");
    }

    #[test]
    fn compress_for_prompt_preserves_content_after_a_truly_unterminated_escape() {
        // No alphabetic byte anywhere in the rest of the string -- the old
        // implementation's `.find()` consumed the whole remaining iterator
        // looking for one, silently dropping everything after it. Confirmed
        // via a wrap-skill council audit (2026-09-28): fixed by bounding the
        // terminator search instead of scanning unboundedly.
        let input = "before\u{1b}[315;999";
        assert_eq!(compress_for_prompt(input), "before\u{1b}[315;999");
    }

    #[test]
    fn compress_for_prompt_treats_any_letter_as_a_valid_csi_terminator() {
        // Per the ANSI spec any letter ends a CSI sequence, not just 'm' --
        // "\x1b[31T" is a real (if unusual) sequence (Scroll Left), so this
        // is correct stripping, not data loss, despite looking similar to
        // the case above at a glance.
        let input = "\u{1b}[31Text";
        assert_eq!(compress_for_prompt(input), "ext");
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
            curated: false,
            prominence: NodeProminence::Full,
            curation_reason: None,
            last_touched_at: None,
        };
        let prompt = build_prompt(&node, &["src/config.rs".to_string()], &[(NodeKind::Gotcha, "off-by-one".into(), "expiry bug".into(), NodeProminence::Full, None)], &[]);
        assert!(prompt.contains("verify_token"));
        assert!(prompt.contains("fn verify_token() {}"));
        assert!(prompt.contains("src/config.rs"));
        assert!(prompt.contains("off-by-one"));
        assert!(prompt.contains("expiry bug"));
    }

    #[test]
    fn build_prompt_flags_a_reduced_prominence_note_as_lower_confidence() {
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
            curated: false,
            prominence: NodeProminence::Full,
            curation_reason: None,
            last_touched_at: None,
        };
        let prompt = build_prompt(&node, &[], &[(NodeKind::Gotcha, "niche issue".into(), "rare edge case".into(), NodeProminence::Reduced, Some("only affects old Linux envs".into()))], &[]);
        assert!(prompt.contains("lower confidence"), "{prompt}");
        assert!(prompt.contains("only affects old Linux envs"), "{prompt}");
    }

    /// A synthetic Rust function whose body alone is `n` statements long --
    /// large enough to blow `MAX_PROMPT_INPUT_TOKENS` on its own once
    /// wrapped in a real function signature, standing in for a real
    /// oversized symbol (e.g. `agentops-mcp/src/scan.rs`'s 881-line
    /// `persist`) without needing to check one into this crate's fixtures.
    fn oversized_rust_source(n: usize) -> String {
        let mut body = String::new();
        for i in 0..n {
            body.push_str(&format!("    let x{i} = compute_something_moderately_named(i, {i});\n"));
        }
        format!("pub fn oversized_symbol(i: usize) -> usize {{\n{body}    x0\n}}\n")
    }

    #[test]
    fn build_prompt_compresses_an_oversized_symbol_under_budget_instead_of_truncating() {
        let source = oversized_rust_source(4000);
        assert!(count_tokens(&source) > MAX_PROMPT_INPUT_TOKENS, "fixture must actually be oversized to exercise the reduction ladder");

        let node = Node {
            id: 1,
            kind: NodeKind::Symbol,
            repo: "demo".into(),
            path: Some("src/big.rs".into()),
            name: Some("oversized_symbol".into()),
            container: None,
            start_line: Some(1),
            end_line: Some(4002),
            content: Some(source),
            curated: false,
            prominence: NodeProminence::Full,
            curation_reason: None,
            last_touched_at: None,
        };
        let prompt = build_prompt(&node, &[], &[], &[]);

        assert!(count_tokens(&prompt) <= MAX_PROMPT_INPUT_TOKENS, "prompt should fit under budget: {} tokens", count_tokens(&prompt));
        assert!(prompt.contains("pub fn oversized_symbol(i: usize) -> usize {"), "signature should survive compression: {prompt}");
        assert!(!prompt.contains("compute_something_moderately_named"), "body should have been elided by compression, not truncation: {prompt}");
        assert!(!prompt.contains("[truncated]"), "a realistic oversized symbol should be handled by compression, not need the truncation fallback: {prompt}");
    }

    /// A synthetic Rust struct with `n` fields -- `compress_symbol_source`
    /// deliberately never touches struct/class/trait bodies (their fields
    /// are the useful payload, not implementation detail -- see
    /// `compress.rs::leaves_struct_definitions_unchanged`), so a struct this
    /// large stays exactly this large through the whole reduction ladder
    /// and forces the truncation fallback to actually be exercised.
    fn oversized_rust_struct_source(n: usize) -> String {
        let mut fields = String::new();
        for i in 0..n {
            fields.push_str(&format!("    pub field_with_a_moderately_long_name_{i}: usize,\n"));
        }
        format!("pub struct OversizedStruct {{\n{fields}}}\n")
    }

    #[test]
    fn build_prompt_falls_back_to_truncation_when_even_compression_is_not_enough() {
        // A struct, not a function: compression never touches struct
        // bodies (see `oversized_rust_struct_source`'s doc comment above),
        // so this is the fixture that actually forces the truncation
        // fallback rather than being fully absorbed by compression.
        let source = oversized_rust_struct_source(60_000);
        assert!(count_tokens(&source) > MAX_PROMPT_INPUT_TOKENS * 5, "fixture must be far larger than budget to force the truncation fallback");

        let node = Node {
            id: 1,
            kind: NodeKind::Symbol,
            repo: "demo".into(),
            path: Some("src/huge.rs".into()),
            name: Some("oversized_symbol".into()),
            container: None,
            start_line: Some(1),
            end_line: Some(400_002),
            content: Some(source),
            curated: false,
            prominence: NodeProminence::Full,
            curation_reason: None,
            last_touched_at: None,
        };
        let prompt = build_prompt(&node, &[], &[], &[]);

        assert!(count_tokens(&prompt) <= MAX_PROMPT_INPUT_TOKENS, "prompt should still fit under budget: {} tokens", count_tokens(&prompt));
        assert!(prompt.contains("[truncated]"), "an unrealistically giant symbol should hit the truncation fallback: {prompt}");
    }

    #[test]
    fn build_prompt_drops_related_context_from_the_tail_before_truncating_source() {
        let source = oversized_rust_source(4000);
        let node = Node {
            id: 1,
            kind: NodeKind::Symbol,
            repo: "demo".into(),
            path: Some("src/big.rs".into()),
            name: Some("oversized_symbol".into()),
            container: None,
            start_line: Some(1),
            end_line: Some(4002),
            content: Some(source),
            curated: false,
            prominence: NodeProminence::Full,
            curation_reason: None,
            last_touched_at: None,
        };

        let make_match = |name: &str| agentops_retrieval::PatternCompletionMatch {
            node: Node {
                id: 2,
                kind: NodeKind::Symbol,
                repo: "demo".into(),
                path: Some("src/other.rs".into()),
                name: Some(name.to_string()),
                container: None,
                start_line: Some(1),
                end_line: Some(1),
                content: Some("fn other() {}".into()),
                curated: false,
                prominence: NodeProminence::Full,
                curation_reason: None,
                last_touched_at: None,
            },
            via: agentops_retrieval::PatternCompletionSource::Similar(0.9),
            notes: vec![(3, NodeKind::Gotcha, format!("note about {name}"), "some recorded knowledge here".into(), NodeProminence::Full, None)],
        };
        let related = vec![make_match("sibling_a"), make_match("sibling_b")];

        let prompt = build_prompt(&node, &[], &[], &related);
        assert!(count_tokens(&prompt) <= MAX_PROMPT_INPUT_TOKENS, "prompt should fit under budget: {} tokens", count_tokens(&prompt));
    }

    #[test]
    fn build_prompt_truncates_existing_notes_when_they_alone_exceed_budget() {
        // A small, ordinary symbol -- the point is that `existing_notes`
        // alone, not the symbol source, is what's oversized here. Before
        // Step 4 existed, this returned a prompt over MAX_PROMPT_INPUT_TOKENS
        // with no signal, since `existing_notes` were never reduced.
        let node = Node {
            id: 1,
            kind: NodeKind::Symbol,
            repo: "demo".into(),
            path: Some("src/small.rs".into()),
            name: Some("small_symbol".into()),
            container: None,
            start_line: Some(1),
            end_line: Some(3),
            content: Some("pub fn small_symbol() -> i32 {\n    42\n}\n".into()),
            curated: false,
            prominence: NodeProminence::Full,
            curation_reason: None,
            last_touched_at: None,
        };

        let huge_note_text = "this note describes a very subtle interaction between two systems in exhaustive detail ".repeat(6000);
        let existing_notes = vec![(NodeKind::Decision, "a sprawling decision record".to_string(), huge_note_text, NodeProminence::Full, None)];
        assert!(count_tokens(&existing_notes[0].2) > MAX_PROMPT_INPUT_TOKENS, "fixture note must itself be oversized to exercise Step 4");

        let prompt = build_prompt(&node, &[], &existing_notes, &[]);

        assert!(count_tokens(&prompt) <= MAX_PROMPT_INPUT_TOKENS, "prompt must never exceed budget even when existing_notes alone are huge: {} tokens", count_tokens(&prompt));
        assert!(prompt.contains("small_symbol"), "the symbol itself should still be identifiable: {prompt}");
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

        let result = call_anthropic(&mock_config(&server.uri()), "test", "explain this").unwrap();
        assert_eq!(result.text, "This function verifies a token.");
        assert_eq!(result.input_tokens, 42);
        assert_eq!(result.output_tokens, 7);
    }

    #[tokio::test]
    async fn call_anthropic_maps_401_to_a_clear_key_error() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST")).respond_with(wiremock::ResponseTemplate::new(401)).mount(&server).await;

        let err = call_anthropic(&mock_config(&server.uri()), "test", "x").unwrap_err();
        assert!(err.to_string().contains("401"));
        assert!(err.to_string().contains("AGENTOPS_ANTHROPIC_API_KEY"));
    }

    #[tokio::test]
    async fn call_anthropic_maps_429_to_a_distinct_rate_limit_error() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST")).respond_with(wiremock::ResponseTemplate::new(429)).mount(&server).await;

        let err = call_anthropic(&mock_config(&server.uri()), "test", "x").unwrap_err();
        assert!(err.to_string().contains("429"));
        assert!(err.to_string().to_lowercase().contains("rate limit"));
    }

    #[tokio::test]
    async fn a_successful_call_reports_exactly_one_spend_record() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "content": [{"type": "text", "text": "ok"}],
                "usage": {"input_tokens": 42, "output_tokens": 7}
            })))
            .mount(&server)
            .await;

        let recorder = UsageRecorder::default();
        let config = mock_config(&server.uri()).with_usage_sink(recorder.clone());
        call_anthropic(&config, "explain_symbol", "x").unwrap();

        let records = recorder.records();
        assert_eq!(records.len(), 1, "{records:?}");
        let rec = &records[0];
        assert_eq!((rec.operation.as_str(), rec.provider.as_str(), rec.model.as_str()), ("explain_symbol", "anthropic", "claude-sonnet-5"));
        assert_eq!((rec.input_tokens, rec.output_tokens), (42, 7));
        assert!(rec.success && rec.error_kind.is_none());
    }

    #[tokio::test]
    async fn a_failed_call_is_recorded_before_its_error_reaches_the_caller() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST")).respond_with(wiremock::ResponseTemplate::new(429)).mount(&server).await;

        let recorder = UsageRecorder::default();
        let config = mock_config(&server.uri()).with_usage_sink(recorder.clone());
        let err = call_anthropic(&config, "classify_note", "x").unwrap_err();
        assert!(err.to_string().contains("429"));

        let records = recorder.records();
        assert_eq!(records.len(), 1, "{records:?}");
        assert!(!records[0].success);
        assert_eq!(records[0].error_kind, Some(LlmErrorKind::RateLimited));
        assert_eq!((records[0].input_tokens, records[0].output_tokens), (0, 0));
    }

    #[tokio::test]
    async fn a_structured_request_answered_with_non_json_is_recorded_as_bad_json() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "content": [{"type": "text", "text": "Sure! Here are the modules:"}],
                "usage": {"input_tokens": 30, "output_tokens": 6}
            })))
            .mount(&server)
            .await;

        let recorder = UsageRecorder::default();
        let config = mock_config(&server.uri()).with_usage_sink(recorder.clone());
        assert!(call_json(&config, "group_modules", "x", serde_json::json!({"type": "object"})).is_err());

        let records = recorder.records();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].error_kind, Some(LlmErrorKind::BadJson));
        assert_eq!((records[0].input_tokens, records[0].output_tokens), (30, 6), "a bad-JSON response still spent its tokens");
    }

    #[test]
    fn recorded_spend_persists_to_the_store_priced_by_model() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let recorder = UsageRecorder::default();
        let base = LlmCallRecord { operation: "explain_symbol".into(), provider: "anthropic".into(), model: "claude-opus-5-5".into(), input_tokens: 1_000_000, output_tokens: 0, latency_ms: 5, success: true, error_kind: None };
        recorder.record(&base);
        recorder.record(&LlmCallRecord { provider: "groq".into(), model: "llama-3.3-70b-versatile".into(), success: false, error_kind: Some(LlmErrorKind::RateLimited), ..base.clone() });

        assert_eq!(recorder.persist(&store, "demo"), 2);
        assert!(recorder.records().is_empty(), "persist drains the buffer so a second persist can't double-count");

        let rows = store.llm_usage_for_repo("demo").unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].cost_estimate_usd, Some(4.0));
        assert_eq!(rows[1].cost_estimate_usd, None, "an unpriced provider stays NULL, never $0");
        assert_eq!(rows[1].error_kind.as_deref(), Some("rate_limited"));
        assert!(store.llm_usage_for_repo("other").unwrap().is_empty());
    }

    fn openai_config(base_url: &str, json_mode: JsonMode) -> LlmConfig {
        LlmConfig {
            api_key: "test-key".into(),
            model: "llama-3.3-70b-versatile".into(),
            api_url: format!("{base_url}/openai/v1/chat/completions"),
            provider: Provider::OpenAiCompatible { name: "groq".into(), json_mode },
            ..Default::default()
        }
    }

    #[test]
    fn each_json_mode_builds_its_providers_own_request_shape() {
        let schema = serde_json::json!({"type": "object", "properties": {"a": {"type": "string"}}, "required": ["a"], "additionalProperties": false});
        let body = |mode| chat_completions_body("m", 100, "p", Some(schema.clone()), mode);

        let strict = body(JsonMode::JsonSchemaStrict);
        assert_eq!(strict["response_format"]["type"], "json_schema");
        assert_eq!(strict["response_format"]["json_schema"]["strict"], true);
        assert_eq!(strict["response_format"]["json_schema"]["schema"], schema);

        let plain = body(JsonMode::JsonSchema);
        assert_eq!(plain["response_format"]["type"], "json_schema");
        assert!(plain["response_format"]["json_schema"].get("strict").is_none());

        let object = body(JsonMode::JsonObject);
        assert_eq!(object["response_format"], serde_json::json!({"type": "json_object"}));

        let guided = body(JsonMode::GuidedJson);
        assert_eq!(guided["guided_json"], schema, "NVIDIA NIM takes the schema as a top-level guided_json field");
        assert!(guided.get("response_format").is_none());

        let unstructured = chat_completions_body("m", 100, "p", None, JsonMode::JsonSchemaStrict);
        assert!(unstructured.get("response_format").is_none() && unstructured.get("guided_json").is_none());
    }

    #[tokio::test]
    async fn an_openai_compatible_call_reads_text_and_usage_and_reports_its_provider_name() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/openai/v1/chat/completions"))
            .and(wiremock::matchers::header("authorization", "Bearer test-key"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{"message": {"role": "assistant", "content": "{\"a\": \"b\"}"}}],
                "usage": {"prompt_tokens": 55, "completion_tokens": 9, "total_tokens": 64}
            })))
            .mount(&server)
            .await;

        let recorder = UsageRecorder::default();
        let config = openai_config(&server.uri(), JsonMode::JsonSchemaStrict).with_usage_sink(recorder.clone());
        let result = call_json(&config, "librarian_classify", "x", serde_json::json!({"type": "object"})).unwrap();
        assert_eq!(result.text, "{\"a\": \"b\"}");
        assert_eq!((result.input_tokens, result.output_tokens), (55, 9));

        let records = recorder.records();
        assert_eq!(records.len(), 1);
        assert_eq!((records[0].provider.as_str(), records[0].operation.as_str()), ("groq", "librarian_classify"));
    }

    #[tokio::test]
    async fn an_openai_compatible_401_maps_to_an_auth_failure() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST")).respond_with(wiremock::ResponseTemplate::new(401)).mount(&server).await;

        let recorder = UsageRecorder::default();
        let config = openai_config(&server.uri(), JsonMode::JsonObject).with_usage_sink(recorder.clone());
        let err = call_anthropic(&config, "librarian_classify", "x").unwrap_err();
        assert!(err.to_string().contains("groq") && err.to_string().contains("401"), "{err}");
        assert_eq!(recorder.records()[0].error_kind, Some(LlmErrorKind::Auth));
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

    /// Initiative 4: `explain_symbol`'s prompt must actually include
    /// pattern-completed context from a graph-connected symbol that carries
    /// its own notes -- asserted by only satisfying the mock when the sent
    /// request body contains that recombined content, so the test fails
    /// (mock returns an unmatched-request error) if the wiring is broken,
    /// not just checked after the fact against a fixed mock response.
    #[tokio::test]
    async fn explain_symbol_includes_pattern_completed_context_from_a_connected_symbol() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::body_string_contains("watch out"))
            .and(wiremock::matchers::body_string_contains("edge case in the connected helper"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "content": [{"type": "text", "text": "Verifies a bearer token, related to a known edge case elsewhere."}],
                "usage": {"input_tokens": 10, "output_tokens": 5}
            })))
            .mount(&server)
            .await;

        let store = SqliteGraphStore::open_in_memory().unwrap();
        let symbol_id = symbol_node(&store, "demo", "src/auth.rs", "verify_token", "fn verify_token() {}");
        let connected_id = symbol_node(&store, "demo", "src/helpers.rs", "connected_helper", "fn connected_helper() {}");
        store.add_edge("demo", symbol_id, connected_id, EdgeRelation::References).unwrap();
        let gotcha_id = upsert_node(&store, NewNode { kind: NodeKind::Gotcha, repo: "demo".into(), path: None, name: Some("watch out".into()), container: None, start_line: None, end_line: None, content: Some("edge case in the connected helper".into()) })
            .unwrap();
        store.add_edge("demo", gotcha_id, connected_id, EdgeRelation::Affects).unwrap();
        let config = mock_config(&server.uri());

        let definition_id = explain_symbol(&store, &config, "demo", symbol_id).unwrap();
        let definition = store.get_node("demo", definition_id).unwrap().unwrap();
        assert_eq!(definition.content.as_deref(), Some("Verifies a bearer token, related to a known edge case elsewhere."));
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
        let config = AnthropicConfig { api_key: "unused".into(), model: "claude-sonnet-5".into(), max_tokens: 1024, api_url: "http://127.0.0.1:1/v1/messages".into(), ..Default::default() };
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

    fn event(created_at: &str, tool_name: &str, description: &str) -> agentops_graph::SessionEvent {
        agentops_graph::SessionEvent { id: 0, repo: "demo".into(), session_id: "s1".into(), tool_name: tool_name.into(), description: description.into(), node_id: None, event_kind: "activity".into(), created_at: created_at.into() }
    }

    #[test]
    fn parse_task_summaries_splits_all_three_tagged_sections() {
        let text = "TECHNICAL:\nRewrote the auth middleware to use JWT.\n\nNON_TECHNICAL:\nMade logins faster and more secure.\n\nCLIENT_FRIENDLY:\nLogin is now faster.";
        let summaries = parse_task_summaries(text).unwrap();
        assert_eq!(summaries.technical, "Rewrote the auth middleware to use JWT.");
        assert_eq!(summaries.non_technical, "Made logins faster and more secure.");
        assert_eq!(summaries.client_friendly, "Login is now faster.");
    }

    #[test]
    fn parse_task_summaries_errors_clearly_when_a_section_is_missing() {
        let text = "TECHNICAL:\nsomething\n\nCLIENT_FRIENDLY:\nsomething else";
        let err = parse_task_summaries(text).unwrap_err();
        assert!(err.to_string().contains("NON_TECHNICAL"));
    }

    #[test]
    fn summarize_task_activity_refuses_an_empty_activity_log() {
        let config = AnthropicConfig { api_key: "unused".into(), model: "claude-sonnet-5".into(), max_tokens: 1024, api_url: "http://127.0.0.1:1/v1/messages".into(), ..Default::default() };
        let err = summarize_task_activity(&config, "Fix login bug", &[]).unwrap_err();
        assert!(err.to_string().contains("nothing to summarize"));
    }

    #[tokio::test]
    async fn summarize_task_activity_parses_a_real_shaped_response_and_includes_rendered_activity_in_the_prompt() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::body_string_contains("scan_repo"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "content": [{"type": "text", "text": "TECHNICAL:\nRan scan_repo and upserted 3 symbols.\n\nNON_TECHNICAL:\nScanned the codebase for changes.\n\nCLIENT_FRIENDLY:\nProgress made on the requested feature."}],
                "usage": {"input_tokens": 20, "output_tokens": 10}
            })))
            .mount(&server)
            .await;

        let config = mock_config(&server.uri());
        let activity = vec![event("2026-08-11T00:00:00Z", "scan_repo", "scanned 3 files")];

        let summaries = summarize_task_activity(&config, "Fix login bug", &activity).unwrap();
        assert_eq!(summaries.technical, "Ran scan_repo and upserted 3 symbols.");
        assert_eq!(summaries.client_friendly, "Progress made on the requested feature.");
    }

    #[test]
    fn group_core_modules_short_circuits_with_no_network_call_when_given_no_paths() {
        let config = AnthropicConfig { api_key: "unused".into(), model: "claude-sonnet-5".into(), max_tokens: 1024, api_url: "http://127.0.0.1:1/v1/messages".into(), ..Default::default() };
        let groups = group_core_modules(&config, "demo", &[]).unwrap();
        assert!(groups.is_empty());
    }

    #[test]
    fn parse_module_groups_deserializes_into_module_labels() {
        let text = r#"{"modules":[{"label":"Authentication","file_paths":["src/auth/session.ts","src/auth/oauth.ts"]}]}"#;
        let groups = parse_module_groups(text).unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].label, "Authentication");
        assert_eq!(groups[0].file_paths, vec!["src/auth/session.ts", "src/auth/oauth.ts"]);
    }

    #[test]
    fn parse_module_groups_errors_clearly_on_malformed_json() {
        let err = parse_module_groups("not json").unwrap_err();
        assert!(err.to_string().contains("parsing Anthropic module-grouping JSON response"));
    }

    #[tokio::test]
    async fn group_core_modules_sends_output_config_json_schema_and_parses_the_response() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::body_string_contains("json_schema"))
            .and(wiremock::matchers::body_string_contains("src/auth/session.ts"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "content": [{"type": "text", "text": "{\"modules\":[{\"label\":\"Authentication\",\"file_paths\":[\"src/auth/session.ts\"]}]}"}],
                "usage": {"input_tokens": 20, "output_tokens": 10}
            })))
            .mount(&server)
            .await;

        let config = mock_config(&server.uri());
        let groups = group_core_modules(&config, "demo", &[PathBuf::from("src/auth/session.ts")]).unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].label, "Authentication");
    }
}
