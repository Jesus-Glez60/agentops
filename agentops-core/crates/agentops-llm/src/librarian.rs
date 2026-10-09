//! The "librarian" classifier: decides whether a recorded gotcha is
//! portable (true for anyone using the technology) or specific to the
//! project it was found in, names the technology, and drafts a
//! project-neutral rewrite. Evaluation-only today — `agentops librarian
//! dry-run` runs it across several models against hand-labeled answers;
//! nothing here writes back to the graph.
//!
//! The schema is written in the strict-compatible form (every property
//! required, nullable values as `["string","null"]`,
//! `additionalProperties: false`) so the one schema works for Anthropic's
//! `output_config` and every `JsonMode` an OpenAI-compatible provider
//! takes — see the recorded gotcha on provider structured-output shapes.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::LlmConfig;

pub const OPERATION: &str = "librarian_classify";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Portable,
    ProjectSpecific,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LibrarianVerdict {
    pub scope: Scope,
    /// Free-form lowercase id (`sqlite`, `react`, `css-grid`, `axum`) —
    /// deliberately not restricted to registered libraries, since many
    /// portable gotchas are about engines, languages, or tools no docs
    /// library covers.
    pub technology: Option<String>,
    pub version: Option<String>,
    pub generalized_title: String,
    pub generalized_body: String,
    pub rationale: String,
}

pub fn verdict_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "scope": { "type": "string", "enum": ["portable", "project_specific"] },
            "technology": { "type": ["string", "null"] },
            "version": { "type": ["string", "null"] },
            "generalized_title": { "type": "string" },
            "generalized_body": { "type": "string" },
            "rationale": { "type": "string" }
        },
        "required": ["scope", "technology", "version", "generalized_title", "generalized_body", "rationale"],
        "additionalProperties": false
    })
}

fn build_prompt(title: &str, body: &str) -> String {
    format!(
        "You are cataloguing engineering knowledge. Below is a \"gotcha\" note recorded while working on one specific software project.\n\n\
         Decide its scope:\n\
         - portable: the lesson holds for anyone using the same technology, regardless of this project (e.g. a library API quirk, a database engine limitation, a CSS behavior, a tool's config rule).\n\
         - project_specific: the lesson only makes sense inside this project (its own architecture, deploy setup, internal modules, or a one-off bug in its own code).\n\n\
         Then:\n\
         - technology: the single main technology the lesson is about, as a short lowercase id (e.g. \"sqlite\", \"react\", \"axum\", \"css-grid\", \"docker-compose\", \"claude-code\"). Use null if project_specific with no external technology involved.\n\
         - version: the version range the lesson applies to if the note states or clearly implies one, else null.\n\
         - generalized_title / generalized_body: rewrite the note so a developer on a *different* project could use it. Keep the actual rule and any detail that makes it true (the exact failure mode, the condition that triggers it, how to detect or avoid it). Remove this project's file paths, commit hashes, internal names and counts. For a project_specific note, still write the most reusable version you can.\n\
         - rationale: one sentence on why you chose that scope.\n\n\
         Respond with a single JSON object with exactly these keys: scope, technology, version, generalized_title, generalized_body, rationale.\n\n\
         Note title: {title}\n\nNote body:\n{body}"
    )
}

/// Classifies one note. `title`/`body` should already be redacted
/// (`agentops_security::redact`) by the caller. A response that parses as
/// JSON but not as a `LibrarianVerdict` (wrong enum value, missing key) gets
/// one retry, then errors; each attempt is its own spend record.
pub fn classify_for_library(config: &LlmConfig, title: &str, body: &str) -> Result<LibrarianVerdict> {
    let prompt = build_prompt(title, body);
    let mut last_err = None;
    for _ in 0..2 {
        match crate::call_json(config, OPERATION, &prompt, verdict_schema()) {
            Ok(result) => match serde_json::from_str::<LibrarianVerdict>(&result.text) {
                Ok(verdict) => return Ok(verdict),
                Err(e) => last_err = Some(anyhow::Error::from(e).context(format!("response didn't match the verdict schema: {}", result.text))),
            },
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("no attempt made"))).context("classifying note for the librarian")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_strict_schema_verdict_round_trips_including_nulls() {
        let json = r#"{"scope":"project_specific","technology":null,"version":null,"generalized_title":"t","generalized_body":"b","rationale":"r"}"#;
        let verdict: LibrarianVerdict = serde_json::from_str(json).unwrap();
        assert_eq!(verdict.scope, Scope::ProjectSpecific);
        assert_eq!(verdict.technology, None);
    }

    #[test]
    fn the_schema_requires_every_property_it_declares() {
        let schema = verdict_schema();
        let props: Vec<&str> = schema["properties"].as_object().unwrap().keys().map(String::as_str).collect();
        let required: Vec<&str> = schema["required"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        for p in &props {
            assert!(required.contains(p), "{p} must be required for strict-mode providers");
        }
        assert_eq!(schema["additionalProperties"], false);
    }

    #[tokio::test]
    async fn a_schema_mismatch_retries_once_then_errors_with_two_spend_records() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "content": [{"type": "text", "text": "{\"scope\": \"maybe\"}"}],
                "usage": {"input_tokens": 10, "output_tokens": 3}
            })))
            .mount(&server)
            .await;

        let recorder = crate::UsageRecorder::default();
        let config = LlmConfig { api_key: "k".into(), api_url: format!("{}/v1/messages", server.uri()), ..Default::default() }.with_usage_sink(recorder.clone());
        assert!(classify_for_library(&config, "t", "b").is_err());
        assert_eq!(recorder.records().len(), 2, "both attempts spent tokens and must both be recorded");
    }
}
