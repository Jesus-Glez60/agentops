//! Use-case layer: library discovery, doc scraping/chunking/embedding, and
//! changelog sync. Rebuilt clean for the vnext pass — see
//! ~/.claude/plans/let-s-start-module-1-robust-hummingbird.md for the full
//! design and the confirmed `main`-branch gaps this closes.
//!
//! Like `main`, this crate is explicitly NOT under `agentops-scanner`'s
//! zero-network-egress invariant — fetching from the network (registries,
//! GitHub, and now a one-time embedding-model download) is its entire job.

mod changelog;
mod chunk;
mod classify;
mod discover;
pub mod embed;
pub mod retrieve;
mod scrape;

pub use changelog::{ChangelogFetchEntry, ChangelogSync, GitHubReleasesSync};
pub use chunk::{chunk_and_store, ChunkOutcome, MAX_CHUNK_TOKENS};
pub use classify::classify_dependency;
pub use discover::{DiscoveredLibrary, Ecosystem, LibraryDiscovery, RegistryDiscovery};
pub use embed::{Embedder, LocalEmbedder};
pub use scrape::{scrape_docs, ScrapedPage};

use anyhow::Result;
use docbrain_graph::DocbrainStore;

/// Convenience wrapper used by production callers (`docbrain-mcp`): scrapes
/// `docs_url`, chunks/dedups/embeds/edges it, and stores it under
/// `(slug, version)`, using the real local embedding model
/// (`LocalEmbedder`, the `Embedder` port's production adapter).
pub fn scrape_chunk_and_store(store: &dyn DocbrainStore, slug: &str, version: &str, docs_url: &str, max_pages: usize) -> Result<Vec<ChunkOutcome>> {
    let pages = scrape_docs(docs_url, max_pages)?;
    chunk_and_store(store, slug, version, &pages, &LocalEmbedder)
}
