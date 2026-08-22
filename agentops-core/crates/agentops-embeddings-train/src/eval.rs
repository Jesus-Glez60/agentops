//! Eval-gated promotion (Initiative 5, CLS-inspired retrieval plan): a
//! newly-trained projection head is only promoted to active if it doesn't
//! regress recall@k on a held-out slice of the replay buffer, scored
//! against whichever projection (if any) is currently active.

use std::collections::HashMap;

use candle_core::Device;

use crate::model::ProjectionHead;
use crate::replay::ReplayExample;

/// How many nearest-by-cosine-similarity candidates count as a "hit" for
/// recall@k -- deliberately small since the held-out set itself is small
/// (a handful to a few hundred examples per consolidation run).
pub const RECALL_K: usize = 5;

/// Fraction of resolved examples held out for eval rather than trained on.
pub const HELD_OUT_FRACTION: f64 = 0.2;

/// Splits `examples` into `(train, held_out)` deterministically by index
/// (not randomly) -- reproducible across a training/eval pair within one
/// consolidation run, which matters more here than true randomness would.
pub fn split_held_out(examples: &[ReplayExample]) -> (Vec<ReplayExample>, Vec<ReplayExample>) {
    if examples.len() < 5 {
        // Too few to hold anything out meaningfully -- train on all of it,
        // eval against the same set (a weak signal, but better than an
        // empty held-out set making every promotion trivially "pass").
        return (examples.to_vec(), examples.to_vec());
    }
    let held_out_count = ((examples.len() as f64) * HELD_OUT_FRACTION).round().max(1.0) as usize;
    let split_at = examples.len() - held_out_count;
    (examples[..split_at].to_vec(), examples[split_at..].to_vec())
}

/// recall@`RECALL_K`: for each held-out `(anchor, positive)` pair, project
/// every candidate embedding in `pool` (anchors + positives + negatives
/// from the full example set, so there's a real "distractor" pool to rank
/// against, not just the two relevant vectors), rank by cosine similarity
/// to the projected anchor, and count it a hit if the positive lands in
/// the top `RECALL_K`. `projection: None` means "score using the raw,
/// unprojected embedding" -- the baseline a candidate head must beat.
pub fn recall_at_k(device: &Device, held_out: &[ReplayExample], pool: &HashMap<i64, Vec<f32>>, projection: Option<&ProjectionHead>) -> anyhow::Result<f64> {
    if held_out.is_empty() {
        return Ok(0.0);
    }

    let project = |raw: &[f32]| -> anyhow::Result<Vec<f32>> {
        match projection {
            Some(head) => Ok(head.apply_one(device, raw)?),
            None => Ok(raw.to_vec()),
        }
    };

    let mut projected_pool: HashMap<i64, Vec<f32>> = HashMap::with_capacity(pool.len());
    for (&id, raw) in pool {
        projected_pool.insert(id, project(raw)?);
    }

    let mut hits = 0usize;
    let mut scored = 0usize;
    for ex in held_out {
        let (Some(anchor_raw), Some(_)) = (pool.get(&ex.anchor_id), pool.get(&ex.positive_id)) else { continue };
        let anchor_projected = project(anchor_raw)?;

        let mut ranked: Vec<(i64, f32)> = projected_pool.iter().filter(|(&id, _)| id != ex.anchor_id).map(|(&id, v)| (id, cosine(&anchor_projected, v))).collect();
        ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        ranked.truncate(RECALL_K);

        scored += 1;
        if ranked.iter().any(|(id, _)| *id == ex.positive_id) {
            hits += 1;
        }
    }

    if scored == 0 {
        return Ok(0.0);
    }
    Ok(hits as f64 / scored as f64)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_held_out_keeps_a_nonempty_training_set() {
        let examples: Vec<ReplayExample> = (0..20).map(|i| ReplayExample { anchor_id: i, positive_id: i + 100, negative_id: i + 200, weight: 1.0 }).collect();
        let (train, held_out) = split_held_out(&examples);
        assert_eq!(train.len() + held_out.len(), 20);
        assert!(!held_out.is_empty());
        assert!(train.len() > held_out.len());
    }

    #[test]
    fn recall_at_k_scores_perfectly_when_the_positive_is_identical_to_the_anchor() {
        let device = Device::Cpu;
        let mut pool = HashMap::new();
        pool.insert(1, vec![1.0, 0.0, 0.0]);
        pool.insert(2, vec![1.0, 0.0, 0.0]); // identical to anchor -- must always rank #1
        pool.insert(3, vec![0.0, 1.0, 0.0]);
        pool.insert(4, vec![0.0, 0.0, 1.0]);

        let held_out = vec![ReplayExample { anchor_id: 1, positive_id: 2, negative_id: 3, weight: 1.0 }];
        let recall = recall_at_k(&device, &held_out, &pool, None).unwrap();
        assert_eq!(recall, 1.0);
    }

    #[test]
    fn recall_at_k_is_zero_when_the_positive_is_nowhere_close() {
        let device = Device::Cpu;
        let mut pool = HashMap::new();
        pool.insert(1, vec![1.0, 0.0, 0.0]);
        pool.insert(2, vec![-1.0, 0.0, 0.0]); // maximally dissimilar
        for i in 3..20 {
            pool.insert(i, vec![1.0 - (i as f32) * 0.01, 0.01, 0.0]);
        }

        let held_out = vec![ReplayExample { anchor_id: 1, positive_id: 2, negative_id: 3, weight: 1.0 }];
        let recall = recall_at_k(&device, &held_out, &pool, None).unwrap();
        assert_eq!(recall, 0.0);
    }
}
