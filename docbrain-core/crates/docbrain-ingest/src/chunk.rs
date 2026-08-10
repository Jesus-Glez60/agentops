//! Token-bounded chunking + dedup + synapse-edge construction — the actual
//! fix for `main`'s naive, unbounded, edge-less DOM-heading splitting.
//!
//! Splitting is **heading-first, token-bounded second**: the page is split
//! into sections at every Markdown heading boundary (never merging two
//! sections into one node, however small), and only a section that itself
//! exceeds [`MAX_CHUNK_TOKENS`] gets further split by
//! [`text_splitter`]'s `MarkdownSplitter` (sized in real tokens via
//! `tiktoken-rs`'s `cl100k_base` — a practical token-budget proxy, not a
//! claim of exact Anthropic tokenization). An earlier token-first design
//! (splitting the whole page by token budget alone) let small adjacent
//! sections merge into one node, which meant a node's `topic` label only
//! reflected its *first* heading while its content silently included
//! unrelated later sections too — confirmed live against real Next.js docs
//! before this fix. Each resulting chunk is upserted keyed on
//! `(library, version, content_hash)`, so a re-scrape only inserts
//! genuinely new content. Chunks are linked by `Sequence` (page order) and
//! `ParentSection` (heading hierarchy) edges as they're created; same-page
//! anchor links become `CrossReference` edges once every chunk's topic is
//! known.
//!
//! **Code examples are split out of their prose into their own
//! `CodeExample` nodes**, linked back via `HasExample` edges, before any
//! token-budget splitting happens. This is the planning/implementation
//! split: a planning-stage `search_docs` call (excluding `CodeExample` by
//! default) retrieves only the explanatory text without paying tokens for
//! code it doesn't need yet; an implementation-stage call can fetch exactly
//! the code linked to a prose node it already found, via
//! `get_code_examples`. It also means a fenced code block is never at risk
//! of being cut mid-block by the token-bounded sub-splitter, since it's
//! pulled out before that splitter ever sees the section.

use std::collections::HashMap;

use anyhow::Result;
use docbrain_graph::{content_hash, DocbrainStore, EdgeRelation, NewDocNode, NodeKind, UpsertOutcome};
use text_splitter::{ChunkConfig, MarkdownSplitter};

use agentops_embeddings::Embedder;

use crate::scrape::ScrapedPage;

/// Token ceiling per chunk. A concrete bound where `main` had none at all —
/// the number itself is a starting point to tune against real corpora, not
/// a value with special significance. Only sections that individually
/// exceed this get sub-split; a section under it is never merged with its
/// neighbor just because there'd be room.
pub const MAX_CHUNK_TOKENS: usize = 500;

#[derive(Debug, Clone, Copy)]
pub struct ChunkOutcome {
    pub node_id: i64,
    pub inserted: bool,
}

/// One heading-bounded section of a page: `level` 0 means the page's
/// preamble before its first heading (or the whole page, if it has none).
struct Section {
    level: usize,
    topic: String,
    content: String,
}

