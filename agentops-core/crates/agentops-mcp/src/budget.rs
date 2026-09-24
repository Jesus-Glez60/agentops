//! Shared truncation for MCP tool responses that could otherwise inline an
//! unbounded amount of node content (get_symbol, related_context,
//! explain_symbol) into the caller's context window. Every truncatable value
//! in this crate already has (or is handed) a stable node id, so capping
//! never needs new storage — a truncated result just tells the caller to
//! call `fetch_content` with that id to get the rest.

/// Default cap, in characters, applied when a tool call doesn't pass its own
/// `max_chars` override.
pub const DEFAULT_CHAR_BUDGET: usize = 4000;

pub struct Capped {
    pub text: String,
    pub truncated: bool,
}

/// Truncates `text` to at most `budget` chars, always on a UTF-8 char
/// boundary. If truncated, appends a note telling the caller to call
/// `fetch_content` with `id` to get the full text.
pub fn cap(text: &str, budget: usize, id: i64) -> Capped {
    match text.char_indices().nth(budget) {
        None => Capped { text: text.to_string(), truncated: false },
        Some((byte_idx, _)) => {
            let mut truncated = text[..byte_idx].to_string();
            truncated.push_str(&format!("\n… [truncated — call fetch_content with id {id} for the full content]"));
            Capped { text: truncated, truncated: true }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_text_is_not_truncated() {
        let capped = cap("hello", 4000, 1);
        assert_eq!(capped.text, "hello");
        assert!(!capped.truncated);
    }

    #[test]
    fn long_text_is_truncated_at_budget() {
        let text = "a".repeat(5000);
        let capped = cap(&text, 4000, 42);
        assert!(capped.truncated);
        assert!(capped.text.starts_with(&"a".repeat(4000)));
        assert!(capped.text.contains("fetch_content with id 42"));
    }

    #[test]
    fn truncation_is_utf8_boundary_safe() {
        // Each "é" is 2 bytes but 1 char — a naive byte-index truncation
        // could land mid-codepoint.
        let text = "é".repeat(10);
        let capped = cap(&text, 5, 7);
        assert!(capped.truncated);
        assert!(capped.text.starts_with(&"é".repeat(5)));
    }
}
