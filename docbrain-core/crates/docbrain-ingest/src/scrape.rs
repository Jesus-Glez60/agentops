//! HTML documentation scraping — the piece that was actually missing for
//! Docbrain-1 ("ingest docs into version-aware molecules"): `discover()`
//! finds a library's docs URL, but nothing previously turned that URL into
//! real `doc_nodes` content. This module does exactly that: fetch a docs
//! page, split it into heading-scoped sections (the "molecule" boundary —
//! one section per `<h1>/<h2>/<h3>`, matching how a reader would actually
//! navigate the page), optionally following a handful of same-origin,
//! same-path-prefix links one level deep so a single `scrape` call can pull
//! in more than the landing page without turning into an unbounded crawler.

use anyhow::{Context, Result};
use scraper::{Html, Selector};
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScrapedSection {
    pub topic: String,
    pub content: String,
}

/// Fetches `url` and splits it into heading-scoped sections. If `max_pages`
/// is greater than 1, also follows up to `max_pages - 1` same-origin links
/// found on that page whose path starts with `url`'s own path (so a docs
/// site's nav bar linking out to the marketing homepage, blog, etc. doesn't
/// get pulled in) — a shallow, bounded crawl rather than a general spider.
pub fn scrape_docs(url: &str, max_pages: usize) -> Result<Vec<ScrapedSection>> {
    let base = Url::parse(url).with_context(|| format!("parsing docs URL '{url}'"))?;
    let html = fetch(url)?;
    let mut sections = parse_sections(&html);

    if max_pages > 1 {
        let links = same_scope_links(&html, &base);
        for link in links.into_iter().take(max_pages - 1) {
            match fetch(link.as_str()) {
                Ok(page_html) => sections.extend(parse_sections(&page_html)),
                Err(e) => {
                    // One follow-up page failing (dead link, redirect loop,
                    // transient error) shouldn't sink the whole scrape — the
                    // landing page's sections are still real, useful content.
                    eprintln!("docbrain: warning: failed to scrape linked page {link}: {e}");
                }
            }
        }
    }

    Ok(sections)
}

fn fetch(url: &str) -> Result<String> {
    let mut response = ureq::get(url)
        .header("User-Agent", "docbrain-ingest")
        .call()
        .with_context(|| format!("fetching docs page '{url}'"))?;
    response.body_mut().read_to_string().with_context(|| format!("reading response body for '{url}'"))
}

/// Splits a page into sections at each `<h1>`/`<h2>`/`<h3>` boundary. A page
/// with no headings at all becomes a single "overview" section rather than
/// being dropped — some docs pages (a short README-style page) genuinely
/// have no heading structure, and that's still real content worth keeping.
fn parse_sections(html: &str) -> Vec<ScrapedSection> {
    let document = Html::parse_document(html);
    let body_selector = Selector::parse("body").unwrap();
    let Some(body) = document.select(&body_selector).next() else {
        return Vec::new();
    };

    let heading_selector = Selector::parse("h1, h2, h3").unwrap();
    let mut sections = Vec::new();
    let mut current_topic: Option<String> = None;
    let mut current_text = String::new();

    for node in body.descendants() {
        if let Some(element) = node.value().as_element() {
            if matches!(element.name(), "h1" | "h2" | "h3") {
                if let Some(topic) = current_topic.take() {
                    push_section(&mut sections, topic, &current_text);
                }
                current_text.clear();
                let el_ref = scraper::ElementRef::wrap(node).unwrap();
                current_topic = Some(clean_text(&el_ref.text().collect::<String>()));
                continue;
            }
        }
        if let Some(text) = node.value().as_text() {
            // Skip text that belongs to a heading element itself — its
            // ancestor was already consumed above via `el_ref.text()`.
            let in_heading = node.ancestors().any(|a| {
                a.value().as_element().is_some_and(|e| matches!(e.name(), "h1" | "h2" | "h3"))
            });
            if !in_heading {
                current_text.push_str(text);
                current_text.push(' ');
            }
        }
    }
    if let Some(topic) = current_topic {
        push_section(&mut sections, topic, &current_text);
    } else if sections.is_empty() {
        let all_text = clean_text(&body.text().collect::<String>());
        if !all_text.is_empty() {
            sections.push(ScrapedSection { topic: "overview".to_string(), content: all_text });
        }
    }

    let _ = heading_selector; // selector kept for documentation of intent/tests below
    sections
}

