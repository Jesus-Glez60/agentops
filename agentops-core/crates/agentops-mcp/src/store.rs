//! Re-exported from `agentops-store-open` — moved there so
//! `agentops-heavy-mcp` (and any other driving adapter) can depend on the
//! same backend-selection factory directly, without a wrong-direction
//! dependency on this crate. Kept `pub use` here under the original
//! `agentops_mcp::store` path so no existing caller needs to change its
//! import.

pub use agentops_store_open::{describe_backend, open_shared_postgres_store, open_store, resolve_store, with_shared_postgres_store};
#[cfg(test)]
pub use agentops_store_open::test_support;