/// Chunks and stores every page from one `scrape_docs` call. `embed` is the
/// `Embedder` port, not `crate::embed::LocalEmbedder` directly, so tests
/// can supply a fast, offline, deterministic fake instead of hitting the
/// real local model — production callers pass `&LocalEmbedder`.
pub fn chunk_and_store(store: &dyn DocbrainStore, slug: &str, version: &str, pages: &[ScrapedPage], embed: &dyn Embedder) -> Result<Vec<ChunkOutcome>> {
    let bpe = tiktoken_rs::cl100k_base()?;
    let sub_splitter = MarkdownSplitter::new(ChunkConfig::new(MAX_CHUNK_TOKENS).with_sizer(bpe.clone()));

    let mut outcomes = Vec::new();
    // topic slug -> node id, across every page in this scrape, so
    // cross-reference links can resolve regardless of which page defined
    // the target heading.
    let mut topic_index: HashMap<String, i64> = HashMap::new();
    let mut pending_links: Vec<(String, String)> = Vec::new();

    for page in pages {
        if page.markdown.trim().is_empty() {
            continue;
        }
        let default_topic = page.default_topic.clone().unwrap_or_else(|| "overview".to_string());
        let sections = split_into_sections(&page.markdown, &default_topic);

        let mut heading_stack: Vec<(usize, i64)> = Vec::new();
        let mut prev_node_id: Option<i64> = None;

        for section in &sections {
            let (prose_content, examples) = extract_code_blocks(&section.content);
            let section_tokens = bpe.encode_with_special_tokens(&prose_content).len();
            let pieces: Vec<&str> = if section_tokens <= MAX_CHUNK_TOKENS { vec![&prose_content] } else { sub_splitter.chunks(&prose_content).collect() };

            for (i, piece) in pieces.iter().enumerate() {
                let token_count = bpe.encode_with_special_tokens(piece).len() as i64;
                let hash = content_hash(piece);
                let embedding = embed.embed(piece)?;

                let outcome = store.upsert_doc_node(
                    slug,
                    NewDocNode { version, topic: &section.topic, content: piece, content_hash: &hash, token_count, embedding: Some(&embedding), kind: NodeKind::Prose },
                )?;
                let node_id = outcome.node_id();
                outcomes.push(ChunkOutcome { node_id, inserted: matches!(outcome, UpsertOutcome::Inserted(_)) });
                if i == 0 {
                    topic_index.entry(slugify(&section.topic)).or_insert(node_id);
                }

                if let Some(prev) = prev_node_id {
                    store.add_doc_edge(prev, node_id, EdgeRelation::Sequence)?;
                }
                prev_node_id = Some(node_id);

                // Only the section's first piece is the "real" heading node
                // that later subsections should parent to — continuation
                // pieces from sub-splitting an oversized section are still
                // just that section's content, not a new heading level.
                // It's also the piece every code example the section had
                // attaches to, regardless of where in the section the code
                // originally appeared — same simplification as heading
                // attribution, and for the same reason: one canonical node
                // per section owns that section's metadata edges.
                if i == 0 {
                    for example in &examples {
                        let code_hash = content_hash(&example.code);
                        let code_embedding = embed.embed(&example.code)?;
                        let code_token_count = bpe.encode_with_special_tokens(&example.code).len() as i64;
                        let code_topic = match &example.language {
                            Some(lang) => format!("{} (code example: {lang})", section.topic),
                            None => format!("{} (code example)", section.topic),
                        };
                        let code_outcome = store.upsert_doc_node(
                            slug,
                            NewDocNode {
                                version,
                                topic: &code_topic,
                                content: &example.code,
                                content_hash: &code_hash,
                                token_count: code_token_count,
                                embedding: Some(&code_embedding),
                                kind: NodeKind::CodeExample,
                            },
                        )?;
                        let code_node_id = code_outcome.node_id();
                        outcomes.push(ChunkOutcome { node_id: code_node_id, inserted: matches!(code_outcome, UpsertOutcome::Inserted(_)) });
                        store.add_doc_edge(node_id, code_node_id, EdgeRelation::HasExample)?;
                    }

                    if section.level > 0 {
                        while heading_stack.last().is_some_and(|(l, _)| *l >= section.level) {
                            heading_stack.pop();
                        }
                        if let Some((_, parent_id)) = heading_stack.last() {
                            store.add_doc_edge(node_id, *parent_id, EdgeRelation::ParentSection)?;
                        }
                        heading_stack.push((section.level, node_id));
                    }
                }
            }
        }

        pending_links.extend(page.anchor_links.iter().cloned());
    }

    for (source_topic, target_fragment) in pending_links {
        let source_id = topic_index.get(&slugify(&source_topic)).copied();
        let target_id = topic_index.get(&slugify(&target_fragment)).copied();
        if let (Some(s), Some(t)) = (source_id, target_id) {
            if s != t {
                store.add_doc_edge(s, t, EdgeRelation::CrossReference)?;
            }
        }
    }

    Ok(outcomes)
}

/// One fenced code block pulled out of a section's prose.
struct CodeExample {
    language: Option<String>,
    /// The full fenced block (` ```lang\n...\n``` `), ready to use as-is —
    /// not just the bare code — so a `get_code_examples` caller gets
    /// something directly pasteable/renderable.
    code: String,
}

