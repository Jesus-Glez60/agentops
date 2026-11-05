//! A whole-word test on lowercased text, for the CLI's scope proposals.

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_words_only() {
        assert!(contains_word("on deploy-box: fine", "deploy-box"));
        assert!(!contains_word("check the inbox", "box"));
        assert!(contains_word("agentops_anthropic_api_key is set", "anthropic"), "an underscore separates words");
    }
}
