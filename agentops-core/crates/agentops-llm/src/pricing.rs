//! Hand-maintained $/million-token rates for Anthropic models — the one
//! shared source for both `agentops-cli usage sync`'s session-cost estimate
//! and this crate's own per-call LLM spend ledger. Verified against
//! Anthropic's pricing page (indexed as `anthropic-pricing@2026-10`);
//! re-check there whenever a new model ships, since a stale rate silently
//! misprices every row (the previous substring table priced Opus 5.5 at the
//! retired Opus 4.1 rate, ~3.75x too high).
//!
//! Matching is exact-id first, then a family+version prefix (so dated
//! snapshots like `claude-haiku-4-5-20251001` resolve), then `None` —
//! deliberately no bare-substring fallback, and no guessed default for an
//! unknown or non-Anthropic model. Callers must surface `None` as "unknown
//! cost", never as $0.

/// Rates in USD per million tokens. `cache_write` is the 5-minute
/// cache-write rate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelRates {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
}

const fn rates(input: f64, output: f64, cache_read: f64, cache_write: f64) -> ModelRates {
    ModelRates { input, output, cache_read, cache_write }
}

/// `(model id prefix, rates)`. Ordered so a longer, more specific prefix is
/// tried before any shorter one it extends (`claude-opus-5-5` before
/// `claude-opus-5`) — `rates_for` takes the first match.
const ANTHROPIC_RATES: &[(&str, ModelRates)] = &[
    ("claude-fable-5-1", rates(10.0, 50.0, 0.25, 12.5)),
    ("claude-fable-5", rates(10.0, 50.0, 1.0, 12.5)),
    ("claude-opus-5-5", rates(4.0, 20.0, 0.20, 5.0)),
    ("claude-opus-5", rates(5.0, 25.0, 0.50, 6.25)),
    ("claude-opus-4-8", rates(5.0, 25.0, 0.50, 6.25)),
    ("claude-opus-4-7", rates(5.0, 25.0, 0.50, 6.25)),
    ("claude-opus-4-6", rates(5.0, 25.0, 0.50, 6.25)),
    ("claude-opus-4-5", rates(5.0, 25.0, 0.50, 6.25)),
    ("claude-opus-4-1", rates(15.0, 75.0, 1.50, 18.75)),
    ("claude-opus-4", rates(15.0, 75.0, 1.50, 18.75)),
    ("claude-sonnet-5-5", rates(2.0, 10.0, 0.10, 2.5)),
    ("claude-sonnet-5", rates(2.0, 10.0, 0.20, 2.5)),
    ("claude-sonnet-4-6", rates(3.0, 15.0, 0.30, 3.75)),
    ("claude-sonnet-4-5", rates(3.0, 15.0, 0.30, 3.75)),
    ("claude-sonnet-4", rates(3.0, 15.0, 0.30, 3.75)),
    // Haiku 5.5: the <=100k-token-prompt tier. Every caller here sends
    // prompts far under that (`MAX_PROMPT_INPUT_TOKENS` is 60k).
    ("claude-haiku-5-5", rates(0.10, 0.50, 0.01, 0.125)),
    ("claude-haiku-4-5", rates(1.0, 5.0, 0.10, 1.25)),
];

/// Rates for `model` under `provider`, or `None` when unknown. Only the
/// `"anthropic"` provider has a rate table; every other provider (free
/// tiers, OpenAI-compatible endpoints) is `None` by design.
pub fn rates_for(provider: &str, model: &str) -> Option<ModelRates> {
    if provider != "anthropic" {
        return None;
    }
    ANTHROPIC_RATES.iter().find(|(prefix, _)| model == *prefix || model.strip_prefix(prefix).is_some_and(|rest| rest.starts_with('-'))).map(|(_, r)| *r)
}

/// Estimated USD cost, or `None` when the model's rates are unknown.
pub fn cost_estimate_usd(provider: &str, model: &str, input_tokens: i64, output_tokens: i64, cache_read_tokens: i64, cache_write_tokens: i64) -> Option<f64> {
    let r = rates_for(provider, model)?;
    let million = 1_000_000.0;
    Some((input_tokens as f64 * r.input + output_tokens as f64 * r.output + cache_read_tokens as f64 * r.cache_read + cache_write_tokens as f64 * r.cache_write) / million)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opus_5_5_uses_its_own_rate_not_the_retired_opus_4_1_rate() {
        assert_eq!(rates_for("anthropic", "claude-opus-5-5"), Some(rates(4.0, 20.0, 0.20, 5.0)));
        let cost = cost_estimate_usd("anthropic", "claude-opus-5-5", 1_000_000, 1_000_000, 0, 0).unwrap();
        assert!((cost - 24.0).abs() < 1e-9, "{cost}");
    }

    #[test]
    fn a_dated_snapshot_id_resolves_through_its_family_prefix() {
        assert_eq!(rates_for("anthropic", "claude-haiku-4-5-20251001"), rates_for("anthropic", "claude-haiku-4-5"));
    }

    #[test]
    fn a_longer_prefix_wins_over_the_shorter_one_it_extends() {
        assert_eq!(rates_for("anthropic", "claude-opus-5").unwrap().input, 5.0);
        assert_eq!(rates_for("anthropic", "claude-opus-5-5").unwrap().input, 4.0);
        assert_eq!(rates_for("anthropic", "claude-sonnet-5-5-20260901").unwrap().cache_read, 0.10);
    }

    #[test]
    fn unknown_models_and_non_anthropic_providers_have_no_rate() {
        assert_eq!(rates_for("anthropic", "claude-mystery-9"), None);
        assert_eq!(rates_for("anthropic", "opus"), None, "bare substrings must not match");
        assert_eq!(rates_for("groq", "llama-3.3-70b-versatile"), None);
        assert_eq!(cost_estimate_usd("groq", "llama-3.3-70b-versatile", 100, 100, 0, 0), None);
    }
}