/// Pulls every fenced Markdown code block (` ```lang ... ``` `) out of
/// `content`, replacing each with a short `[Code example N]` placeholder in
/// the returned prose text. An unterminated fence (malformed input) is left
/// as plain prose rather than silently dropped.
fn extract_code_blocks(content: &str) -> (String, Vec<CodeExample>) {
    let mut prose = String::new();
    let mut examples = Vec::new();
    let mut lines = content.lines().peekable();

    while let Some(line) = lines.next() {
        let Some(lang_tag) = line.trim_start().strip_prefix("```") else {
            prose.push_str(line);
            prose.push('\n');
            continue;
        };

        let language = lang_tag.trim();
        let language = if language.is_empty() { None } else { Some(language.to_string()) };
        let mut code_lines = Vec::new();
        let mut closed = false;
        for code_line in lines.by_ref() {
            if code_line.trim_start().starts_with("```") {
                closed = true;
                break;
            }
            code_lines.push(code_line);
        }

        if closed {
            let fence_lang = language.clone().unwrap_or_default();
            let code = format!("```{fence_lang}\n{}\n```", code_lines.join("\n"));
            examples.push(CodeExample { language, code });
            prose.push_str(&format!("[Code example {}]\n", examples.len()));
        } else {
            // Unterminated fence — keep as prose rather than lose it.
            prose.push_str(line);
            prose.push('\n');
            for l in code_lines {
                prose.push_str(l);
                prose.push('\n');
            }
        }
    }

    (prose, examples)
}

/// Splits `markdown` at every heading line (`^#{1,6} text`) — a hard
/// boundary, never merged with its neighbor regardless of size. Text before
/// the first heading (or the whole page, if it has none) becomes a `level:
/// 0` section topic'd `default_topic`.
fn split_into_sections(markdown: &str, default_topic: &str) -> Vec<Section> {
    let mut sections = Vec::new();
    let mut current_level = 0usize;
    let mut current_topic = default_topic.to_string();
    let mut section_start = 0usize;
    let mut offset = 0usize;

    for line in markdown.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') {
            let level = trimmed.chars().take_while(|&c| c == '#').count();
            let text = trimmed[level..].trim();
            if level <= 6 && !text.is_empty() {
                if offset > section_start {
                    let content = markdown[section_start..offset].trim().to_string();
                    if !content.is_empty() {
                        sections.push(Section { level: current_level, topic: current_topic.clone(), content });
                    }
                }
                section_start = offset;
                current_level = level;
                current_topic = text.to_string();
            }
        }
        offset += line.len();
    }
    if offset > section_start {
        let content = markdown[section_start..offset].trim().to_string();
        if !content.is_empty() {
            sections.push(Section { level: current_level, topic: current_topic, content });
        }
    }

    sections
}

