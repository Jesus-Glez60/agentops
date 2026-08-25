//! Fetches the frozen base model's weights/tokenizer/config from the Hub
//! (Initiative 6, CLS-inspired retrieval plan). A one-time, explicit
//! network operation triggered by `consolidate_model`/`agentops consolidate
//! --model` -- never automatic, matching this codebase's existing
//! "opt-in, real cost, never run on a scan" convention for `explain_symbol`
//! and `with_embeddings`.

use std::path::PathBuf;

use anyhow::{Context, Result};

/// Qwen2.5-Coder-0.5B-Instruct -- the smallest Qwen2.5-Coder checkpoint,
/// chosen after confirming `candle-transformers::models::qwen2` supports
/// the architecture and that this size is realistically LoRA-trainable on
/// CPU/Metal in a single consolidation pass (the "short spike before
/// committing" the plan called for, done as a live feasibility check, not
/// assumed). A code-specialized model, not a general-purpose one, since
/// this repo's own curated Gotcha/Decision/Definition text is
/// overwhelmingly about code.
pub const MODEL_ID: &str = "Qwen/Qwen2.5-Coder-0.5B-Instruct";

pub struct ModelFiles {
    pub config: PathBuf,
    pub tokenizer: PathBuf,
    pub weights: PathBuf,
}

impl ModelFiles {
    /// `eos_token_id` -- needed by `generate_greedy`/`eval::evaluate` to
    /// know when to stop, but not one of `qwen2::Config`'s own fields (that
    /// struct only carries the architecture-shape fields the model
    /// constructor needs). Read directly from the same `config.json` via a
    /// minimal, separate deserialize target rather than widening
    /// `candle-transformers`' own `Config` type.
    pub fn eos_token_id(&self) -> Result<u32> {
        #[derive(serde::Deserialize)]
        struct Extras {
            eos_token_id: u32,
        }
        let extras: Extras = serde_json::from_str(&std::fs::read_to_string(&self.config).context("reading config.json for eos_token_id")?).context("parsing eos_token_id out of config.json")?;
        Ok(extras.eos_token_id)
    }
}

/// Downloads (or reuses the local Hub cache for) `MODEL_ID`'s three
/// required files. Blocking/synchronous -- this runs inside an already
/// explicit, already-blocking consolidation command, not a request path.
pub fn fetch_model_files() -> Result<ModelFiles> {
    let api = hf_hub::api::sync::Api::new().context("initializing the Hugging Face Hub API client")?;
    let repo = api.model(MODEL_ID.to_string());
    let config = repo.get("config.json").context("downloading config.json")?;
    let tokenizer = repo.get("tokenizer.json").context("downloading tokenizer.json")?;
    let weights = repo.get("model.safetensors").context("downloading model.safetensors")?;
    Ok(ModelFiles { config, tokenizer, weights })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Live, network-touching download -- deliberately not run by default
    /// CI/`cargo test` (see `#[ignore]`), since it fetches a real ~1GB
    /// model from the Hub. Run explicitly (`cargo test -- --ignored`) to
    /// warm the local Hub cache or verify connectivity/file names.
    #[test]
    #[ignore]
    fn fetch_model_files_downloads_the_real_checkpoint() {
        let files = fetch_model_files().unwrap();
        assert!(files.config.exists());
        assert!(files.tokenizer.exists());
        assert!(files.weights.exists());
        println!("config: {:?}\ntokenizer: {:?}\nweights: {:?}", files.config, files.tokenizer, files.weights);
    }
}
