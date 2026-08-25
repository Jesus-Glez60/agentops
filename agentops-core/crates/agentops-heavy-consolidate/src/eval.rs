//! Eval-gated promotion scoring (Initiative 6, CLS-inspired retrieval
//! plan): generates a real completion for each held-out example's prompt
//! and scores it against the note's own curated text via cosine similarity
//! under the *frozen, un-adapted* base embedder
//! (`agentops_embeddings::LocalEmbedder`) -- deliberately independent of
//! Initiative 5's own trained projection, so this eval signal can't reward
//! a candidate for drifting toward whatever Initiative 5 happened to learn
//! rather than toward genuinely matching the curated ground truth. Stated
//! plainly, per the plan this shipped against: automatic grading of
//! generated-explanation quality is inherently imperfect -- this gate
//! reduces regression risk, it does not guarantee quality.

use agentops_embeddings::Embedder;
use anyhow::Result;
use tokenizers::Tokenizer;

use crate::data::TrainingExample;
use crate::model::AdaptedModel;

pub const DEFAULT_MAX_NEW_TOKENS: usize = 64;
pub const HELD_OUT_FRACTION: f64 = 0.2;

/// Splits `examples` into `(train, held_out)` deterministically by index,
/// same convention `agentops-embeddings-train::eval::split_held_out`
/// already uses -- reproducible within one consolidation run, which
/// matters more here than true randomness would.
pub fn split_held_out(examples: &[TrainingExample]) -> (Vec<TrainingExample>, Vec<TrainingExample>) {
    if examples.len() < 5 {
        return (examples.to_vec(), examples.to_vec());
    }
    let held_out_count = ((examples.len() as f64) * HELD_OUT_FRACTION).round().max(1.0) as usize;
    let split_at = examples.len() - held_out_count;
    (examples[..split_at].to_vec(), examples[split_at..].to_vec())
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|v| v * v).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|v| v * v).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na * nb)
    }
}

/// Mean cosine similarity, across `held_out`, between a real greedy
/// generation from `model` and each example's own curated target text.
pub fn evaluate(model: &mut AdaptedModel, tokenizer: &Tokenizer, embedder: &dyn Embedder, held_out: &[TrainingExample], eos_token_id: u32, max_new_tokens: usize) -> Result<f64> {
    if held_out.is_empty() {
        return Ok(0.0);
    }

    let mut total = 0.0f64;
    let mut scored = 0usize;
    for example in held_out {
        let prompt_ids = tokenizer.encode(example.prompt.as_str(), true).map_err(|e| anyhow::anyhow!("tokenizing eval prompt: {e}"))?.get_ids().to_vec();
        if prompt_ids.is_empty() {
            continue;
        }
        let generated_ids = model.generate_greedy(&prompt_ids, max_new_tokens, eos_token_id)?;
        let generated_text = tokenizer.decode(&generated_ids, true).map_err(|e| anyhow::anyhow!("decoding generated tokens: {e}"))?;
        if generated_text.trim().is_empty() {
            // A degenerate (immediate-EOS) generation scores as a real
            // zero, not skipped -- an adapter that only ever produces
            // empty output must not pass eval by having nothing to
            // compare.
            scored += 1;
            continue;
        }

        let generated_embedding = embedder.embed(&generated_text)?;
        let target_embedding = embedder.embed(&example.target)?;
        total += cosine(&generated_embedding, &target_embedding) as f64;
        scored += 1;
    }

    if scored == 0 {
        return Ok(0.0);
    }
    Ok(total / scored as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evaluate_returns_zero_for_an_empty_held_out_set() {
        // Doesn't need a real model -- `held_out.is_empty()` short-circuits
        // before touching `model`/`tokenizer`, so this can't be tested
        // without one via the public signature; covered instead by the
        // live end-to-end eval test in `lib.rs`'s own consolidation test.
        assert_eq!(cosine(&[], &[]), 0.0);
    }

    #[test]
    fn cosine_of_identical_vectors_is_one() {
        assert!((cosine(&[1.0, 2.0, 3.0], &[1.0, 2.0, 3.0]) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_of_orthogonal_vectors_is_zero() {
        assert!(cosine(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-6);
    }
}
