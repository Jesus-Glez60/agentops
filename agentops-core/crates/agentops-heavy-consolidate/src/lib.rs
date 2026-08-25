//! Local LoRA-flavored code-explanation consolidation (Initiative 6,
//! CLS-inspired retrieval plan). See `agentops-embeddings-train` (Initiative
//! 5) for the sibling embedding-consolidation crate this one deliberately
//! mirrors in shape (frozen base + small trainable adapter, replay-buffer
//! -driven, eval-gated promotion) while adapting it to a causal LM instead
//! of an embedder.

mod data;
mod download;
mod eval;
mod lora;
mod model;
mod store;
mod train;

pub use data::TrainingExample;
pub use model::AdaptedModel;
pub use store::AdapterStore;

use agentops_embeddings::LocalEmbedder;
use agentops_graph::GraphStore;
use anyhow::Result;
use candle_core::Device;
use tokenizers::Tokenizer;

/// Below this many curated training examples, consolidation isn't run at
/// all -- lower than Initiative 5's `MIN_REPLAY_PAIRS` (10) since curated
/// Gotcha/Decision text is inherently scarcer than plasticity-bearing
/// edges (this is real, human-reviewed content, not every graph edge).
pub const MIN_EXAMPLES: usize = 5;
/// Hard ceiling on examples trained per run -- each one costs a real
/// forward+backward pass through a 0.5B-parameter model on CPU, so this
/// bounds worst-case wall-clock cost the same way
/// `agentops-embeddings-train::replay::MAX_REPLAY_PAIRS` bounds Initiative
/// 5's own per-run cost.
pub const MAX_EXAMPLES: usize = 30;

#[derive(Debug, Clone)]
pub struct ModelConsolidationReport {
    pub attempted: bool,
    pub promoted: bool,
    pub examples_used: usize,
    pub candidate_score: f64,
    pub baseline_score: f64,
    pub promoted_version: Option<u32>,
    pub reason: String,
}

/// Runs one full Initiative 6 consolidation pass for `repo`: generates
/// training examples from curated `Gotcha`/`Decision` notes
/// (`data::generate_examples`), downloads/loads the frozen base model
/// (`download`, `model::AdaptedModel`), trains a *fresh* (zero-initialized,
/// not continued from whatever's currently active -- same convention
/// Initiative 5's replay-driven retraining already established) LoRA
/// adapter on the majority split, evaluates both the fresh candidate and
/// whatever's currently active against the held-out split, and promotes
/// the candidate only if it doesn't regress. Skips gracefully (never
/// errors) if there isn't enough curated signal yet.
pub fn consolidate_model(store: &dyn GraphStore, repo: &str) -> Result<ModelConsolidationReport> {
    let skip = |reason: String| ModelConsolidationReport { attempted: false, promoted: false, examples_used: 0, candidate_score: 0.0, baseline_score: 0.0, promoted_version: None, reason };

    let examples = data::generate_examples(store, &LocalEmbedder, repo, MAX_EXAMPLES)?;
    if examples.len() < MIN_EXAMPLES {
        return Ok(skip(format!("only {} curated Gotcha/Decision example(s) with a connected symbol, need at least {MIN_EXAMPLES}", examples.len())));
    }

    let (train_examples, held_out) = eval::split_held_out(&examples);

    let files = download::fetch_model_files()?;
    let tokenizer = Tokenizer::from_file(&files.tokenizer).map_err(|e| anyhow::anyhow!("loading tokenizer: {e}"))?;
    let eos_token_id = files.eos_token_id()?;
    let device = Device::Cpu;
    let adapter_store = store::AdapterStore::open(repo)?;

    // Baseline: whatever's currently active (or the untrained, exact-no-op
    // adapter if nothing's been promoted yet -- the same "baseline is raw/
    // unadapted on a first run" shape Initiative 5's own eval gate uses).
    let mut baseline_model = model::AdaptedModel::load(&files, &device)?;
    adapter_store.load_active_into(baseline_model.adapter_varmap_mut())?;
    let baseline_score = eval::evaluate(&mut baseline_model, &tokenizer, &LocalEmbedder, &held_out, eos_token_id, eval::DEFAULT_MAX_NEW_TOKENS)?;
    drop(baseline_model);

    // Candidate: a separate, fresh model instance (zero-initialized
    // adapter), trained on the majority split.
    let mut candidate_model = model::AdaptedModel::load(&files, &device)?;
    train::train(&mut candidate_model, &tokenizer, &train_examples, &train::TrainConfig::default())?;
    let candidate_score = eval::evaluate(&mut candidate_model, &tokenizer, &LocalEmbedder, &held_out, eos_token_id, eval::DEFAULT_MAX_NEW_TOKENS)?;

    let version = adapter_store.save_new_version(candidate_model.adapter_varmap())?;
    let promoted = candidate_score >= baseline_score;
    if promoted {
        adapter_store.promote(version)?;
    }

    Ok(ModelConsolidationReport {
        attempted: true,
        promoted,
        examples_used: examples.len(),
        candidate_score,
        baseline_score,
        promoted_version: promoted.then_some(version),
        reason: if promoted {
            format!("promoted v{version}: candidate score {candidate_score:.3} >= baseline {baseline_score:.3}")
        } else {
            format!("kept previous version: candidate score {candidate_score:.3} regressed vs. baseline {baseline_score:.3}")
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consolidate_model_skips_gracefully_with_too_few_examples() {
        let store = agentops_graph::SqliteGraphStore::open_in_memory().unwrap();
        let report = consolidate_model(&store, "test-heavy-consolidate-empty").unwrap();
        assert!(!report.attempted);
        assert!(!report.promoted);
    }
}