fn slugify(s: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for c in s.to_lowercase().chars() {
        if c.is_alphanumeric() {
            out.push(c);
            last_dash = false;
        } else if !last_dash && !out.is_empty() {
            out.push('-');
            last_dash = true;
        }
    }
    out.trim_end_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scrape::ScrapedPage;
    use docbrain_graph::SqliteDocbrainStore;

    struct FakeEmbedder;
    impl Embedder for FakeEmbedder {
        fn embed(&self, _text: &str) -> Result<Vec<f32>> {
            Ok(vec![0.1_f32; docbrain_graph::EMBEDDING_DIM])
        }
    }
    const FAKE_EMBED: FakeEmbedder = FakeEmbedder;

    fn store_with_library(slug: &str) -> SqliteDocbrainStore {
        let store = SqliteDocbrainStore::open_in_memory().unwrap();
        store.add_library(slug, slug, None, Some("https://example.com/docs")).unwrap();
        store
    }

    fn page(markdown: impl Into<String>) -> ScrapedPage {
        ScrapedPage { markdown: markdown.into(), default_topic: None, anchor_links: vec![] }
    }

    #[test]
    fn no_chunk_exceeds_the_token_ceiling() {
        let store = store_with_library("bigdoc");
        let long_paragraph = "This is a sentence about configuration options and behavior. ".repeat(200);
        let p = page(format!("# Overview\n\n{long_paragraph}"));

        chunk_and_store(&store, "bigdoc", "1.0", std::slice::from_ref(&p), &FAKE_EMBED).unwrap();

        let bpe = tiktoken_rs::cl100k_base().unwrap();
        let nodes = store.all_doc_nodes("bigdoc").unwrap();
        assert!(nodes.len() > 1, "a long heading-sparse page must split into multiple chunks");
        for node in &nodes {
            let tokens = bpe.encode_with_special_tokens(&node.content).len();
            assert!(tokens <= MAX_CHUNK_TOKENS, "chunk exceeded token ceiling: {tokens} > {MAX_CHUNK_TOKENS}");
        }
    }

    #[test]
    fn heading_sparse_page_gets_sequence_edges_between_its_chunks() {
        let store = store_with_library("bigdoc");
        let long_paragraph = "Filler sentence describing the API in detail. ".repeat(200);
        let p = page(format!("# Overview\n\n{long_paragraph}"));

        let outcomes = chunk_and_store(&store, "bigdoc", "1.0", std::slice::from_ref(&p), &FAKE_EMBED).unwrap();
        assert!(outcomes.len() > 1);
        for pair in outcomes.windows(2) {
            let edges = store.node_edges(pair[1].node_id).unwrap();
            assert!(
                edges.iter().any(|e| e.from_node == pair[0].node_id && e.to_node == pair[1].node_id && matches!(e.relation, EdgeRelation::Sequence)),
                "expected a Sequence edge between consecutive chunks"
            );
        }
    }

    #[test]
    fn rescraping_identical_content_produces_no_new_duplicate_nodes() {
        let store = store_with_library("next");
        let p = page("# Getting Started\n\nInstall the package first.");

        chunk_and_store(&store, "next", "1.0", std::slice::from_ref(&p), &FAKE_EMBED).unwrap();
        let count_after_first = store.all_doc_nodes("next").unwrap().len();

        chunk_and_store(&store, "next", "1.0", std::slice::from_ref(&p), &FAKE_EMBED).unwrap();
        let count_after_second = store.all_doc_nodes("next").unwrap().len();

        assert_eq!(count_after_first, count_after_second, "re-scraping identical content must not duplicate nodes");
    }

    #[test]
    fn small_adjacent_sections_are_never_merged_into_one_node() {
        // Regression test for the exact gap found live against real Next.js
        // docs: two short sections that would both fit under one token
        // budget must still become two separate nodes, each topic'd
        // accurately — not silently merged under the first section's topic.
        let store = store_with_library("next");
        let p = page("# Overview\n\nShort intro.\n\n## Configuration\n\nShort config note.\n\n## Deployment\n\nShort deploy note.");

        let outcomes = chunk_and_store(&store, "next", "1.0", std::slice::from_ref(&p), &FAKE_EMBED).unwrap();
        let nodes: Vec<_> = outcomes.iter().map(|o| store.get_node(o.node_id).unwrap().unwrap()).collect();
        let topics: Vec<&str> = nodes.iter().map(|n| n.topic.as_str()).collect();

        assert_eq!(topics, vec!["Overview", "Configuration", "Deployment"], "each heading must produce its own node, never merged with a neighbor");
        assert!(!nodes[0].content.contains("Configuration"), "Overview's node must not also contain Configuration's content");
    }

    #[test]
    fn a_page_starting_with_no_heading_uses_the_provided_default_topic() {
        let store = store_with_library("next");
        let p = ScrapedPage { markdown: "Just a short preamble with no heading at all.".to_string(), default_topic: Some("Getting Started".to_string()), anchor_links: vec![] };

        let outcomes = chunk_and_store(&store, "next", "1.0", std::slice::from_ref(&p), &FAKE_EMBED).unwrap();
        assert_eq!(outcomes.len(), 1);
        let node = store.get_node(outcomes[0].node_id).unwrap().unwrap();
        assert_eq!(node.topic, "Getting Started");
    }

    /// Repeats `phrase` until the tokenizer counts at least `target_tokens`
    /// — used to force a section past the ceiling so it's actually
    /// sub-split, exercising the oversized-section path specifically.
    fn text_of_at_least_tokens(bpe: &tiktoken_rs::CoreBPE, phrase: &str, target_tokens: usize) -> String {
        let mut text = String::new();
        while bpe.encode_with_special_tokens(&text).len() < target_tokens {
            text.push_str(phrase);
        }
        text
    }

    #[test]
    fn parent_section_edge_links_a_subsection_to_its_parent_heading() {
        let store = store_with_library("next");
        let p = page("# Getting Started\n\nIntro text.\n\n## Configuration\n\nSet your API key.");

        let outcomes = chunk_and_store(&store, "next", "1.0", std::slice::from_ref(&p), &FAKE_EMBED).unwrap();
        let nodes: Vec<_> = outcomes.iter().map(|o| store.get_node(o.node_id).unwrap().unwrap()).collect();
        let getting_started = nodes.iter().find(|n| n.topic == "Getting Started").expect("a chunk topic'd Getting Started");
        let configuration = nodes.iter().find(|n| n.topic == "Configuration").expect("a chunk topic'd Configuration");

        let edges = store.node_edges(configuration.id).unwrap();
        assert!(
            edges.iter().any(|e| e.from_node == configuration.id && e.to_node == getting_started.id && matches!(e.relation, EdgeRelation::ParentSection)),
            "expected a ParentSection edge from the Configuration subsection to its Getting Started parent"
        );
    }

    #[test]
    fn oversized_section_is_sub_split_but_stays_under_one_topic() {
        let store = store_with_library("next");
        let bpe = tiktoken_rs::cl100k_base().unwrap();
        let long_body = text_of_at_least_tokens(&bpe, "Detailed configuration explanation sentence. ", 900);
        let p = page(format!("# Configuration\n\n{long_body}"));

        let outcomes = chunk_and_store(&store, "next", "1.0", std::slice::from_ref(&p), &FAKE_EMBED).unwrap();
        assert!(outcomes.len() > 1, "a 900-token section under a 500-token ceiling must be sub-split");
        let nodes: Vec<_> = outcomes.iter().map(|o| store.get_node(o.node_id).unwrap().unwrap()).collect();
        assert!(nodes.iter().all(|n| n.topic == "Configuration"), "sub-split pieces of one section must all keep that section's topic");
    }

    #[test]
    fn cross_reference_edge_links_same_page_anchor_to_its_target_heading() {
        let store = store_with_library("next");
        let p = ScrapedPage {
            markdown: "# Overview\n\nSee configuration below.\n\n## Configuration\n\nDetails here.".to_string(),
            default_topic: None,
            anchor_links: vec![("Overview".to_string(), "configuration".to_string())],
        };

        let outcomes = chunk_and_store(&store, "next", "1.0", std::slice::from_ref(&p), &FAKE_EMBED).unwrap();
        let nodes: Vec<_> = outcomes.iter().map(|o| store.get_node(o.node_id).unwrap().unwrap()).collect();
        let overview_node = nodes.iter().find(|n| n.topic == "Overview").expect("a chunk topic'd Overview");
        let configuration_node = nodes.iter().find(|n| n.topic == "Configuration").expect("a chunk topic'd Configuration");

        let edges = store.node_edges(overview_node.id).unwrap();
        assert!(
            edges.iter().any(|e| e.from_node == overview_node.id && e.to_node == configuration_node.id && matches!(e.relation, EdgeRelation::CrossReference)),
            "expected a CrossReference edge from the Overview anchor to the Configuration heading it points at"
        );
    }

    #[test]
    fn extract_code_blocks_pulls_fenced_code_out_of_prose() {
        let content = "Some intro text.\n\n```tsx\nexport default function Page() {}\n```\n\nMore text after.";
        let (prose, examples) = extract_code_blocks(content);

        assert_eq!(examples.len(), 1);
        assert_eq!(examples[0].language.as_deref(), Some("tsx"));
        assert!(examples[0].code.contains("export default function Page"));
        assert!(prose.contains("[Code example 1]"));
        assert!(!prose.contains("export default function Page"), "the code itself must not remain in the prose text");
        assert!(prose.contains("More text after"));
    }

    #[test]
    fn extract_code_blocks_leaves_an_unterminated_fence_as_prose() {
        let content = "Intro.\n\n```tsx\nunterminated code with no closing fence";
        let (prose, examples) = extract_code_blocks(content);
        assert!(examples.is_empty());
        assert!(prose.contains("unterminated code with no closing fence"), "malformed input must not silently lose content");
    }

    #[test]
    fn code_examples_become_separate_nodes_linked_via_has_example_edges() {
        let store = store_with_library("next");
        let p = page("# Creating a dynamic segment\n\nWrap the segment name in square brackets.\n\n```tsx\nexport default function BlogPostPage() {}\n```\n\nLearn more about dynamic segments.");

        let outcomes = chunk_and_store(&store, "next", "1.0", std::slice::from_ref(&p), &FAKE_EMBED).unwrap();
        let nodes: Vec<_> = outcomes.iter().map(|o| store.get_node(o.node_id).unwrap().unwrap()).collect();

        let prose_node = nodes.iter().find(|n| n.kind == docbrain_graph::NodeKind::Prose).expect("expected a prose node");
        let code_node = nodes.iter().find(|n| n.kind == docbrain_graph::NodeKind::CodeExample).expect("expected a code example node");

        assert!(!prose_node.content.contains("BlogPostPage"), "prose node must not contain the raw code");
        assert!(prose_node.content.contains("[Code example 1]"), "prose node should reference the extracted example");
        assert!(code_node.content.contains("BlogPostPage"));

        let edges = store.node_edges(prose_node.id).unwrap();
        assert!(
            edges.iter().any(|e| e.from_node == prose_node.id && e.to_node == code_node.id && matches!(e.relation, EdgeRelation::HasExample)),
            "expected a HasExample edge from the prose node to its code example"
        );
    }
}
