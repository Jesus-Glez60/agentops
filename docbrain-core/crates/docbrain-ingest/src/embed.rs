//! Local embedding generation via `fastembed` — no external service, same
//! process as the rest of docbrain (see the plan's "one system, no
//! tiering" design). Model is loaded once per process and reused; the
//! Hugging Face download on first use is the intentional, documented
//! exception in `deny.toml`'s `wrappers`-scoped `reqwest` allowance.
//!
//! `Embedder` is the port; `LocalEmbedder` is the one adapter today. Use
//! cases (`chunk.rs`, `retrieve.rs`) depend on `&dyn Embedder`, never on
//! `fastembed` or `LocalEmbedder` directly — a remote/API-backed embedder
//! could be added later as a second adapter without touching them.

use std::sync::{Mutex, OnceLock};

use anyhow::{Context, Result};
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};

/// The embedding port.
pub trait Embedder {
    fn embed(&self, text: &str) -> Result<Vec<f32>>;
}

/// `fastembed`-backed adapter: BGE-small-en-v1.5 running locally, no
/// external service. Zero-sized — trivially constructible wherever it's
/// needed, since all real state lives in the process-wide model cache
/// below.
pub struct LocalEmbedder;

impl Embedder for LocalEmbedder {
    fn embed(&self, text: &str) -> Result<Vec<f32>> {
        embed_text(text)
    }
}

/// BGE-small-en-v1.5 — 384 dimensions, matching `docbrain_graph::EMBEDDING_DIM`.
/// Chosen for a light local footprint over a larger model like BGE-M3, since
/// this runs inline in the ingestion/query path with no dedicated GPU
/// assumed.
fn model() -> Result<&'static Mutex<TextEmbedding>> {
    static MODEL: OnceLock<Mutex<TextEmbedding>> = OnceLock::new();
    // Double-checked locking: `OnceLock::get_or_init` can't propagate a
    // `Result`, and a naive "check-then-init" race lets two threads both
    // see it uninitialized and call `TextEmbedding::try_new` concurrently —
    // both then race to write the same on-disk model cache file, and
    // hf-hub's own file lock fails outright on contention rather than
    // waiting (verified: this really happened under `cargo test`'s default
    // parallel threads). `INIT_LOCK` serializes the init section itself so
    // only one thread ever downloads/loads the model.
    static INIT_LOCK: Mutex<()> = Mutex::new(());

    if MODEL.get().is_none() {
        let _guard = INIT_LOCK.lock().expect("embedding model init lock poisoned");
        if MODEL.get().is_none() {
            let embedding = TextEmbedding::try_new(InitOptions::new(EmbeddingModel::BGESmallENV15))
                .context("initializing local embedding model (BGE-small-en-v1.5)")?;
            let _ = MODEL.set(Mutex::new(embedding));
        }
    }
    Ok(MODEL.get().expect("just initialized above"))
}

/// Embeds one piece of text, matching `docbrain_graph::EMBEDDING_DIM`. Not
/// `pub` — the port (`Embedder`) and its adapter (`LocalEmbedder`) are the
/// public surface; this is `LocalEmbedder`'s implementation detail.
fn embed_text(text: &str) -> Result<Vec<f32>> {
    let model = model()?;
    let mut guard = model.lock().expect("embedding model mutex poisoned");
    let mut embeddings = guard.embed(vec![text.to_string()], None).context("generating embedding")?;
    let embedding = embeddings.pop().ok_or_else(|| anyhow::anyhow!("fastembed returned no embedding for a non-empty input"))?;
    debug_assert_eq!(embedding.len(), docbrain_graph::EMBEDDING_DIM);
    Ok(embedding)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Real model load + inference (downloads on first run, then cached) —
    // this is the one thing `chunk.rs`'s tests deliberately fake out, so it
    // needs its own live check that the real path actually works end to end.
    #[test]
    fn embeds_real_text_at_the_expected_dimension() {
        let embedding = embed_text("Install the package first.").expect("embedding generation should succeed");
        assert_eq!(embedding.len(), docbrain_graph::EMBEDDING_DIM);
        assert!(embedding.iter().any(|&v| v != 0.0), "embedding should not be all zeros");
    }

    #[test]
    fn similar_sentences_embed_closer_than_unrelated_ones() {
        let a = embed_text("How do I configure the API key?").unwrap();
        let b = embed_text("Where do I set my API key in the configuration?").unwrap();
        let c = embed_text("The quarterly financial report is due Friday.").unwrap();

        let cosine = |x: &[f32], y: &[f32]| -> f32 {
            let dot: f32 = x.iter().zip(y).map(|(xi, yi)| xi * yi).sum();
            let na: f32 = x.iter().map(|v| v * v).sum::<f32>().sqrt();
            let nb: f32 = y.iter().map(|v| v * v).sum::<f32>().sqrt();
            dot / (na * nb)
        };

        assert!(cosine(&a, &b) > cosine(&a, &c), "semantically similar sentences should be closer than unrelated ones");
    }
}
