//! `GET /connect.sh` -- Initiative 2's one-command remote connect:
//! `curl -fsSL <server>/connect.sh | sh` installs the `agentops` CLI (if
//! missing) and runs `agentops connect --remote ... --agents ... --yes`
//! against *this* server, with the right `--remote` URL and tool selection
//! already filled in. Deliberately unauthenticated (no tenant/session
//! logic) -- it's a small, mostly-static script, the same public-
//! fetchability posture as `install.sh` on GitHub; the real secret (the
//! user's `AGENTOPS_API_KEY`) is read from the caller's own shell
//! environment at run time, never embedded in this script's text.
//!
//! **Self-referential URL, no `AGENTOPS_PUBLIC_API_URL` needed here**: the
//! handler derives `--remote` from the *incoming request's own*
//! `Host`/`X-Forwarded-*` headers, the same derivation
//! `apps/web/src/lib/server/public-api-url.ts` uses for the onboarding
//! page's copy-paste command. Since the user is fetching this script from
//! that exact URL, it's definitionally reachable -- no separate guess-or-
//! configure step needed, unlike the web onboarding page (which renders
//! server-side, with no request of its own to introspect the API's URL
//! from, hence needing the env var/guess fallback there).
//!
//! **curl-pipe-to-shell safety** (2026 guidance from kicksecure.com/wiki,
//! arp242.net, bettercli.org, and the in-toto/witness and
//! elastic/docs-builder installer scripts): the whole script body is
//! wrapped in a function, called only on the last line -- a network blip
//! truncating the download leaves an incomplete function definition, which
//! fails to parse and exits before ever reaching a call it never got to,
//! rather than executing a syntactically-valid-but-truncated command
//! prefix. `GET /connect.sh` viewed directly in a browser (not piped) is
//! this script's own "preview before you pipe" story, same as
//! `install.sh` on GitHub -- no separate preview route needed.

use axum::extract::Query;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde::Deserialize;

/// Matches `agentops-cli`'s own `select_agents` interactive defaults
/// (`named = ["claude", "cursor", "codex", "gemini-cli"]`,
/// `defaults = [true, true, false, false]`) -- so a bare
/// `curl .../connect.sh | sh` with no `?agents=` still does something
/// reasonable rather than connecting zero tools.
const DEFAULT_AGENTS: &str = "claude,cursor";

const INSTALL_SCRIPT_URL: &str = "https://raw.githubusercontent.com/Jesus-Glez60/agentops/main/install.sh";

#[derive(Debug, Deserialize)]
pub(crate) struct ConnectScriptQuery {
    #[serde(default)]
    agents: Option<String>,
}

/// `Host`/`X-Forwarded-Host`/`X-Forwarded-Proto` -- same header set and
/// precedence `apps/web/src/lib/server/public-api-url.ts` reads, so the two
/// URL-derivation stories (the web onboarding page's copy-paste command,
/// and this route's self-generated one) stay consistent with each other.
fn derive_remote_url(headers: &HeaderMap) -> String {
    let proto = headers.get("x-forwarded-proto").and_then(|v| v.to_str().ok()).unwrap_or("http");
    let host = headers
        .get("x-forwarded-host")
        .or_else(|| headers.get(header::HOST))
        .and_then(|v| v.to_str().ok())
        .unwrap_or("localhost:8420");
    format!("{proto}://{host}")
}

fn is_known_agent(agent: &str) -> bool {
    matches!(agent, "claude" | "cursor" | "codex" | "gemini-cli")
}

async fn connect_script(headers: HeaderMap, Query(q): Query<ConnectScriptQuery>) -> Response {
    let remote_url = derive_remote_url(&headers);
    let requested = q.agents.as_deref().unwrap_or(DEFAULT_AGENTS);
    let agents: Vec<&str> = requested.split(',').map(str::trim).filter(|a| !a.is_empty()).collect();
    if agents.is_empty() || !agents.iter().all(|a| is_known_agent(a)) {
        return (StatusCode::BAD_REQUEST, format!("invalid 'agents' -- must be a comma-separated list from claude,cursor,codex,gemini-cli, got {requested:?}")).into_response();
    }
    let agents_csv = agents.join(",");

    let script = format!(
        r#"#!/usr/bin/env bash
set -euo pipefail
_agentops_connect() {{
  if ! command -v agentops >/dev/null 2>&1; then
    echo "agentops CLI not found -- installing..."
    # Prefer building from a local agentops source checkout over the
    # public GitHub download -- walks up from the current directory
    # looking for agentops-core/crates/agentops-cli, the same monorepo
    # layout this server itself is built from. Covers a self-hosted
    # deployment whose source repo isn't publicly readable (install.sh's
    # GitHub download 404s for anyone against a private repo, not just a
    # misconfiguration), and is strictly faster/more trustworthy than a
    # network download when the exact source is already sitting right
    # there on disk.
    _agentops_src=""
    _dir="$PWD"
    while [ "$_dir" != "/" ]; do
      if [ -f "$_dir/agentops-core/crates/agentops-cli/Cargo.toml" ]; then
        _agentops_src="$_dir"
        break
      fi
      _dir="$(dirname "$_dir")"
    done
    if [ -n "$_agentops_src" ]; then
      echo "Found a local agentops source checkout at $_agentops_src -- building from source (requires cargo)..."
      (cd "$_agentops_src" && cargo install --path agentops-core/crates/agentops-cli --locked)
    else
      curl -fsSL {INSTALL_SCRIPT_URL} | sh
      export PATH="$HOME/.agentops/bin:$PATH"
    fi
  fi
  if [ -z "${{AGENTOPS_API_KEY:-}}" ]; then
    echo "error: set AGENTOPS_API_KEY first (Profile -> Connect a coding tool, or the onboarding checklist's \"Generate API key\")" >&2
    return 1
  fi
  agentops connect --remote "{remote_url}" --api-key "$AGENTOPS_API_KEY" --agents "{agents_csv}" --yes
}}
_agentops_connect
"#
    );

    // `text/x-shellscript` (not `text/plain`) so a direct browser visit
    // downloads/displays it as a script -- `curl -fsSL | sh` doesn't care
    // about content-type either way.
    (StatusCode::OK, [(header::CONTENT_TYPE, "text/x-shellscript; charset=utf-8")], script).into_response()
}

