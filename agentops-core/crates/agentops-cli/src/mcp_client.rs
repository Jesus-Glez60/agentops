//! A blocking client for one `tools/call` on the server's `/mcp` endpoint,
//! authenticated with the remote marker's API key. Shared by `connect`'s
//! repo registration and `import-memory`'s `add_note` calls.

use anyhow::{Context, Result};
use serde_json::Value;

/// Calls `tool` with `arguments` and returns its first text block. A
/// non-2xx status, a JSON-RPC error, or `isError` on the result is an
/// error carrying the server's message.
pub(crate) fn call_tool(server_url: &str, api_key: &str, tool: &str, arguments: Value) -> Result<String> {
    let mut response = ureq::post(format!("{}/mcp", server_url.trim_end_matches('/')))
        .header("Authorization", &format!("Bearer {api_key}"))
        .config()
        .http_status_as_error(false)
        .build()
        .send_json(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": tool, "arguments": arguments },
        }))
        .with_context(|| format!("calling POST /mcp ({tool})"))?;
    let status = response.status();
    if !status.is_success() {
        let body = response.body_mut().read_to_string().unwrap_or_default();
        anyhow::bail!("POST /mcp ({tool}) returned {status}: {body}");
    }
    let body: Value = response.body_mut().read_json().context("parsing /mcp response")?;
    if let Some(error) = body.get("error") {
        anyhow::bail!("{tool} failed: {error}");
    }
    let message = body.pointer("/result/content/0/text").and_then(|v| v.as_str()).unwrap_or("(no message returned)").to_string();
    if body.pointer("/result/isError").and_then(|v| v.as_bool()).unwrap_or(false) {
        anyhow::bail!("{tool} failed: {message}");
    }
    Ok(message)
}
