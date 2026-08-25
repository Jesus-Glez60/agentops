//! Training loop for `LoraAdapter` (Initiative 6, CLS-inspired retrieval
//! plan): weighted next-token cross-entropy over each example's *target*
//! tokens only (the prompt portion is masked out of the loss -- standard
//! instruction-tuning practice), one example at a time rather than padded
//! batches, since example lengths vary widely and this keeps the
//! implementation simple and correct over throughput-optimal -- the same
//! "correctness over efficiency for an explicit, infrequent consolidation
//! pass" tradeoff `model::AdaptedModel::generate_greedy` already makes.

use anyhow::{Context, Result};
use candle_core::Tensor;
use candle_nn::{AdamW, Optimizer, ParamsAdamW};
use tokenizers::Tokenizer;

use crate::data::TrainingExample;
use crate::model::AdaptedModel;

#[derive(Debug, Clone)]
pub struct TrainConfig {
    pub epochs: usize,
    pub lr: f64,
    /// Examples (after tokenizing prompt+target together) longer than this
    /// are truncated from the end -- a defensive bound against an
    /// unexpectedly long pattern-completed context blowing up per-step
    /// cost on CPU, not something expected to trigger often given
    /// `data::generate_examples`'s own prompt sizes.
    pub max_seq_len: usize,
}

impl Default for TrainConfig {
    fn default() -> Self {
        Self { epochs: 3, lr: 1e-4, max_seq_len: 512 }
    }
}

/// One example's contribution to the loss, tokenized and split into
/// (predictive-logit-positions, target-token-labels) -- `None` if nothing
/// usable survived tokenization/truncation (e.g. the target got truncated
/// away entirely), in which case the example is silently skipped rather
/// than erroring the whole run over one bad example.
fn tokenize_example(tokenizer: &Tokenizer, example: &TrainingExample, max_seq_len: usize) -> Result<Option<(Vec<u32>, usize)>> {
    let prompt_ids = tokenizer.encode(example.prompt.as_str(), true).map_err(|e| anyhow::anyhow!("tokenizing prompt: {e}"))?.get_ids().to_vec();
    let full_text = format!("{}{}", example.prompt, example.target);
    let mut full_ids = tokenizer.encode(full_text.as_str(), true).map_err(|e| anyhow::anyhow!("tokenizing full example: {e}"))?.get_ids().to_vec();
    if full_ids.len() > max_seq_len {
        full_ids.truncate(max_seq_len);
    }

    let split = prompt_ids.len();
    if split == 0 || split >= full_ids.len() {
        return Ok(None);
    }
    Ok(Some((full_ids, split)))
}

/// Trains `model`'s adapter in place over `examples`, weighted by each
/// example's own salience (`TrainingExample::weight` -- `data::generate_
/// examples`' plasticity/curation-derived weight, applied as a direct loss
/// multiplier, the same convention Initiative 5's weighted triplet loss
/// already used). Only the adapter's own `VarMap` is ever handed to the
/// optimizer -- the base model and `lm_head` were loaded as plain
/// `Tensor`s (see `AdaptedModel::load`'s own doc comment), so they never
/// appear in `adapter_varmap().all_vars()` and are structurally
/// un-trainable regardless of what the loss graph touches.
pub fn train(model: &mut AdaptedModel, tokenizer: &Tokenizer, examples: &[TrainingExample], config: &TrainConfig) -> Result<()> {
    let params = ParamsAdamW { lr: config.lr, ..Default::default() };
    let mut opt = AdamW::new(model.adapter_varmap().all_vars(), params).context("constructing the adapter optimizer")?;

    for _epoch in 0..config.epochs {
        for example in examples {
            let Some((token_ids, split)) = tokenize_example(tokenizer, example, config.max_seq_len)? else { continue };
            let device = model.device().clone();

            let input = Tensor::from_slice(&token_ids, (1, token_ids.len()), &device)?;
            let logits = model.forward_all_positions(&input)?.squeeze(0)?; // (seq_len, vocab)

            // Position `i` predicts token `i + 1` -- the logits that
            // predict the target span `token_ids[split..]` live at
            // `[split - 1, len - 1)`.
            let pred_len = token_ids.len() - split;
            let pred_logits = logits.narrow(0, split - 1, pred_len)?.contiguous()?;
            let labels = Tensor::from_slice(&token_ids[split..], pred_len, &device)?;

            let loss = candle_nn::loss::cross_entropy(&pred_logits, &labels)?;
            let weighted_loss = (loss * example.weight)?;
            opt.backward_step(&weighted_loss)?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::download::fetch_model_files;
    use candle_core::Device;

    /// Live smoke test: one real training step against the real model must
    /// not error and must actually change the adapter's weights (proof
    /// gradients genuinely flowed back to it through the frozen base, not
    /// just that the loop ran).
    #[test]
    #[ignore]
    fn training_one_step_changes_the_adapters_weights() {
        let files = fetch_model_files().unwrap();
        let device = Device::Cpu;
        let mut model = AdaptedModel::load(&files, &device).unwrap();
        let tokenizer = Tokenizer::from_file(&files.tokenizer).unwrap();

        let before: Vec<f32> = model.adapter_varmap().all_vars()[0].as_tensor().flatten_all().unwrap().to_vec1().unwrap();

        let examples = vec![TrainingExample { prompt: "<|im_start|>user\nExplain foo<|im_end|>\n<|im_start|>assistant\n".into(), target: "foo does a thing.<|im_end|>".into(), weight: 1.0 }];
        train(&mut model, &tokenizer, &examples, &TrainConfig { epochs: 1, lr: 1e-2, max_seq_len: 512 }).unwrap();

        let after: Vec<f32> = model.adapter_varmap().all_vars()[0].as_tensor().flatten_all().unwrap().to_vec1().unwrap();
        assert_ne!(before, after, "at least one training step must move the adapter's weights away from their zero-initialized start");
    }
}
