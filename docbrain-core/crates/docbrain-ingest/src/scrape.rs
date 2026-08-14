//! Documentation fetching — prefers a site's own clean Markdown export when
//! available (the emerging `llms.txt`/content-negotiation convention: many
//! doc sites, including Next.js's, serve a `text/markdown` representation
//! of a canonical page via `Accept: text/markdown` or a `.md` URL suffix),
//! falling back to HTML scraping otherwise. Either way the result is a
//! single continuous Markdown-ish string (headings preserved as `#`/`##`/
//! ... prefixes); actual chunk boundaries are decided downstream by
//! `chunk.rs`'s heading-first, token-bounded splitter, not here.
//!
//! The HTML fallback fixes two confirmed `main`-branch gaps at the source:
//! - **Boilerplate stripping**: `nav`/`header`/`footer`/`aside`/`script`/
//!   `style` subtrees are skipped entirely, and `<main>`/`<article>` is
//!   preferred over `<body>` when present, so repeated nav/footer text
//!   doesn't get ingested on every crawled page.
//! - Heading structure survives into the output text instead of being
//!   flattened into whitespace-collapsed plain text.
//!
//! When a real Markdown export is available, both of these are moot — it's
//! already clean, already has real heading syntax, and is meaningfully
//! smaller (a ~200KB HTML page vs. a ~600-byte Markdown page for the same
//! content, measured against a real Next.js docs page) — so it's strictly
//! preferred when the site offers it.

use anyhow::{Context, Result};
use scraper::{ElementRef, Html, Selector};
use url::Url;

const BOILERPLATE_TAGS: &[&str] = &["nav", "header", "footer", "aside", "script", "style"];
const HEADING_TAGS: &[&str] = &["h1", "h2", "h3", "h4"];

/// One scraped page: its Markdown-ish text, an optional title pulled from
/// Markdown-export frontmatter (used as the page's default topic before its
/// first real heading), and same-page anchor links found in it (`(nearest
/// heading before the link, target #fragment)`) — used by `chunk.rs` to
/// build `CrossReference` edges between chunks.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ScrapedPage {
    pub markdown: String,
    pub default_topic: Option<String>,
    pub anchor_links: Vec<(String, String)>,
}

/// Fetches `url` and extracts its content, preferring a clean Markdown
/// export over HTML scraping when the site offers one. If `max_pages` is
/// greater than 1, also follows up to `max_pages - 1` further pages: an
/// `llms.txt` index (llmstxt.org's emerging convention — a curated
/// Markdown link list most real doc sites now publish specifically to
/// point an LLM/crawler at actual content, not nav/marketing chrome) if
/// one is found, falling back to same-origin, same-path-prefix links
/// scraped off the landing page's HTML otherwise. Link discovery always
/// uses HTML for the fallback path (Markdown exports don't reliably expose
/// the same nav structure), even when the page content itself is fetched
/// as Markdown.
pub fn scrape_docs(url: &str, max_pages: usize) -> Result<Vec<ScrapedPage>> {
    let base = Url::parse(url).with_context(|| format!("parsing docs URL '{url}'"))?;
    let html = fetch(url)?;
    let mut pages = vec![scrape_one_page(url, &html)];

    if max_pages > 1 {
        let links = fetch_llms_txt_links(&base).unwrap_or_else(|| same_scope_links(&html, &base));
        for link in links.into_iter().take(max_pages - 1) {
            match fetch(link.as_str()) {
                Ok(page_html) => pages.push(scrape_one_page(link.as_str(), &page_html)),
                Err(e) => {
                    // One follow-up page failing shouldn't sink the whole
                    // scrape — the landing page's content is still real.
                    eprintln!("docbrain: warning: failed to scrape linked page {link}: {e}");
                }
            }
        }
    }

    Ok(pages)
}

