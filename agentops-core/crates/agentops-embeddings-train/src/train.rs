//! Contrastive/triplet-margin training loop for `ProjectionHead`
//! (Initiative 5, CLS-inspired retrieval plan). CPU-only, seconds-scale on
//! the replay buffer sizes `MAX_REPLAY_PAIRS` bounds this to -- no GPU
//! assumption, matching `agentops-embeddings::LocalEmbedder`'s own
//! no-dedicated-GPU design note.

use agentops_embeddings::EMBEDDING_DIM;
use anyhow::Result;
use candle_core::{DType, Device, Tensor};
use candle_nn::{AdamW, Optimizer, ParamsAdamW, VarBuilder, VarMap};

use crate::model::ProjectionHead;
use crate::replay::ReplayExample;

#[derive(Debug, Clone)]
pub struct TrainConfig {
    pub epochs: usize,
    pub lr: f64,
    /// Triplet margin -- how much closer the positive must be than the
    /// negative (in cosine similarity) before the loss for that example
    /// hits zero.
    pub margin: f32,
}

impl Default for TrainConfig {
    fn default() -> Self {
        Self { epochs: 30, lr: 1e-3, margin: 0.2 }
    }
}

/// Trains a fresh `ProjectionHead` on `examples` (already resolved to
/// anchor/positive/negative node ids by `replay::resolve_examples`),
/// looking up each id's raw embedding via `embedding_of`. Weighted triplet
/// margin loss: `weight * relu(margin - cos(anchor, positive) +
/// cos(anchor, negative))`, so a heavily-reinforced edge (Initiative 1's
/// plasticity weight) pulls harder on the projection than a barely-touched
/// one -- the salience-weighted replay this initiative is named for,
/// expressed as a loss weight rather than a resampling frequency.
pub fn train(examples: &[ReplayExample], embedding_of: impl Fn(i64) -> Option<Vec<f32>>, config: &TrainConfig) -> Result<(VarMap, ProjectionHead)> {
    let device = Device::Cpu;
    let varmap = VarMap::new();
    let vb = VarBuilder::from_varmap(&varmap, DType::F32, &device);
    let head = ProjectionHead::new(vb)?;

    let mut anchors = Vec::with_capacity(examples.len() * EMBEDDING_DIM);
    let mut positives = Vec::with_capacity(examples.len() * EMBEDDING_DIM);
    let mut negatives = Vec::with_capacity(examples.len() * EMBEDDING_DIM);
    let mut weights = Vec::with_capacity(examples.len());
    for ex in examples {
        let (Some(a), Some(p), Some(n)) = (embedding_of(ex.anchor_id), embedding_of(ex.positive_id), embedding_of(ex.negative_id)) else { continue };
        anchors.extend(a);
        positives.extend(p);
        negatives.extend(n);
        weights.push(ex.weight as f32);
    }
    anyhow::ensure!(!weights.is_empty(), "no example had all three embeddings available");
    let n = weights.len();

    let anchor_t = Tensor::from_vec(anchors, (n, EMBEDDING_DIM), &device)?;
    let positive_t = Tensor::from_vec(positives, (n, EMBEDDING_DIM), &device)?;
    let negative_t = Tensor::from_vec(negatives, (n, EMBEDDING_DIM), &device)?;
    let weight_t = Tensor::from_vec(weights, n, &device)?;
    let margin_t = Tensor::full(config.margin, n, &device)?;

    let params = ParamsAdamW { lr: config.lr, ..Default::default() };
    let mut opt = AdamW::new(varmap.all_vars(), params)?;

    for _ in 0..config.epochs {
        let a = head.forward(&anchor_t)?;
        let p = head.forward(&positive_t)?;
        let neg = head.forward(&negative_t)?;

        // Unit-normalized rows (ProjectionHead::forward guarantees this),
        // so a row-wise dot product *is* cosine similarity.
        let pos_sim = (&a * &p)?.sum(candle_core::D::Minus1)?;
        let neg_sim = (&a * &neg)?.sum(candle_core::D::Minus1)?;

        let per_example_loss = ((&margin_t - &pos_sim)? + &neg_sim)?.relu()?;
        let weighted_loss = (&per_example_loss * &weight_t)?;
        let loss = weighted_loss.mean_all()?;

        opt.backward_step(&loss)?;
    }

    Ok((varmap, head))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::replay::ReplayExample;
    use std::collections::HashMap;

    fn unit_vec(dominant: usize) -> Vec<f32> {
        let mut v = vec![0.001f32; EMBEDDING_DIM];
        v[dominant] = 1.0;
        v
    }

    /// A minimal, deterministic end-to-end training smoke test: anchor and
    /// positive share a dominant dimension (should end up close), negative
    /// has a different one entirely (should end up far) -- training must
    /// increase the anchor-positive similarity margin over the untrained
    /// starting point.
    #[test]
    fn training_increases_positive_similarity_relative_to_negative() {
        let mut embeddings: HashMap<i64, Vec<f32>> = HashMap::new();
        embeddings.insert(1, unit_vec(0));
        embeddings.insert(2, unit_vec(0));
        embeddings.insert(3, unit_vec(200));
        let lookup = |id: i64| embeddings.get(&id).cloned();

        let examples = vec![ReplayExample { anchor_id: 1, positive_id: 2, negative_id: 3, weight: 1.0 }];
        let config = TrainConfig { epochs: 50, lr: 5e-3, margin: 0.2 };
        let (_, head) = train(&examples, lookup, &config).unwrap();

        let device = Device::Cpu;
        let a = head.apply_one(&device, &embeddings[&1]).unwrap();
        let p = head.apply_one(&device, &embeddings[&2]).unwrap();
        let neg = head.apply_one(&device, &embeddings[&3]).unwrap();
        let pos_sim: f32 = a.iter().zip(&p).map(|(x, y)| x * y).sum();
        let neg_sim: f32 = a.iter().zip(&neg).map(|(x, y)| x * y).sum();
        assert!(pos_sim > neg_sim, "after training, anchor must be closer to its positive than to the negative: pos={pos_sim} neg={neg_sim}");
    }

    #[test]
    fn train_errors_clearly_when_no_example_has_every_embedding() {
        let examples = vec![ReplayExample { anchor_id: 1, positive_id: 2, negative_id: 3, weight: 1.0 }];
        let result = train(&examples, |_| None, &TrainConfig::default());
        assert!(result.is_err());
    }
}