/// Standalone router for this one route -- merged into `agentops-server`'s
/// composed app alongside `/health`, no shared state needed.
pub(crate) fn router() -> Router {
    Router::new().route("/connect.sh", get(connect_script))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn body_text(response: Response) -> String {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn default_agents_is_claude_and_cursor_when_omitted() {
        let app = router();
        let resp = app.oneshot(Request::builder().uri("/connect.sh").header("host", "example.com:8420").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_text(resp).await;
        assert!(body.contains(r#"--agents "claude,cursor""#), "{body}");
    }

    #[tokio::test]
    async fn agents_query_param_is_templated_through() {
        let app = router();
        let resp = app.oneshot(Request::builder().uri("/connect.sh?agents=claude,codex").header("host", "example.com:8420").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_text(resp).await;
        assert!(body.contains(r#"--agents "claude,codex""#), "{body}");
    }

    #[tokio::test]
    async fn an_unknown_agent_400s() {
        let app = router();
        let resp = app.oneshot(Request::builder().uri("/connect.sh?agents=claude,not-a-real-agent").header("host", "example.com:8420").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn remote_url_is_derived_from_the_requests_own_host_header() {
        let app = router();
        let resp = app.oneshot(Request::builder().uri("/connect.sh").header("host", "my-agentops-server:18420").body(Body::empty()).unwrap()).await.unwrap();
        let body = body_text(resp).await;
        assert!(body.contains(r#"--remote "http://my-agentops-server:18420""#), "{body}");
    }

    #[tokio::test]
    async fn x_forwarded_proto_and_host_take_precedence_over_the_host_header() {
        let app = router();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/connect.sh")
                    .header("host", "internal-service:8420")
                    .header("x-forwarded-proto", "https")
                    .header("x-forwarded-host", "agentops.example.com")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_text(resp).await;
        assert!(body.contains(r#"--remote "https://agentops.example.com""#), "{body}");
    }

    #[tokio::test]
    async fn the_script_is_function_wrapped_and_a_truncated_download_fails_to_parse() {
        let app = router();
        let resp = app.oneshot(Request::builder().uri("/connect.sh").header("host", "example.com").body(Body::empty()).unwrap()).await.unwrap();
        let body = body_text(resp).await;
        // Truncate mid-function -- an incomplete `_agentops_connect() { ...`
        // body is a shell syntax error (unbalanced brace/quotes), which
        // `sh -n` (parse-only, no execution) must reject rather than
        // silently accept a partial command prefix.
        let truncated = &body[..body.len() / 2];
        let check = std::process::Command::new("sh").arg("-n").arg("-c").arg(truncated).output().unwrap();
        assert!(!check.status.success(), "a truncated download must fail to parse, not partially execute: {truncated}");
    }

    #[tokio::test]
    async fn the_script_checks_for_a_local_source_checkout_before_downloading_from_github() {
        let app = router();
        let resp = app.oneshot(Request::builder().uri("/connect.sh").header("host", "example.com").body(Body::empty()).unwrap()).await.unwrap();
        let body = body_text(resp).await;
        assert!(body.contains("agentops-core/crates/agentops-cli/Cargo.toml"), "{body}");
        assert!(body.contains("cargo install --path agentops-core/crates/agentops-cli"), "{body}");
        let check = std::process::Command::new("sh").arg("-n").arg("-c").arg(&body).output().unwrap();
        assert!(check.status.success(), "the full (untruncated) script must be valid shell: {}", String::from_utf8_lossy(&check.stderr));
    }
}