/// Tries a handful of conventional locations for a site's `llms.txt`
/// (llmstxt.org: a Markdown file of `[Title](url): description` links,
/// meant to be exactly the "here's our real documentation, skip the rest
/// of the site" index docbrain's own shallow crawl wants) — relative to
/// the docs URL's own directory first (most specific), then the docs path
/// with an `llms.txt` sibling, then the site root. Returns `None` if none
/// resolve to a non-empty link list, letting the caller fall back to HTML
/// link-scraping.
fn fetch_llms_txt_links(base: &Url) -> Option<Vec<Url>> {
    let mut candidates = Vec::new();
    if let Ok(sibling) = base.join("llms.txt") {
        candidates.push(sibling);
    }
    if let Ok(mut root) = Url::parse(&format!("{}://{}", base.scheme(), base.host_str()?)) {
        root.set_path("/llms.txt");
        candidates.push(root.clone());
        root.set_path("/docs/llms.txt");
        candidates.push(root);
    }
    candidates.dedup();

    for candidate in candidates {
        let Some(links) = fetch_and_parse_llms_txt(&candidate) else { continue };
        if links.is_empty() {
            continue;
        }

        // Some sites publish a root-level "meta" llms.txt whose own links
        // are a mix of the docs index, blog posts, and marketing/learn
        // pages (confirmed live: nextjs.org's root llms.txt is exactly
        // this) rather than a direct list of documentation pages — but one
        // of those links points to a more specific, nested llms.txt (e.g.
        // covering just /docs) that *does* list real content pages. Follow
        // that one hop deeper instead of crawling the meta index's own
        // mixed link list, which would pull in blog/learn pages docbrain
        // has no business scraping as "the library's docs".
        if let Some(nested_url) = links.iter().find(|u| u.as_str() != candidate.as_str() && u.path().ends_with("llms.txt")) {
            if let Some(nested_links) = fetch_and_parse_llms_txt(nested_url) {
                if !nested_links.is_empty() {
                    return Some(nested_links);
                }
            }
        }

        return Some(links);
    }
    None
}

fn fetch_and_parse_llms_txt(url: &Url) -> Option<Vec<Url>> {
    let mut resp = ureq::get(url.as_str()).header("User-Agent", "docbrain-ingest").call().ok()?;
    let body = resp.body_mut().read_to_string().ok()?;
    Some(extract_markdown_links(&body, url))
}

/// Every `[text](url)` target in `markdown`, resolved against `base` and
/// deduplicated in first-seen order — llms.txt's own link list, not the
/// heading-anchor extraction `extract_markdown_anchor_links` does (that one
/// only tracks in-page `#fragment` links; this one wants the real target
/// URLs, same-page or not).
fn extract_markdown_links(markdown: &str, base: &Url) -> Vec<Url> {
    let mut seen = std::collections::BTreeSet::new();
    let mut links = Vec::new();
    for line in markdown.lines() {
        let mut rest = line;
        while let Some(start) = rest.find("](") {
            let after = &rest[start + 2..];
            let Some(end) = after.find(')') else { break };
            let target = &after[..end];
            rest = &after[end + 1..];

            let Ok(resolved) = base.join(target) else { continue };
            if resolved.scheme() != "http" && resolved.scheme() != "https" {
                continue;
            }
            let mut normalized = resolved.clone();
            normalized.set_fragment(None);
            if seen.insert(normalized.to_string()) {
                links.push(normalized);
            }
        }
    }
    links
}

/// Tries a clean Markdown export of `url` first; falls back to extracting
/// from `html_fallback` (already fetched for link discovery, so this is
/// free) if the site doesn't offer one.
fn scrape_one_page(url: &str, html_fallback: &str) -> ScrapedPage {
    match fetch_markdown_export(url) {
        Some(raw) => {
            let (default_topic, body) = strip_frontmatter_title(&raw);
            ScrapedPage { anchor_links: extract_markdown_anchor_links(&body), markdown: body, default_topic }
        }
        None => extract_page(html_fallback),
    }
}

/// Tries the two conventions real doc sites use to serve a clean Markdown
/// representation of a page: `Accept: text/markdown` content negotiation
/// on the canonical URL (checked via the response's actual `Content-Type`,
/// not assumed from a 200 status), then a `.md` URL suffix. Returns `None`
/// if neither works — the HTML fallback path handles that case.
fn fetch_markdown_export(url: &str) -> Option<String> {
    if let Ok(mut resp) = ureq::get(url).header("User-Agent", "docbrain-ingest").header("Accept", "text/markdown").call() {
        let is_markdown = resp.headers().get("Content-Type").and_then(|v| v.to_str().ok()).is_some_and(|ct| ct.contains("markdown"));
        if is_markdown {
            if let Ok(body) = resp.body_mut().read_to_string() {
                return Some(body);
            }
        }
    }

    if !url.ends_with(".md") {
        let md_url = format!("{}.md", url.trim_end_matches('/'));
        if let Ok(mut resp) = ureq::get(&md_url).header("User-Agent", "docbrain-ingest").call() {
            let is_markdown = resp.headers().get("Content-Type").and_then(|v| v.to_str().ok()).is_none_or(|ct| !ct.contains("html"));
            if is_markdown {
                if let Ok(body) = resp.body_mut().read_to_string() {
                    return Some(body);
                }
            }
        }
    }

    None
}