fn push_section(sections: &mut Vec<ScrapedSection>, topic: String, raw_text: &str) {
    let content = clean_text(raw_text);
    if !content.is_empty() {
        sections.push(ScrapedSection { topic, content });
    }
}

fn clean_text(raw: &str) -> String {
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Same-origin links whose path starts with `base`'s own path — keeps a
/// shallow crawl inside the same docs section instead of wandering off to
/// a marketing site's blog/pricing pages that happen to share a domain.
fn same_scope_links(html: &str, base: &Url) -> Vec<Url> {
    let document = Html::parse_document(html);
    let link_selector = Selector::parse("a[href]").unwrap();
    let base_path = base.path();

    let mut seen = std::collections::BTreeSet::new();
    let mut links = Vec::new();
    for el in document.select(&link_selector) {
        let Some(href) = el.value().attr("href") else { continue };
        let Ok(resolved) = base.join(href) else { continue };
        if resolved.origin() != base.origin() {
            continue;
        }
        if !resolved.path().starts_with(base_path.trim_end_matches(|c| c != '/')) && resolved.path() != base_path {
            continue;
        }
        let mut normalized = resolved.clone();
        normalized.set_fragment(None);
        normalized.set_query(None);
        if normalized == *base {
            continue;
        }
        if seen.insert(normalized.to_string()) {
            links.push(normalized);
        }
    }
    links
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_page_into_heading_scoped_sections() {
        let html = r#"
            <html><body>
                <h1>Getting Started</h1>
                <p>Install the package first.</p>
                <h2>Configuration</h2>
                <p>Set your API key in an env var.</p>
                <h2>Advanced Usage</h2>
                <p>See the examples directory.</p>
            </body></html>
        "#;
        let sections = parse_sections(html);
        assert_eq!(sections.len(), 3);
        assert_eq!(sections[0].topic, "Getting Started");
        assert!(sections[0].content.contains("Install the package first"));
        assert_eq!(sections[1].topic, "Configuration");
        assert!(sections[1].content.contains("Set your API key"));
        assert_eq!(sections[2].topic, "Advanced Usage");
    }

    #[test]
    fn page_with_no_headings_becomes_a_single_overview_section() {
        let html = "<html><body><p>Just some plain content, no headings here.</p></body></html>";
        let sections = parse_sections(html);
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].topic, "overview");
        assert!(sections[0].content.contains("Just some plain content"));
    }

    #[test]
    fn empty_page_produces_no_sections() {
        let html = "<html><body></body></html>";
        assert!(parse_sections(html).is_empty());
    }

    #[test]
    fn same_scope_links_stay_within_the_docs_path_prefix_and_origin() {
        let html = r#"
            <html><body>
                <a href="/docs/routing">Routing</a>
                <a href="/docs/config">Config</a>
                <a href="/blog/announcement">Blog</a>
                <a href="https://other-site.example/docs/x">Other origin</a>
                <a href="/docs">Docs index</a>
            </body></html>
        "#;
        let base = Url::parse("https://example.com/docs/intro").unwrap();
        let links = same_scope_links(html, &base);
        let paths: Vec<&str> = links.iter().map(|u| u.path()).collect();
        assert!(paths.contains(&"/docs/routing"));
        assert!(paths.contains(&"/docs/config"));
        assert!(!paths.contains(&"/blog/announcement"));
        assert!(!paths.iter().any(|p| p.starts_with("/docs/x")), "must not follow a different origin");
    }

    // Hits a real, stable, well-known docs page — matching this project's
    // established practice (see docbrain-ingest's discover() tests) of
    // verifying network-facing code against the real thing rather than a
    // mock that could silently drift from reality.
    #[test]
    fn scrapes_a_real_docs_page_end_to_end() {
        let sections = scrape_docs("https://docs.rs/anyhow/latest/anyhow/", 1).unwrap();
        assert!(!sections.is_empty(), "expected at least one section from a real docs.rs page");
        assert!(sections.iter().any(|s| !s.content.is_empty()));
    }
}
