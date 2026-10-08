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

/// Case-insensitive substring match against a JSON value's own stringified
/// form -- deliberately coarse (not a schema-aware "does this item represent
/// an error" check), matching Headroom's own documented `SmartCrusher`
/// behavior: a keyword match anywhere in an item is reason enough to never
/// drop it, since a false *keep* costs budget but a false *drop* loses a
/// real error silently.
const ERROR_KEYWORDS: [&str; 5] = ["error", "fail", "exception", "panic", "fatal"];

/// Result of [`compress_json`] -- `value` is what the caller should actually
/// return to the agent; `dropped` is every item that didn't fit, each paired
/// with its full original JSON text so a caller with store access can
/// persist it (e.g. as a `NodeKind::ToolOutput` node, the same mechanism
/// `agentops-cli::hook_capture` already uses for over-budget tool output)
/// and splice a `fetch_content`-style pointer into `value` in its place.
/// Deliberately *not* done inside this function: `compress_json` has no
/// `GraphStore`/repo dependency today, and giving it one just to mint ids
/// would make this crate's one pure, store-free module depend on the same
/// store every other tool handler already has in scope at the call site —
/// see `tools.rs` for where that persistence actually happens.
pub struct CompressedJson {
    pub value: serde_json::Value,
    pub dropped: Vec<(usize, String)>,
}

/// SmartCrusher-equivalent: compresses a JSON array of objects by dropping
/// "common" items once the budget is exceeded, while always preserving (a)
/// any item matching an [`ERROR_KEYWORDS`] substring, and (b) rare-value
/// outliers on the first field common to a majority of items that has low
/// enough cardinality to be meaningfully "categorical" (mirrors Headroom's
/// documented Pareto check: the smallest set of values covering ≥80% of
/// items, capped at 5 distinct values -- a value outside that set is rare
/// enough to be load-bearing, not noise). A non-array input, or one that
/// already fits `budget` once serialized, is returned unchanged with
/// nothing dropped -- this is a budget-exceeded fallback, not a default
/// transform every JSON response should pay for.
pub fn compress_json(value: &serde_json::Value, budget: usize) -> CompressedJson {
    let items = match value.as_array() {
        Some(items) if !items.is_empty() => items,
        _ => return CompressedJson { value: value.clone(), dropped: Vec::new() },
    };

    if serde_json::to_string(value).map(|s| s.chars().count()).unwrap_or(usize::MAX) <= budget {
        return CompressedJson { value: value.clone(), dropped: Vec::new() };
    }

    let categorical_field = find_categorical_field(items);
    let rare_values: std::collections::HashSet<String> = categorical_field.as_deref().map(|field| rare_values_for_field(items, field)).unwrap_or_default();

    let is_preserved = |item: &serde_json::Value| -> bool {
        let text = item.to_string().to_lowercase();
        if ERROR_KEYWORDS.iter().any(|kw| text.contains(kw)) {
            return true;
        }
        if let Some(field) = &categorical_field {
            if let Some(v) = item.get(field).and_then(|v| v.as_str()) {
                if rare_values.contains(v) {
                    return true;
                }
            }
        }
        false
    };

    let mut kept = Vec::with_capacity(items.len());
    let mut dropped = Vec::new();
    // Greedy: every preserved item is always kept regardless of budget (a
    // preserved item being dropped would defeat the whole point of
    // preserving it); remaining budget after those is spent on "common"
    // items in their original order, oldest-fits-first, until exhausted.
    let preserved_chars: usize = items.iter().filter(|i| is_preserved(i)).map(|i| i.to_string().chars().count()).sum();
    let mut remaining_for_common = budget.saturating_sub(preserved_chars);

    for (idx, item) in items.iter().enumerate() {
        if is_preserved(item) {
            kept.push(item.clone());
            continue;
        }
        let item_chars = item.to_string().chars().count();
        if item_chars <= remaining_for_common {
            remaining_for_common -= item_chars;
            kept.push(item.clone());
        } else {
            dropped.push((idx, item.to_string()));
        }
    }

    if !dropped.is_empty() {
        kept.push(serde_json::json!({ "_compressed_items_dropped": dropped.len() }));
    }

    CompressedJson { value: serde_json::Value::Array(kept), dropped }
}

