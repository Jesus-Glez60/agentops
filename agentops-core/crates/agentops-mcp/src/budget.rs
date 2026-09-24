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

/// Assembles named sections (in priority order — earliest is most
/// important) into one string capped at `total_budget` chars. Checked
/// against `docbrain-mcp`'s existing token-budget mechanism
/// (`docbrain_ingest::retrieve::get_docs`/`search_docs`) before writing this
/// — that one accumulates a single ordered list of homogeneous items until
/// a budget is exhausted; this needs guaranteed per-category representation
/// with lower-priority *sections* trimmed first, a genuinely different
/// shape, so it isn't a duplicate. Doesn't point anywhere to fetch the rest
/// (unlike `cap`) — a session guide's sections are ephemeral summaries, not
/// stored node content.
pub fn cap_sections(sections: &[(&str, String)], total_budget: usize) -> String {
    let mut out = String::new();
    let mut remaining = total_budget;

    for (title, body) in sections {
        if body.is_empty() {
            continue;
        }
        let header = format!("## {title}\n");
        let separator = "\n\n";
        let overhead = header.chars().count() + separator.chars().count();
        if remaining <= overhead {
            // No room even for this section's header — drop it and every
            // lower-priority section after it, never a higher-priority one.
            break;
        }
        let available_for_body = remaining - overhead;
        let body_to_use = match body.char_indices().nth(available_for_body) {
            None => body.clone(),
            Some((byte_idx, _)) => format!("{}…", &body[..byte_idx]),
        };
        remaining = remaining.saturating_sub(overhead + body_to_use.chars().count());
        out.push_str(&header);
        out.push_str(&body_to_use);
        out.push_str(separator);
    }

    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cap_sections_includes_every_section_when_the_budget_is_generous() {
        let sections = [("Task", "Fix the bug".to_string()), ("Files", "a.rs, b.rs".to_string())];
        let out = cap_sections(&sections, 1000);
        assert!(out.contains("## Task\nFix the bug"));
        assert!(out.contains("## Files\na.rs, b.rs"));
    }

    #[test]
    fn cap_sections_skips_empty_sections() {
        let sections = [("Task", "Fix the bug".to_string()), ("Errors", String::new())];
        let out = cap_sections(&sections, 1000);
        assert!(!out.contains("## Errors"));
    }

    #[test]
    fn cap_sections_drops_lower_priority_sections_first_when_budget_is_tight() {
        let sections = [("Task", "a".repeat(50)), ("LowPriority", "b".repeat(50))];
        let out = cap_sections(&sections, 20);
        assert!(out.contains("## Task"), "the higher-priority section must survive: {out}");
        assert!(!out.contains("LowPriority"), "the lower-priority section must be dropped, not the higher one: {out}");
    }

    #[test]
    fn cap_sections_trims_a_section_body_that_partially_fits() {
        let sections = [("Task", "a".repeat(100))];
        let out = cap_sections(&sections, 30);
        assert!(out.starts_with("## Task\n"));
        assert!(out.len() < 100, "body should have been trimmed: {out}");
        assert!(out.contains('…'));
    }

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
