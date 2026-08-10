//! Cross-cutting: secret-redaction gate used by driving adapters and docgen
//! output, plus API-key generation/verification shared by REST servers
//! (`docbrain-api`, and `agentops-api` once it's built).
//!
//! Only `api_key` is implemented so far — added narrowly to unblock
//! `docbrain-api`'s auth gate; the secret-redaction gate itself is still
//! future work (Module 1 of the codebrain-side foundation rebuild).

pub mod api_key;