/// The first field present on a strict majority of `items` whose observed
/// values have low enough cardinality (≤ half the item count, and ≤ 50 per
/// Headroom's own documented cap) to be worth treating as categorical for
/// [`rare_values_for_field`] -- a free-text field (e.g. a message string
/// that's different on every item) would have cardinality ≈ item count and
/// is correctly skipped.
fn find_categorical_field(items: &[serde_json::Value]) -> Option<String> {
    let mut field_counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for item in items {
        if let Some(obj) = item.as_object() {
            for key in obj.keys() {
                *field_counts.entry(key.as_str()).or_default() += 1;
            }
        }
    }
    let majority = items.len() / 2;
    field_counts
        .into_iter()
        .filter(|&(_, count)| count > majority)
        .find_map(|(field, _)| {
            let values: std::collections::HashSet<&str> = items.iter().filter_map(|i| i.get(field).and_then(|v| v.as_str())).collect();
            let cardinality = values.len();
            (cardinality > 0 && cardinality <= (items.len() / 2).max(1) && cardinality <= 50).then(|| field.to_string())
        })
}

/// Greedy Pareto coverage: the smallest set of `field`'s distinct values
/// whose combined item count covers ≥80% of `items`, capped at 5 values.
/// Any value *not* in that set is "rare" -- matches Headroom's own
/// documented `detect_rare_status_values` algorithm (verified against its
/// real source, see the project note recorded for this plan).
fn rare_values_for_field(items: &[serde_json::Value], field: &str) -> std::collections::HashSet<String> {
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut total = 0usize;
    for item in items {
        if let Some(v) = item.get(field).and_then(|v| v.as_str()) {
            *counts.entry(v.to_string()).or_default() += 1;
            total += 1;
        }
    }
    if total == 0 {
        return std::collections::HashSet::new();
    }

    let mut sorted: Vec<(String, usize)> = counts.into_iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1));

    let threshold = (total * 4).div_ceil(5); // ceil(80% of total)
    let mut covered = 0usize;
    let mut top_values: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (value, count) in &sorted {
        if top_values.len() >= 5 || covered >= threshold {
            break;
        }
        top_values.insert(value.clone());
        covered += count;
    }

    sorted.into_iter().map(|(v, _)| v).filter(|v| !top_values.contains(v)).collect()
}

