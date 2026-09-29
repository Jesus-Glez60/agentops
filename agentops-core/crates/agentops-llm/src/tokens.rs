//! Token counting for prompt-budgeting, mirroring
//! `docbrain-ingest/src/chunk.rs`'s existing use of `tiktoken-rs`'s
//! `cl100k_base` — reused here rather than a different encoding so token
//! counts stay commensurable across the workspace. A practical
//! token-budget proxy for Claude's own tokenizer, not a claim of exact
//! parity with it.

use std::sync::LazyLock;

use tiktoken_rs::CoreBPE;

static ENCODER: LazyLock<CoreBPE> = LazyLock::new(|| tiktoken_rs::cl100k_base().expect("cl100k_base's bundled ranks must load"));

/// Counts `text`'s tokens under the shared `cl100k_base` encoder.
pub fn count_tokens(text: &str) -> usize {
    ENCODER.count_with_special_tokens(text)
}

/// Truncates `text` to at most `max_tokens` tokens (from the start), used
/// as `build_prompt`'s last-resort reduction once compression and dropping
/// related context have both been exhausted and the prompt is still over
/// budget.
pub fn truncate_to_tokens(text: &str, max_tokens: usize) -> String {
    let tokens = ENCODER.encode_with_special_tokens(text);
    if tokens.len() <= max_tokens {
        return text.to_string();
    }
    ENCODER.decode(&tokens[..max_tokens]).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_more_tokens_for_longer_text() {
        let short = count_tokens("hello");
        let long = count_tokens("hello ".repeat(200).trim());
        assert!(long > short);
    }

    #[test]
    fn truncate_leaves_short_text_unchanged() {
        assert_eq!(truncate_to_tokens("hello world", 1000), "hello world");
    }

    #[test]
    fn truncate_shortens_long_text() {
        let long = "hello world ".repeat(500);
        let truncated = truncate_to_tokens(&long, 10);
        assert!(count_tokens(&truncated) <= 10);
        assert!(truncated.len() < long.len());
    }
}
