//! Per-call LLM spend recording. `send_messages` reports every exit path —
//! success, auth/rate-limit/other HTTP failures, transport errors, and
//! unparseable JSON — to the config's optional `LlmUsageSink` *before*
//! returning or propagating the error, so a failed call is never silently
//! uncounted.
//!
//! **Why a buffer, not a store-backed sink**: `GraphStore` has no
//! `Send`/`Sync` bound (`SqliteGraphStore` wraps a `rusqlite::Connection`),
//! so a sink can't hold a store behind the `Arc<dyn LlmUsageSink>` the
//! config carries. `UsageRecorder` buffers records in memory instead, and
//! the caller — which already holds `(store, repo)` — persists them right
//! after the LLM function returns, on success *and* on error:
//!
//! ```ignore
//! let recorder = UsageRecorder::default();
//! let config = LlmConfig::from_env()?.with_usage_sink(recorder.clone());
//! let result = explain_symbol(store, &config, &repo, id);
//! recorder.persist(store, &repo);
//! let id = result?;
//! ```
//!
//! The same recorder is what `librarian dry-run` reads its token/latency
//! report from, so the dry run measures through exactly the path the
//! server ledger uses.
//!
//! `persist` calls `GraphStore::record_llm_usage`, which for
//! `PostgresGraphStore` runs `rt.block_on` internally — it must be called
//! from a non-runtime thread (e.g. inside `spawn_blocking`, which is where
//! `/mcp` tool calls already run), never directly on an async task.

use std::sync::{Arc, Mutex};

use agentops_graph::{GraphStore, NewLlmUsage};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmErrorKind {
    Auth,
    RateLimited,
    BadJson,
    Other,
}

impl LlmErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            LlmErrorKind::Auth => "auth",
            LlmErrorKind::RateLimited => "rate_limited",
            LlmErrorKind::BadJson => "bad_json",
            LlmErrorKind::Other => "other",
        }
    }
}

/// One LLM API call's outcome, as reported to an `LlmUsageSink`.
#[derive(Debug, Clone, PartialEq)]
pub struct LlmCallRecord {
    pub operation: String,
    pub provider: String,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub latency_ms: u64,
    pub success: bool,
    pub error_kind: Option<LlmErrorKind>,
}

pub trait LlmUsageSink: Send + Sync {
    fn record(&self, rec: &LlmCallRecord);
}

/// In-memory `LlmUsageSink` — see this module's doc comment for why
/// persisting is a separate, caller-driven step.
#[derive(Clone, Default)]
pub struct UsageRecorder {
    records: Arc<Mutex<Vec<LlmCallRecord>>>,
}

impl LlmUsageSink for UsageRecorder {
    fn record(&self, rec: &LlmCallRecord) {
        self.records.lock().unwrap_or_else(|e| e.into_inner()).push(rec.clone());
    }
}

impl UsageRecorder {
    /// A snapshot of every record so far, without clearing them.
    pub fn records(&self) -> Vec<LlmCallRecord> {
        self.records.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Drains every buffered record.
    pub fn take(&self) -> Vec<LlmCallRecord> {
        std::mem::take(&mut *self.records.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// Drains and writes every buffered record to `store` under `repo`,
    /// pricing each via `pricing::cost_estimate_usd`. Best-effort: a write
    /// failure is logged and skipped, never propagated — recording spend
    /// must not turn a successful LLM call into a failed tool call. Returns
    /// how many rows were written.
    pub fn persist(&self, store: &dyn GraphStore, repo: &str) -> usize {
        let mut written = 0;
        for rec in self.take() {
            let usage = NewLlmUsage {
                repo: repo.to_string(),
                operation: rec.operation.clone(),
                provider: rec.provider.clone(),
                model: rec.model.clone(),
                input_tokens: rec.input_tokens as i64,
                output_tokens: rec.output_tokens as i64,
                cost_estimate_usd: crate::pricing::cost_estimate_usd(&rec.provider, &rec.model, rec.input_tokens as i64, rec.output_tokens as i64, 0, 0),
                latency_ms: rec.latency_ms as i64,
                success: rec.success,
                error_kind: rec.error_kind.map(|k| k.as_str().to_string()),
            };
            match store.record_llm_usage(usage) {
                Ok(_) => written += 1,
                Err(e) => eprintln!("llm_usage: failed to record {} call for {repo}: {e}", rec.operation),
            }
        }
        written
    }
}