/// One CacheAligner-equivalent hazard: a span of `text` that looks like
/// volatile, cache-unsafe content (a UUID, JWT, hex hash, or ISO-8601
/// timestamp) -- detector-only, matching Headroom's own documented
/// "never mutate the cache hot zone" invariant. Callers decide what to do
/// with a hazard (e.g. warn before caching a response verbatim); this
/// function never rewrites `text` itself.
#[derive(Debug, Clone, PartialEq)]
pub struct CacheHazard {
    pub kind: CacheHazardKind,
    /// Byte range into the original `text`.
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheHazardKind {
    Uuid,
    Jwt,
    HexHash,
    Timestamp,
}

/// Scans `text` token-by-token (splitting on whitespace and common JSON
/// punctuation) for shape-only matches -- no regex crate dependency needed
/// for any of these, matching Headroom's own documented approach (stdlib
/// `uuid::UUID`/`datetime.fromisoformat` parsing, JWT shape checks, hex
/// length-set checks). Order of checks matters: a JWT is itself often
/// hex/base64-shaped, so it's checked before the generic hex-hash length
/// check to avoid double-classifying one token.
pub fn detect_cache_unsafe(text: &str) -> Vec<CacheHazard> {
    let mut hazards = Vec::new();
    let mut idx = 0;
    // `:` deliberately excluded -- an ISO-8601 timestamp's own `T00:00:00`
    // portion contains it, and splitting on it would shred exactly the
    // token this function needs intact to recognize as a timestamp
    // (confirmed live this session: the colon-split version silently never
    // matched any timestamp).
    for token in text.split(|c: char| c.is_whitespace() || matches!(c, '"' | ',' | '{' | '}' | '[' | ']')) {
        let start = text[idx..].find(token).map(|p| idx + p).unwrap_or(idx);
        let end = start + token.len();
        idx = end;
        if token.is_empty() {
            continue;
        }

        if is_uuid_shaped(token) {
            hazards.push(CacheHazard { kind: CacheHazardKind::Uuid, start, end });
        } else if is_jwt_shaped(token) {
            hazards.push(CacheHazard { kind: CacheHazardKind::Jwt, start, end });
        } else if is_hex_hash_shaped(token) {
            hazards.push(CacheHazard { kind: CacheHazardKind::HexHash, start, end });
        } else if is_iso8601_shaped(token) {
            hazards.push(CacheHazard { kind: CacheHazardKind::Timestamp, start, end });
        }
    }
    hazards
}

/// Standard 8-4-4-4-12 hex-digit UUID shape -- `pub` so other crates (e.g.
/// `agentops-heavy-api::tenant_repo`'s `derive_name_from_local_id`, which
/// needs the identical check to strip a UUID suffix off a `local_id`) can
/// reuse this instead of reimplementing the same validation.
pub fn is_uuid_shaped(token: &str) -> bool {
    let parts: Vec<&str> = token.split('-').collect();
    parts.len() == 5 && [8, 4, 4, 4, 12].iter().zip(&parts).all(|(&len, part)| part.len() == len && part.chars().all(|c| c.is_ascii_hexdigit()))
}

fn is_jwt_shaped(token: &str) -> bool {
    let parts: Vec<&str> = token.split('.').collect();
    parts.len() == 3 && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'))
}

/// Hex digest length set per Headroom's own documented check — md5 (32),
/// sha1 (40), sha256 (64).
fn is_hex_hash_shaped(token: &str) -> bool {
    matches!(token.len(), 32 | 40 | 64) && token.chars().all(|c| c.is_ascii_hexdigit())
}

fn is_iso8601_shaped(token: &str) -> bool {
    token.len() >= "2024-01-01T00:00:00".len() && token.as_bytes().get(4) == Some(&b'-') && token.as_bytes().get(7) == Some(&b'-') && token.as_bytes().get(10) == Some(&b'T') && token.chars().take(10).all(|c| c.is_ascii_digit() || c == '-')
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

    #[test]
    fn compress_json_returns_input_unchanged_when_it_already_fits_the_budget() {
        let value = serde_json::json!([{"status": "ok", "id": 1}, {"status": "ok", "id": 2}]);
        let result = compress_json(&value, 10_000);
        assert_eq!(result.value, value);
        assert!(result.dropped.is_empty());
    }

    #[test]
    fn compress_json_never_drops_an_error_keyword_match() {
        let mut items = vec![serde_json::json!({"status": "error", "id": 0, "msg": "disk full"})];
        for i in 1..50 {
            items.push(serde_json::json!({"status": "ok", "id": i}));
        }
        let value = serde_json::Value::Array(items);
        // Tight enough budget that most "ok" items must be dropped.
        let result = compress_json(&value, 200);

        assert!(!result.dropped.is_empty(), "budget must actually be exceeded for this test to mean anything");
        let kept = result.value.as_array().unwrap();
        assert!(kept.iter().any(|i| i.get("status").and_then(|s| s.as_str()) == Some("error")), "the error item must survive compression: {kept:?}");
    }

    #[test]
    fn compress_json_never_drops_a_rare_categorical_value() {
        // 48 "ok" items (common) + 2 "degraded" items (rare: well under the
        // 80%-coverage top-5 threshold) -- the rare value must survive even
        // with no error keyword present.
        let mut items = vec![serde_json::json!({"status": "degraded", "id": 0})];
        for i in 1..49 {
            items.push(serde_json::json!({"status": "ok", "id": i}));
        }
        items.push(serde_json::json!({"status": "degraded", "id": 49}));
        let value = serde_json::Value::Array(items);
        let result = compress_json(&value, 200);

        assert!(!result.dropped.is_empty());
        let kept = result.value.as_array().unwrap();
        let degraded_count = kept.iter().filter(|i| i.get("status").and_then(|s| s.as_str()) == Some("degraded")).count();
        assert_eq!(degraded_count, 2, "both rare 'degraded' items must survive: {kept:?}");
    }

    #[test]
    fn compress_json_output_can_exceed_budget_when_preserved_items_alone_do() {
        // Documents, not fixes, an intentional design limitation (see
        // `compress_json_then_cap`'s own doc comment): preserved items
        // (error-keyword matches here) are never dropped to hit a size
        // target, so a tiny budget with many preserved items produces
        // output much larger than requested. A caller needing a hard size
        // ceiling must still run the result through `cap`/`cap_sections`
        // afterward -- this function alone does not guarantee one.
        let items: Vec<_> = (0..50).map(|i| serde_json::json!({"status": "error", "id": i, "msg": "disk full"})).collect();
        let value = serde_json::Value::Array(items);
        let budget = 10; // far smaller than even one preserved item's own JSON.

        let result = compress_json(&value, budget);

        assert!(result.dropped.is_empty(), "every item matches the error keyword, so nothing should be droppable: {:?}", result.value);
        let actual_size = serde_json::to_string(&result.value).unwrap().chars().count();
        assert!(actual_size > budget, "output ({actual_size} chars) must exceed the requested budget ({budget}) here -- that's the documented limitation this test exists to verify");
    }

    #[test]
    fn compress_json_dropped_items_carry_their_full_original_json() {
        let mut items = Vec::new();
        for i in 0..50 {
            items.push(serde_json::json!({"status": "ok", "id": i, "padding": "x".repeat(50)}));
        }
        let value = serde_json::Value::Array(items);
        let result = compress_json(&value, 200);

        assert!(!result.dropped.is_empty());
        for (_, original) in &result.dropped {
            let parsed: serde_json::Value = serde_json::from_str(original).expect("dropped content must be valid JSON matching the original item");
            assert_eq!(parsed.get("status").and_then(|s| s.as_str()), Some("ok"));
        }
    }

    #[test]
    fn compress_json_ignores_a_non_array_value() {
        let value = serde_json::json!({"not": "an array"});
        let result = compress_json(&value, 1);
        assert_eq!(result.value, value);
        assert!(result.dropped.is_empty());
    }

    #[test]
    fn detect_cache_unsafe_flags_a_uuid() {
        let hazards = detect_cache_unsafe("session_id: 123e4567-e89b-12d3-a456-426614174000 done");
        assert!(hazards.iter().any(|h| h.kind == CacheHazardKind::Uuid), "{hazards:?}");
    }

    #[test]
    fn detect_cache_unsafe_flags_a_jwt_shaped_token() {
        let hazards = detect_cache_unsafe("token: eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk");
        assert!(hazards.iter().any(|h| h.kind == CacheHazardKind::Jwt), "{hazards:?}");
    }

    #[test]
    fn detect_cache_unsafe_flags_a_sha256_hex_hash() {
        let hash = "a".repeat(64);
        let hazards = detect_cache_unsafe(&format!("digest {hash} ok"));
        assert!(hazards.iter().any(|h| h.kind == CacheHazardKind::HexHash), "{hazards:?}");
    }

    #[test]
    fn detect_cache_unsafe_flags_an_iso8601_timestamp() {
        let hazards = detect_cache_unsafe("created_at 2024-01-01T00:00:00Z");
        assert!(hazards.iter().any(|h| h.kind == CacheHazardKind::Timestamp), "{hazards:?}");
    }

    #[test]
    fn detect_cache_unsafe_does_not_flag_ordinary_prose() {
        let hazards = detect_cache_unsafe("the quick brown fox jumps over the lazy dog, 42 times");
        assert!(hazards.is_empty(), "{hazards:?}");
    }
}
