//! Whole-word tests on lowercased text, shared by recall (true keyword
//! matches) and the CLI's scope proposals.

/// Whether `word` occurs in `text` as a whole word: the characters on
/// either side aren't letters or digits, so `deploy-box` matches in "on
/// deploy-box:" but `box` doesn't match "inbox". Both are lowercase.
pub fn contains_word(text: &str, word: &str) -> bool {
    !word.is_empty()
        && text.match_indices(word).any(|(i, _)| {
            let before = text[..i].chars().next_back();
            let after = text[i + word.len()..].chars().next();
            !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric)
        })
}

/// `contains_word` for a recall keyword or one of its simple English
/// forms: `-s`, `-es`, `-ed` or `-ing` added or removed (`scan` and
/// `scans`, `reindex` and `reindexes`). Postgres keyword pools are stemmed
/// (`plainto_tsquery('english', ..)`), so a literal test alone would
/// undercount what they matched.
pub fn contains_keyword(text: &str, keyword: &str) -> bool {
    let mut forms = vec![keyword.to_string()];
    for suffix in ["s", "es", "ed", "ing"] {
        forms.push(format!("{keyword}{suffix}"));
        if let Some(stem) = keyword.strip_suffix(suffix).filter(|s| s.chars().count() >= 3) {
            forms.push(stem.to_string());
        }
    }
    forms.iter().any(|form| contains_word(text, form))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_words_only_and_simple_forms() {
        assert!(contains_word("on deploy-box: fine", "deploy-box"));
        assert!(!contains_word("check the inbox", "box"));
        assert!(contains_word("agentops_anthropic_api_key is set", "anthropic"), "an underscore separates words");
        assert!(contains_keyword("every scan makes a call", "scans"));
        assert!(contains_keyword("it reindexes nightly", "reindex"));
        assert!(!contains_keyword("rescanning", "scan"), "a different word, not a form");
    }
}
