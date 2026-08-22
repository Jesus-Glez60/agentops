//! Wraps `candle-transformers`' Qwen2 implementation as a frozen base plus
//! `lora::LoraAdapter` residual on the hidden states, right before a frozen
//! `lm_head` (Initiative 6, CLS-inspired retrieval plan). The base model's
//! weights are loaded via `VarBuilder::from_mmaped_safetensors` -- plain
//! `Tensor`s, not `Var`s, so candle's autograd never wastes time computing
//! gradients for them; only the adapter's own small `VarMap` is trainable.

use anyhow::{Context, Result};
use candle_core::{DType, Device, IndexOp, Tensor};
use candle_nn::{Linear, VarBuilder, VarMap};
use candle_transformers::models::qwen2;

use crate::download::ModelFiles;
use crate::lora::LoraAdapter;

/// LoRA bottleneck rank -- deliberately small; this adapts a single
/// residual point (hidden states pre-`lm_head`), not every transformer
/// layer, so it doesn't need canonical LoRA's typical per-layer rank.
pub const LORA_RANK: usize = 16;
pub const LORA_ALPHA: f64 = 32.0;

pub struct AdaptedModel {
    base: qwen2::Model,
    lm_head: Linear,
    adapter: LoraAdapter,
    adapter_varmap: VarMap,
    device: Device,
    pub vocab_size: usize,
}

impl AdaptedModel {
    /// Loads the frozen base + a *fresh* (untrained, exact-no-op) adapter.
    pub fn load(files: &ModelFiles, device: &Device) -> Result<Self> {
        let config: qwen2::Config = serde_json::from_str(&std::fs::read_to_string(&files.config).context("reading config.json")?).context("parsing config.json")?;

        // SAFETY: standard candle idiom for loading a read-only, immutable
        // pretrained checkpoint -- the file is mmapped and never mutated
        // for the lifetime of this process, matching every candle
        // inference example's own usage of this same call.
        let base_vb = unsafe { VarBuilder::from_mmaped_safetensors(&[&files.weights], DType::F32, device) }.context("loading base model weights")?;

        let base = qwen2::Model::new(&config, base_vb.clone()).context("constructing the Qwen2 base model")?;

        let lm_head = if config.tie_word_embeddings {
            // Qwen2.5-Coder-0.5B-Instruct ties its output projection to
            // the input embedding matrix (config.json's own
            // `tie_word_embeddings: true`) -- `qwen2::Model`'s
            // `embed_tokens` field is private, so rather than widen that
            // crate's own API, the tied weight is loaded independently
            // from the same safetensors file at its known path
            // (`model.embed_tokens.weight`), exactly mirroring
            // `qwen2::ModelForCausalLM::new`'s own tied-embeddings branch.
            let embed_weight = base_vb.pp("model").pp("embed_tokens").get((config.vocab_size, config.hidden_size), "weight").context("loading tied embed_tokens weight for the lm_head")?;
            Linear::new(embed_weight, None)
        } else {
            let weight = base_vb.pp("lm_head").get((config.vocab_size, config.hidden_size), "weight").context("loading lm_head weight")?;
            Linear::new(weight, None)
        };

        let adapter_varmap = VarMap::new();
        let adapter_vb = VarBuilder::from_varmap(&adapter_varmap, DType::F32, device);
        let adapter = LoraAdapter::new(adapter_vb, config.hidden_size, LORA_RANK, LORA_ALPHA)?;

        Ok(Self { base, lm_head, adapter, adapter_varmap, device: device.clone(), vocab_size: config.vocab_size })
    }

    pub fn device(&self) -> &Device {
        &self.device
    }

    pub fn adapter_varmap(&self) -> &VarMap {
        &self.adapter_varmap
    }

    /// Mutable access, needed only to load a saved checkpoint's weights
    /// into this model's already-registered adapter parameters
    /// (`VarMap::load` takes `&mut self`) -- everything else about this
    /// type is used through the immutable accessor above.
    pub fn adapter_varmap_mut(&mut self) -> &mut VarMap {
        &mut self.adapter_varmap
    }

    /// Full-sequence forward pass (no KV cache reliance) -- returns logits
    /// over every position, `(batch, seq_len, vocab_size)`. Used for
    /// training, where loss needs every target position's logits, not just
    /// the last one the way single-step generation does.
    ///
    /// Always clears the base model's KV cache first: `qwen2::Model`
    /// caches per `DecoderLayer` internally and is designed for
    /// incremental single-token decoding (`seqlen_offset` growing call to
    /// call); reusing a stale cache across an unrelated full-sequence
    /// call silently corrupts attention. Simpler and safer to always
    /// start clean given this call shape, rather than track offset state
    /// with all this initiative's other call sites.
    pub fn forward_all_positions(&mut self, input_ids: &Tensor) -> Result<Tensor> {
        self.base.clear_kv_cache();
        let hidden = self.base.forward(input_ids, 0, None)?;
        let adapted = self.adapter.forward(&hidden)?;
        Ok(adapted.apply(&self.lm_head)?)
    }

    /// Greedy generation: re-forwards the whole growing sequence each step
    /// (no incremental KV-cache reuse) -- O(n²) in sequence length, a
    /// deliberate simplicity-over-throughput choice for an explicit,
    /// infrequent consolidation-eval call site, not a hot inference path.
    /// Stops at `max_new_tokens` or `eos_token_id`.
    pub fn generate_greedy(&mut self, prompt_ids: &[u32], max_new_tokens: usize, eos_token_id: u32) -> Result<Vec<u32>> {
        let mut tokens = prompt_ids.to_vec();
        for _ in 0..max_new_tokens {
            let input = Tensor::from_slice(&tokens, (1, tokens.len()), &self.device)?;
            let logits = self.forward_all_positions(&input)?;
            let last = logits.i((0, tokens.len() - 1, ..))?;
            let next = last.argmax(0)?.to_scalar::<u32>()?;
            if next == eos_token_id {
                break;
            }
            tokens.push(next);
        }
        Ok(tokens[prompt_ids.len()..].to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::download::fetch_model_files;

    /// Live, network/model-touching smoke test -- loads the real
    /// checkpoint and runs one real forward pass, confirming the tied
    /// -embedding lm_head path and shapes are actually correct against
    /// the real model, not just plausible-looking code. `#[ignore]`d for
    /// the same reason `download`'s own live test is.
    #[test]
    #[ignore]
    fn load_and_forward_pass_produce_the_expected_logits_shape() {
        let files = fetch_model_files().unwrap();
        let device = Device::Cpu;
        let mut model = AdaptedModel::load(&files, &device).unwrap();

        let input = Tensor::from_slice(&[1u32, 2, 3, 4, 5], (1, 5), &device).unwrap();
        let logits = model.forward_all_positions(&input).unwrap();
        assert_eq!(logits.dims(), &[1, 5, model.vocab_size]);
    }
}
