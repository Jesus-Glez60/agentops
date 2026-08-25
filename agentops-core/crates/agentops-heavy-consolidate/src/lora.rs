//! A hand-rolled, minimal LoRA-style low-rank adapter (Initiative 6,
//! CLS-inspired retrieval plan). The plan originally named the
//! `candle-lora` crate (EricLBuehler/candle-lora) for this -- confirmed,
//! via a direct `crates.io` API query before writing any of this code,
//! that it was never actually published there (only ever existed as an
//! unpublished GitHub repo). Hand-rolling is the honest correction, not a
//! worse fallback: canonical LoRA (`y = xW + x(BA)·scale`, `B`
//! zero-initialized so the adapter starts as an exact no-op) is a small,
//! well-understood amount of code -- the same zero-init convention
//! `agentops-embeddings-train::ProjectionHead` already established for
//! Initiative 5, and for the same reason (a deterministic no-op at init,
//! not a "small random" approximation of one).

use candle_core::{Module, Result, Tensor};
use candle_nn::{Init, VarBuilder};

/// A single low-rank adapter applied as a residual on top of a frozen
/// `in_dim -> out_dim` transformation the caller already owns (here,
/// always the identity -- see `LoraAdapter::forward`'s own doc comment for
/// why this crate applies it to hidden states rather than wrapping
/// individual attention/MLP weight matrices the way canonical multi-layer
/// LoRA does).
pub struct LoraLayer {
    a: Tensor,
    b: Tensor,
    scale: f64,
}

impl LoraLayer {
    /// `rank` is the LoRA bottleneck width; `scale` is LoRA's usual
    /// `alpha / rank` scaling factor, applied to the low-rank update.
    pub fn new(vs: VarBuilder, in_dim: usize, out_dim: usize, rank: usize, alpha: f64) -> Result<Self> {
        // `a`: small-random Kaiming-style init (matches how LoRA's own
        // reference implementation initializes its A matrix). `b`:
        // zero-initialized, so `b.matmul(a.matmul(x))` is exactly zero at
        // init regardless of `a`'s random draw -- the adapter is a
        // deterministic no-op until trained.
        let a = vs.get_with_hints((rank, in_dim), "lora_a", candle_nn::init::DEFAULT_KAIMING_NORMAL)?;
        let b = vs.get_with_hints((out_dim, rank), "lora_b", Init::Const(0.))?;
        Ok(Self { a, b, scale: alpha / rank as f64 })
    }

    /// The low-rank update alone (`scale * B(A(x))`), *not* added to `x` --
    /// callers decide how to combine it (a residual add for a hidden
    /// -state adapter, as `LoraAdapter` below does).
    pub fn delta(&self, xs: &Tensor) -> Result<Tensor> {
        let low_rank = xs.broadcast_matmul(&self.a.t()?)?;
        let delta = low_rank.broadcast_matmul(&self.b.t()?)?;
        (delta * self.scale)?.contiguous()
    }
}

/// The trainable adapter this initiative actually trains: a single
/// `LoraLayer` applied as a residual on the causal LM's hidden states,
/// right before the frozen `lm_head` -- **not** inserted into every
/// attention/MLP projection the way canonical LoRA is. Stated plainly:
/// this is a real simplification versus textbook multi-layer LoRA, chosen
/// because (a) `candle-lora` doesn't exist to provide that wiring for
/// free, (b) hand-rolling LoRA inside every `Q`/`K`/`V`/`O`/gate/up/down
/// projection of a transformer means either forking
/// `candle-transformers::models::qwen2` or reimplementing its forward pass
/// entirely, both real, separate scope, and (c) a single hidden-state
/// -level adapter is architecturally the exact same "frozen base + small
/// trainable residual" shape Initiative 5's `ProjectionHead` already used
/// successfully for embeddings, applied consistently here rather than
/// introducing a second, different adaptation strategy. A genuine
/// multi-layer LoRA port is real follow-up work, not silently assumed
/// equivalent to this.
pub struct LoraAdapter {
    layer: LoraLayer,
}

impl LoraAdapter {
    pub fn new(vs: VarBuilder, hidden_size: usize, rank: usize, alpha: f64) -> Result<Self> {
        Ok(Self { layer: LoraLayer::new(vs, hidden_size, hidden_size, rank, alpha)? })
    }

    /// `hidden_states + delta` -- applied to every position in the
    /// sequence (not just the last token), since training needs logits
    /// over the full target span, unlike single-token-at-a-time
    /// generation.
    pub fn forward(&self, hidden_states: &Tensor) -> Result<Tensor> {
        hidden_states + self.layer.delta(hidden_states)?
    }
}

impl Module for LoraAdapter {
    fn forward(&self, xs: &Tensor) -> Result<Tensor> {
        self.forward(xs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use candle_core::{DType, Device};
    use candle_nn::VarMap;

    fn vb() -> (VarMap, VarBuilder<'static>) {
        let varmap = VarMap::new();
        let device = Device::Cpu;
        let vb = VarBuilder::from_varmap(&varmap, DType::F32, &device);
        (varmap, vb)
    }

    #[test]
    fn a_freshly_initialized_adapter_is_an_exact_no_op() {
        let (_varmap, vb) = vb();
        let adapter = LoraAdapter::new(vb, 8, 4, 8.0).unwrap();
        let xs = Tensor::from_slice(&[1f32, 2., 3., -1., 0.5, 2.5, -3., 4.], (1, 8), &Device::Cpu).unwrap();

        let out = adapter.forward(&xs).unwrap();
        let out_v = out.reshape(8).unwrap().to_vec1::<f32>().unwrap();
        let in_v = xs.reshape(8).unwrap().to_vec1::<f32>().unwrap();
        for (o, i) in out_v.iter().zip(&in_v) {
            assert!((o - i).abs() < 1e-6, "expected an exact no-op at init: {out_v:?} vs {in_v:?}");
        }
    }

    #[test]
    fn delta_shape_matches_input_shape_for_a_batch() {
        let (_varmap, vb) = vb();
        let adapter = LoraAdapter::new(vb, 8, 4, 8.0).unwrap();
        let xs = Tensor::zeros((2, 5, 8), DType::F32, &Device::Cpu).unwrap();
        let out = adapter.forward(&xs).unwrap();
        assert_eq!(out.dims(), &[2, 5, 8]);
    }
}