/// Strips a leading `---`-delimited YAML-ish frontmatter block (the shape
/// Next.js's Markdown export uses: `title:`/`description:`/`url:`/
/// `version:` keys), returning its `title:` value (if any) separately —
/// used as the page's default topic before its first real Markdown
/// heading, instead of feeding raw frontmatter lines into the chunker as
/// if they were body text.
fn strip_frontmatter_title(markdown: &str) -> (Option<String>, String) {
    let trimmed = markdown.trim_start();
    let Some(rest) = trimmed.strip_prefix("---") else {
        return (None, markdown.to_string());
    };
    let Some(end) = rest.find("\n---") else {
        return (None, markdown.to_string());
    };

    let frontmatter = &rest[..end];
    let body = rest[end + 4..].trim_start();
    let title = frontmatter.lines().find_map(|l| l.trim().strip_prefix("title:").map(|v| v.trim().trim_matches('"').to_string()));
    (title, body.to_string())
}

/// Same fragment-link extraction as [`extract_page`] does for HTML, but for
/// already-Markdown content: tracks the nearest preceding `#`/`##`/...
/// heading and records `(heading, fragment)` for every `[text](#fragment)`
/// link found.
fn extract_markdown_anchor_links(markdown: &str) -> Vec<(String, String)> {
    let mut links = Vec::new();
    let mut current_heading = String::from("overview");

    for line in markdown.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') {
            let level = trimmed.chars().take_while(|&c| c == '#').count();
            let text = trimmed[level..].trim();
            if !text.is_empty() {
                current_heading = text.to_string();
            }
        }

        let mut rest = line;
        while let Some(idx) = rest.find("](#") {
            let after = &rest[idx + 3..];
            let Some(end) = after.find(')') else { break };
            let fragment = &after[..end];
            if !fragment.is_empty() {
                links.push((current_heading.clone(), fragment.to_string()));
            }
            rest = &after[end + 1..];
        }
    }

    links
}

fn fetch(url: &str) -> Result<String> {
    let mut response = ureq::get(url).header("User-Agent", "docbrain-ingest").call().with_context(|| format!("fetching docs page '{url}'"))?;
    response.body_mut().read_to_string().with_context(|| format!("reading response body for '{url}'"))
}

fn extract_page(html: &str) -> ScrapedPage {
    let document = Html::parse_document(html);
    let root = document
        .select(&Selector::parse("main").unwrap())
        .next()
        .or_else(|| document.select(&Selector::parse("article").unwrap()).next())
        .or_else(|| document.select(&Selector::parse("body").unwrap()).next());
    let Some(root) = root else {
        return ScrapedPage::default();
    };

    let mut markdown = String::new();
    let mut anchor_links = Vec::new();
    let mut current_heading = String::from("overview");

    for node in root.descendants() {
        let in_boilerplate = node.ancestors().any(|a| a.value().as_element().is_some_and(|e| BOILERPLATE_TAGS.contains(&e.name())))
            || node.value().as_element().is_some_and(|e| BOILERPLATE_TAGS.contains(&e.name()));
        if in_boilerplate {
            continue;
        }

        if let Some(element) = node.value().as_element() {
            if HEADING_TAGS.contains(&element.name()) {
                let el_ref = ElementRef::wrap(node).expect("element node wraps to ElementRef");
                let text = clean_text(&el_ref.text().collect::<String>());
                if !text.is_empty() {
                    let level: usize = element.name()[1..].parse().unwrap_or(2);
                    markdown.push_str(&"#".repeat(level));
                    markdown.push(' ');
                    markdown.push_str(&text);
                    markdown.push_str("\n\n");
                    current_heading = text;
                }
                continue;
            }
            if element.name() == "a" {
                if let Some(fragment) = element.attr("href").and_then(|h| h.strip_prefix('#')) {
                    if !fragment.is_empty() {
                        anchor_links.push((current_heading.clone(), fragment.to_string()));
                    }
                }
            }
        }

        if let Some(text) = node.value().as_text() {
            let in_heading = node.ancestors().any(|a| a.value().as_element().is_some_and(|e| HEADING_TAGS.contains(&e.name())));
            if !in_heading {
                let cleaned = clean_text(text);
                if !cleaned.is_empty() {
                    markdown.push_str(&cleaned);
                    markdown.push(' ');
                }
            }
        }
    }

    ScrapedPage { markdown: markdown.trim().to_string(), default_topic: None, anchor_links }
}

