//! The projection head trained on top of the frozen BGE-small embeddings
//! (Initiative 5, CLS-inspired retrieval plan) -- a small residual MLP,
//! not a fine-tune of the base embedding model itself. Keeping the shared
//! semantic backbone (`agentops_embeddings::LocalEmbedder`) completely
//! frozen and only training this thin adapter is what makes catastrophic
//! forgetting a non-issue here: the base encoder's general semantic
//! knowledge is architecturally untouched, only a fast-adapting residual
//! shift on top of it moves.

use agentops_embeddings::EMBEDDING_DIM;
use candle_core::{Device, Result, Tensor};
use candle_nn::{Linear, Module, VarBuilder};

/// Hidden width of the projection head's bottleneck -- small on purpose:
/// this is meant to learn a repo-specific *shift*, not a new embedding
/// space, and a model this size trains in seconds on CPU.
pub const HIDDEN_DIM: usize = 128;

pub struct ProjectionHead {
    linear1: Linear,
    linear2: Linear,
}

impl ProjectionHead {
    pub fn new(vs: VarBuilder) -> Result<Self> {
        let linear1 = candle_nn::linear(EMBEDDING_DIM, HIDDEN_DIM, vs.pp("linear1"))?;
        // `linear2` is zero-initialized (weight *and* bias), not candle's
        // default Kaiming/uniform random init -- the same "adapter starts
        // as a true no-op" convention LoRA itself uses (its `B` matrix is
        // zero-initialized so a freshly-attached adapter changes nothing
        // until it's actually trained). Without this, `forward`'s residual
        // delta at initialization is an arbitrary random direction, which
        // made a freshly-initialized head's output cosine-similarity to
        // its own input unpredictable rather than guaranteed ~1.0 --
        // confirmed as a real flaky-test cause (0.46 vs. an expected >0.5
        // threshold on one random seed) before this fix, not a hypothetical.
        let linear2 = Linear::new(vs.pp("linear2").get_with_hints((EMBEDDING_DIM, HIDDEN_DIM), "weight", candle_nn::init::ZERO)?, Some(vs.pp("linear2").get_with_hints(EMBEDDING_DIM, "bias", candle_nn::init::ZERO)?));
        Ok(Self { linear1, linear2 })
    }

    /// `output = normalize(x + mlp(x))` -- residual, not a replacement
    /// projection, so a repo with no meaningful plasticity signal yet (a
    /// freshly-initialized or barely-trained head) still returns something
    /// very close to the original embedding rather than a random one.
    /// Re-normalized to unit length afterward so cosine-similarity-based
    /// downstream ranking (the same convention `search_hybrid`/
    /// `pattern_complete` already assume for embeddings) stays valid.
    pub fn forward(&self, xs: &Tensor) -> Result<Tensor> {
        let delta = self.linear2.forward(&self.linear1.forward(xs)?.relu()?)?;
        let out = (xs + delta)?;
        normalize_rows(&out)
    }

    /// Applies the head to a single raw embedding -- the shape query-time
    /// re-ranking and evaluation both need, as opposed to `forward`'s
    /// batched-tensor training shape.
    pub fn apply_one(&self, device: &Device, embedding: &[f32]) -> Result<Vec<f32>> {
        let xs = Tensor::from_slice(embedding, (1, EMBEDDING_DIM), device)?;
        let ys = self.forward(&xs)?;
        ys.reshape(EMBEDDING_DIM)?.to_vec1::<f32>()
    }
}

/// L2-normalizes each row of a `(batch, dim)` tensor.
fn normalize_rows(xs: &Tensor) -> Result<Tensor> {
    let norm = xs.sqr()?.sum_keepdim(candle_core::D::Minus1)?.sqrt()?;
    xs.broadcast_div(&norm)
}

#[cfg(test)]
mod tests {
    use super::*;
    use candle_core::DType;
    use candle_nn::VarMap;

    #[test]
    fn forward_output_is_unit_normalized() {
        let device = Device::Cpu;
        let varmap = VarMap::new();
        let vb = VarBuilder::from_varmap(&varmap, DType::F32, &device);
        let head = ProjectionHead::new(vb).unwrap();

        let input: Vec<f32> = (0..EMBEDDING_DIM).map(|i| (i as f32) / EMBEDDING_DIM as f32).collect();
        let out = head.apply_one(&device, &input).unwrap();
        let norm: f32 = out.iter().map(|v| v * v).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-4, "expected unit norm, got {norm}");
    }

    /// `linear2` is zero-initialized (see `ProjectionHead::new`'s own
    /// comment), so a freshly-initialized head's residual delta is exactly
    /// zero and this must hold deterministically -- not "usually true,"
    /// which an earlier version of this test asserted (`cosine > 0.5`) and
    /// which flaked on at least one real random seed (observed 0.4597)
    /// before `linear2` was zero-initialized specifically to fix this.
    #[test]
    fn a_freshly_initialized_head_is_an_exact_no_op_on_a_unit_input() {
        let device = Device::Cpu;
        let varmap = VarMap::new();
        let vb = VarBuilder::from_varmap(&varmap, DType::F32, &device);
        let head = ProjectionHead::new(vb).unwrap();

        let mut input = vec![0f32; EMBEDDING_DIM];
        input[0] = 1.0; // already unit-normalized
        let out = head.apply_one(&device, &input).unwrap();
        let cosine: f32 = out.iter().zip(&input).map(|(a, b)| a * b).sum();
        assert!((cosine - 1.0).abs() < 1e-4, "a freshly-initialized head must be an exact no-op (zero-initialized linear2), got cosine {cosine}");
    }
}