fn clean_text(raw: &str) -> String {
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Same-origin links whose path starts with `base`'s own path — keeps a
/// shallow crawl inside the same docs section instead of wandering off to a
/// marketing site's blog/pricing pages that happen to share a domain.
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
    fn extracts_heading_structure_as_markdown() {
        let html = r#"
            <html><body>
                <h1>Getting Started</h1>
                <p>Install the package first.</p>
                <h2>Configuration</h2>
                <p>Set your API key in an env var.</p>
            </body></html>
        "#;
        let page = extract_page(html);
        assert!(page.markdown.contains("# Getting Started"));
        assert!(page.markdown.contains("## Configuration"));
        assert!(page.markdown.contains("Install the package first"));
    }

    #[test]
    fn strips_boilerplate_nav_header_footer() {
        let html = r#"
            <html><body>
                <nav>Home | Docs | Blog</nav>
                <header>Site Header</header>
                <main>
                    <h1>Real Content</h1>
                    <p>This is the actual documentation.</p>
                </main>
                <footer>Copyright 2026</footer>
            </body></html>
        "#;
        let page = extract_page(html);
        assert!(page.markdown.contains("Real Content"));
        assert!(page.markdown.contains("actual documentation"));
        assert!(!page.markdown.contains("Home | Docs | Blog"));
        assert!(!page.markdown.contains("Site Header"));
        assert!(!page.markdown.contains("Copyright"));
    }

    #[test]
    fn prefers_main_over_body_when_present() {
        let html = r#"
            <html><body>
                <nav>nav text that would leak if body were used directly</nav>
                <main><h1>Doc Title</h1><p>content</p></main>
            </body></html>
        "#;
        let page = extract_page(html);
        assert!(!page.markdown.contains("nav text"));
    }

    #[test]
    fn records_same_page_anchor_links_with_their_nearest_heading() {
        let html = r##"
            <html><body>
                <h1>Overview</h1>
                <p>See <a href="#configuration">configuration</a> below.</p>
                <h2>Configuration</h2>
                <p>Details here.</p>
            </body></html>
        "##;
        let page = extract_page(html);
        assert_eq!(page.anchor_links, vec![("Overview".to_string(), "configuration".to_string())]);
    }

    #[test]
    fn empty_page_produces_empty_markdown() {
        let page = extract_page("<html><body></body></html>");
        assert!(page.markdown.is_empty());
    }

    #[test]
    fn same_scope_links_stay_within_the_docs_path_prefix_and_origin() {
        let html = r#"
            <html><body>
                <a href="/docs/routing">Routing</a>
                <a href="/blog/announcement">Blog</a>
                <a href="https://other-site.example/docs/x">Other origin</a>
            </body></html>
        "#;
        let base = Url::parse("https://example.com/docs/intro").unwrap();
        let links = same_scope_links(html, &base);
        let paths: Vec<&str> = links.iter().map(|u| u.path()).collect();
        assert!(paths.contains(&"/docs/routing"));
        assert!(!paths.contains(&"/blog/announcement"));
    }

    #[test]
    fn strips_frontmatter_and_extracts_its_title() {
        let raw = "---\ntitle: Getting Started\ndescription: intro\nurl: \"https://example.com/docs\"\n---\n\nWelcome to the docs.\n";
        let (title, body) = strip_frontmatter_title(raw);
        assert_eq!(title.as_deref(), Some("Getting Started"));
        assert_eq!(body.trim(), "Welcome to the docs.");
        assert!(!body.contains("---"));
    }

    #[test]
    fn markdown_with_no_frontmatter_is_returned_unchanged() {
        let raw = "# Heading\n\nBody text.";
        let (title, body) = strip_frontmatter_title(raw);
        assert_eq!(title, None);
        assert_eq!(body, raw);
    }

    #[test]
    fn extracts_anchor_links_from_markdown_by_nearest_heading() {
        let markdown = "# Overview\n\nSee [configuration](#configuration) below.\n\n## Configuration\n\nDetails here.";
        let links = extract_markdown_anchor_links(markdown);
        assert_eq!(links, vec![("Overview".to_string(), "configuration".to_string())]);
    }

    // Network-dependent, matching this crate's established practice of
    // verifying against a real site rather than a mock — this is the exact
    // page used in the manual end-to-end smoke test (docbrain-mcp's
    // `ingest_and_query` example).
    #[test]
    fn fetches_a_real_markdown_export_when_the_site_offers_one() {
        match fetch_markdown_export("https://nextjs.org/docs/app/getting-started") {
            Some(markdown) => {
                assert!(markdown.contains("title:"), "expected Next.js's Markdown export to include frontmatter");
                let (title, body) = strip_frontmatter_title(&markdown);
                assert_eq!(title.as_deref(), Some("Getting Started"));
                assert!(body.contains('#'), "expected real Markdown heading syntax in the body");
            }
            None => eprintln!("skipping network-dependent assertion: no markdown export reachable right now"),
        }
    }

    #[test]
    fn extracts_and_resolves_links_out_of_an_llms_txt_style_document() {
        let markdown = "# Docs\n\n> Summary line, not a link.\n\n## Getting Started\n\n- [Installation](https://example.com/docs/install): set up the project\n- [Relative page](/docs/other): a relative link\n- [Anchor only](#skip-me): should be skipped\n- [Mailto](mailto:someone@example.com): should be skipped\n";
        let base = Url::parse("https://example.com/docs/llms.txt").unwrap();
        let links = extract_markdown_links(markdown, &base);
        let paths: Vec<String> = links.iter().map(|u| u.to_string()).collect();
        assert!(paths.contains(&"https://example.com/docs/install".to_string()));
        assert!(paths.contains(&"https://example.com/docs/other".to_string()), "a root-relative link must resolve against the base URL");
        assert!(!paths.iter().any(|p| p.contains("skip-me")), "a same-page anchor link must not be treated as a followable page");
        assert!(!paths.iter().any(|p| p.starts_with("mailto:")), "a non-http(s) link must be skipped");
    }

    #[test]
    fn duplicate_links_in_llms_txt_are_deduplicated_in_first_seen_order() {
        let markdown = "- [A](https://example.com/a)\n- [A again](https://example.com/a)\n- [B](https://example.com/b)\n";
        let base = Url::parse("https://example.com/llms.txt").unwrap();
        let links = extract_markdown_links(markdown, &base);
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].as_str(), "https://example.com/a");
        assert_eq!(links[1].as_str(), "https://example.com/b");
    }

    // Network-dependent: confirms docbrain actually prefers a real site's
    // published llms.txt over blind HTML crawling when both are available.
    #[test]
    fn finds_a_real_sites_published_llms_txt() {
        let base = Url::parse("https://nextjs.org").unwrap();
        match fetch_llms_txt_links(&base) {
            Some(links) => {
                assert!(!links.is_empty());
                assert!(links.iter().all(|u| u.host_str() == Some("nextjs.org")), "llms.txt-derived links should stay on the docs site");
            }
            None => eprintln!("skipping network-dependent assertion: nextjs.org's llms.txt not reachable right now"),
        }
    }

    // Regression test for a real, confirmed case: nextjs.org's root
    // llms.txt is a site-wide meta index (docs + blog + Learn courses)
    // whose own links point to a more specific /docs/llms.txt containing
    // the actual content-page list. Following the meta index's raw links
    // directly pulled in blog posts and a 404 error page instead of real
    // documentation.
    #[test]
    fn follows_a_meta_llms_txt_one_hop_to_the_real_content_index() {
        let base = Url::parse("https://nextjs.org").unwrap();
        match fetch_llms_txt_links(&base) {
            Some(links) => {
                assert!(links.iter().any(|u| u.path().contains("/docs/app/getting-started")), "expected real Next.js doc pages from the nested /docs/llms.txt, not the meta index's own mixed links");
                assert!(!links.iter().any(|u| u.path().starts_with("/blog")), "must not pull in blog posts from the root meta index");
            }
            None => eprintln!("skipping network-dependent assertion: nextjs.org's llms.txt not reachable right now"),
        }
    }
}
