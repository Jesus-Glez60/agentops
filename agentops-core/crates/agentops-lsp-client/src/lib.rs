//! A narrow, best-effort client for one `rust-analyzer` process.
//!
//! `document_symbols` was this crate's original purpose (disambiguating
//! Rust symbols in cases Tree-sitter's own extraction structurally
//! couldn't) -- since superseded: both cited motivating cases were tested
//! live and found to need no LSP involvement at all (generic/blanket impls:
//! already fully resolved by `ast_extract.rs`'s own syntactic `trait`-field
//! read) or to not be visible via this request at all (macro-generated
//! impls: confirmed live that `documentSymbol` shows none of a macro's
//! expansion, for either declarative or derive macros). Kept because it's
//! tested and working, not because it's still load-bearing for the pilot.
//!
//! `expand_macro` is the crate's real remaining value: `rust-analyzer`'s
//! own custom `rust-analyzer/expandMacro` LSP extension returns the literal
//! expanded Rust source text for a `macro_rules!`-style invocation --
//! something Tree-sitter structurally cannot see (it only ever parses the
//! invocation's call-site syntax, never its expansion). That text can be
//! fed straight back through `agentops_scanner::extract_symbols`, the exact
//! same path a normal scan already uses, producing real `Symbol` entries
//! indistinguishable in shape from hand-written ones -- no new graph
//! concept needed. Confirmed live this session **not** to work for
//! `#[derive(...)]`-style invocations (the request returns `null` at every
//! attempted position) -- that class of macro is out of scope for this
//! capability.
//!
//! Every public entry point returns `Result` and is meant to be treated as
//! best-effort by callers: a missing `rust-analyzer` binary, a workspace
//! that doesn't build, a slow/timed-out handshake, or (for `expand_macro`)
//! a position that isn't a `macro_rules!`-expandable invocation are all
//! normal, expected outcomes here, never a reason to fail or block a scan.
//! The already-shipped Tree-sitter-only extraction remains the floor.
//!
//! Verified live against a real `rust-analyzer 1.97.1` process during this
//! crate's own spikes (see the project's recorded knowledge graph and the
//! LSP pilot plan for the verification): `async-lsp` + `tokio::process` +
//! `tokio-util`'s `compat` feature is sufficient -- no need for a second,
//! independent async-runtime crate (`async-process`, used by `async-lsp`'s
//! own canonical example) purely for process-spawning compatibility.

use std::ops::ControlFlow;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result};
use async_lsp::concurrency::ConcurrencyLayer;
use async_lsp::lsp_types::notification::{LogMessage, Progress, PublishDiagnostics, ShowMessage};
use async_lsp::lsp_types::{
    ClientCapabilities, DidOpenTextDocumentParams, DocumentSymbolParams, DocumentSymbolResponse, InitializeParams, InitializedParams, TextDocumentIdentifier, TextDocumentItem, Url,
};
use async_lsp::panic::CatchUnwindLayer;
use async_lsp::router::Router;
use async_lsp::LanguageServer;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
use tower::ServiceBuilder;

/// One resolved symbol from a real language server's own
/// `textDocument/documentSymbol` response -- deliberately a plain, minimal
/// shape, not `lsp_types::SymbolInformation`/`DocumentSymbol` themselves, so
/// callers (e.g. `agentops-scanner`) never need this crate's `async-lsp`/
/// `lsp-types` dependency in their own signatures.
#[derive(Debug, Clone, PartialEq)]
pub struct LspSymbol {
    pub name: String,
    /// A ready-made disambiguator string when this symbol sits inside a
    /// container the language server can name precisely -- e.g.
    /// `"impl Foo"` for an inherent impl's method, `"impl SomeTrait for Foo"`
    /// for a trait impl's -- verified live to already distinguish exactly
    /// the case Tree-sitter's own `container` field can't on its own for
    /// generic/blanket impls.
    pub container_name: Option<String>,
    /// 1-indexed, matching `agentops_scanner::Symbol`'s own convention.
    pub start_line: u32,
    pub end_line: u32,
}

const DEFAULT_RUST_ANALYZER_BIN: &str = "rust-analyzer";

/// `rust-analyzer/expandMacro` -- a custom, non-standard LSP extension (not
/// part of `lsp_types::request`'s built-in set), documented at
/// rust-analyzer.github.io/book/contributing/lsp-extensions.html and
/// verified live this session against a real server. `async-lsp` supports
/// arbitrary custom requests by implementing `lsp_types::request::Request`
/// directly, the same mechanism its own generated omni-trait methods use
/// internally.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExpandMacroParams {
    text_document: TextDocumentIdentifier,
    position: async_lsp::lsp_types::Position,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct ExpandedMacro {
    #[allow(dead_code)]
    name: String,
    expansion: String,
}

enum ExpandMacro {}
impl async_lsp::lsp_types::request::Request for ExpandMacro {
    type Params = ExpandMacroParams;
    type Result = ExpandedMacro;
    const METHOD: &'static str = "rust-analyzer/expandMacro";
}

/// Same "call `shutdown_background()` on the runtime's own drop, not the
/// default blocking drop" precedent as
/// `agentops-graph-pg::PostgresGraphStore`'s own `RuntimeGuard` -- avoids a
/// "cannot drop a runtime in a context where blocking is not allowed" panic
/// if this ever gets dropped from inside another async context.
struct RuntimeGuard(Option<tokio::runtime::Runtime>);

impl Drop for RuntimeGuard {
    fn drop(&mut self) {
        if let Some(rt) = self.0.take() {
            rt.shutdown_background();
        }
    }
}

impl std::ops::Deref for RuntimeGuard {
    type Target = tokio::runtime::Runtime;
    fn deref(&self) -> &tokio::runtime::Runtime {
        self.0.as_ref().unwrap()
    }
}

/// A live connection to one `rust-analyzer` process, indexing one Cargo
/// workspace. Spawn once per scan (not once per file -- re-spawning per
/// file would re-pay the whole workspace's indexing cost every time),
/// query `document_symbols` per file, then let this drop (or call
/// `shutdown` explicitly) when done with the whole pass.
pub struct LspClient {
    rt: RuntimeGuard,
    server: async_lsp::ServerSocket,
    mainloop_task: tokio::task::JoinHandle<Result<(), async_lsp::Error>>,
    child: tokio::process::Child,
    /// Guards against sending a second LSP `shutdown` request, which the
    /// spec forbids -- lets `Drop` always call `shutdown()` safely
    /// regardless of whether a caller already did.
    shut_down: bool,
}

impl LspClient {
    /// Spawns `rust-analyzer` (must already be on `PATH` -- see
    /// `rustup component add rust-analyzer`) rooted at `workspace_root`, and
    /// completes the `initialize`/`initialized` handshake, bounded by
    /// `handshake_timeout`.
    pub fn spawn(workspace_root: &Path, handshake_timeout: Duration) -> Result<Self> {
        Self::spawn_with_binary(workspace_root, DEFAULT_RUST_ANALYZER_BIN, handshake_timeout)
    }

    /// Test seam for `spawn` -- lets a test point at a binary name that
    /// doesn't exist to exercise the "not installed" failure path without
    /// needing to actually uninstall the real one.
    pub fn spawn_with_binary(workspace_root: &Path, binary: &str, handshake_timeout: Duration) -> Result<Self> {
        let workspace_root = workspace_root.canonicalize().with_context(|| format!("resolving workspace root {}", workspace_root.display()))?;
        let rt = tokio::runtime::Runtime::new().context("starting a Tokio runtime for the LSP client")?;

        let (server, mainloop_task, child) = rt.block_on(spawn_and_handshake(workspace_root.clone(), binary.to_string(), handshake_timeout))?;

        Ok(Self { rt: RuntimeGuard(Some(rt)), server, mainloop_task, child, shut_down: false })
    }

    /// Resolves every symbol in `file` (must be inside this client's
    /// workspace root) via a real `textDocument/documentSymbol` request.
    /// `file`'s content is read fresh from disk and sent as the `didOpen`
    /// body -- no editor-style incremental sync, since this is a one-shot
    /// enrichment pass, not a live editing session.
    pub fn document_symbols(&mut self, file: &Path, request_timeout: Duration) -> Result<Vec<LspSymbol>> {
        let file = file.canonicalize().with_context(|| format!("resolving {}", file.display()))?;
        let uri = Url::from_file_path(&file).map_err(|_| anyhow::anyhow!("not a valid file:// URI: {}", file.display()))?;
        let text = std::fs::read_to_string(&file).with_context(|| format!("reading {}", file.display()))?;
        let server = &mut self.server;

        self.rt.block_on(async move {
            server
                .did_open(DidOpenTextDocumentParams { text_document: TextDocumentItem { uri: uri.clone(), language_id: "rust".into(), version: 0, text } })
                .context("sending didOpen")?;

            let response = tokio::time::timeout(
                request_timeout,
                server.document_symbol(DocumentSymbolParams {
                    text_document: TextDocumentIdentifier { uri },
                    work_done_progress_params: Default::default(),
                    partial_result_params: Default::default(),
                }),
            )
            .await
            .context("rust-analyzer did not respond to documentSymbol within the timeout")??;

            Ok(flatten_document_symbol_response(response))
        })
    }

    /// Expands the macro invocation at `line`/`character` (0-indexed, per
    /// LSP convention -- unlike this crate's own `LspSymbol::start_line`,
    /// which is 1-indexed to match `agentops_scanner::Symbol`) in `file` via
    /// `rust-analyzer`'s custom `rust-analyzer/expandMacro` extension,
    /// returning the literal expanded Rust source text. `Err` for anything
    /// that isn't a `macro_rules!`-style invocation the server can expand
    /// (confirmed live: `#[derive(...)]` positions return a JSON `null`
    /// result here, not an error -- `RustAnalyzerError` maps that to `Err`
    /// too, so callers only ever see one outcome shape: expansion text, or
    /// a reason it's unavailable) -- callers must treat this as a normal,
    /// per-invocation outcome, never a reason to fail the whole scan.
    pub fn expand_macro(&mut self, file: &Path, line: u32, character: u32, request_timeout: Duration) -> Result<String> {
        let file = file.canonicalize().with_context(|| format!("resolving {}", file.display()))?;
        let uri = Url::from_file_path(&file).map_err(|_| anyhow::anyhow!("not a valid file:// URI: {}", file.display()))?;
        let text = std::fs::read_to_string(&file).with_context(|| format!("reading {}", file.display()))?;
        let server = &mut self.server;

        self.rt.block_on(async move {
            // Self-sufficient regardless of whether `document_symbols` was
            // already called for this file -- `didOpen` is required before
            // rust-analyzer will resolve a position-based request.
            server
                .did_open(DidOpenTextDocumentParams { text_document: TextDocumentItem { uri: uri.clone(), language_id: "rust".into(), version: 0, text } })
                .context("sending didOpen")?;

            let params = ExpandMacroParams { text_document: TextDocumentIdentifier { uri }, position: async_lsp::lsp_types::Position { line, character } };
            let expanded = tokio::time::timeout(request_timeout, server.request::<ExpandMacro>(params))
                .await
                .context("rust-analyzer did not respond to expandMacro within the timeout")?
                .context("no macro expansion available at this position (not a macro_rules!-style invocation, or expansion failed)")?;
            Ok(expanded.expansion)
        })
    }

    /// Proper LSP `shutdown` request + `exit` notification, per the LSP
    /// spec -- confirmed live during this crate's own spike: skipping this
    /// and just dropping the connection panics inside `async-lsp` itself
    /// (`"Sender is alive"`) and logs a `"client exited without proper
    /// shutdown sequence"` error on the server side. Idempotent -- a second
    /// call (or `Drop` calling it after an explicit call already did) is a
    /// no-op.
    pub fn shutdown(&mut self) {
        if self.shut_down {
            return;
        }
        self.shut_down = true;
        let server = &mut self.server;
        // Best-effort: by the time we're shutting down, a failure here just
        // means the child process gets killed a little less gracefully --
        // `kill_on_drop(true)` on the spawn already guarantees it doesn't
        // outlive this struct regardless.
        let _ = self.rt.block_on(async move {
            LanguageServer::shutdown(server, ()).await?;
            server.exit(())?;
            Ok::<_, async_lsp::Error>(())
        });
    }
}

impl Drop for LspClient {
    fn drop(&mut self) {
        self.shutdown();
        self.mainloop_task.abort();
        let child = &mut self.child;
        let _ = self.rt.block_on(async move { tokio::time::timeout(Duration::from_secs(2), child.wait()).await });
    }
}

async fn spawn_and_handshake(workspace_root: std::path::PathBuf, binary: String, handshake_timeout: Duration) -> Result<(async_lsp::ServerSocket, tokio::task::JoinHandle<Result<(), async_lsp::Error>>, tokio::process::Child)> {
    let mut child = tokio::process::Command::new(&binary)
        .current_dir(&workspace_root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("spawning `{binary}` -- is it installed? (`rustup component add rust-analyzer`)"))?;
    let stdout = child.stdout.take().context("rust-analyzer child had no stdout pipe")?.compat();
    let stdin = child.stdin.take().context("rust-analyzer child had no stdin pipe")?.compat_write();

    let (mainloop, mut server) = async_lsp::MainLoop::new_client(|_server| {
        let mut router = Router::new(());
        // Registering these is not optional: an empty router breaks the
        // connection the moment rust-analyzer emits its first progress/
        // diagnostic notification during indexing -- confirmed live during
        // this crate's own spike.
        router
            .notification::<Progress>(|_, _| ControlFlow::Continue(()))
            .notification::<PublishDiagnostics>(|_, _| ControlFlow::Continue(()))
            .notification::<ShowMessage>(|_, _| ControlFlow::Continue(()))
            .notification::<LogMessage>(|_, _| ControlFlow::Continue(()));
        ServiceBuilder::new().layer(CatchUnwindLayer::default()).layer(ConcurrencyLayer::default()).service(router)
    });
    let mainloop_task = tokio::spawn(async move { mainloop.run_buffered(stdout, stdin).await });

    let root_uri = Url::from_directory_path(&workspace_root).map_err(|_| anyhow::anyhow!("workspace root is not a valid file:// URI: {}", workspace_root.display()))?;
    #[allow(deprecated)]
    let init_params = InitializeParams { root_uri: Some(root_uri), capabilities: ClientCapabilities::default(), ..Default::default() };
    tokio::time::timeout(handshake_timeout, server.initialize(init_params)).await.context("rust-analyzer did not respond to `initialize` within the timeout")??;
    server.initialized(InitializedParams {}).context("sending `initialized`")?;

    // `initialize` itself responds long before rust-analyzer finishes
    // loading the crate graph / macro-resolution database in the
    // background -- confirmed live this session: `expand_macro` called
    // immediately after the handshake unreliably returns a JSON `null`
    // (indistinguishable from the genuine "not expandable" case) rather
    // than the real expansion, for a request that inherently needs more of
    // the workspace loaded than a purely syntactic `documentSymbol` does. A
    // fixed settle delay is a blunt fix -- the principled one (waiting on
    // rust-analyzer's own progress-completion signal) is a real future
    // improvement, not attempted here.
    tokio::time::sleep(Duration::from_secs(5)).await;

    Ok((server, mainloop_task, child))
}

/// Flattens `lsp_types`'s two possible `documentSymbol` response shapes
/// (nested `DocumentSymbol` tree, or flat `SymbolInformation` list) into one
/// plain `Vec<LspSymbol>` -- verified live that rust-analyzer returns the
/// flat shape by default (no hierarchical-symbol capability advertised),
/// but this handles both rather than assuming a future version won't
/// change that default.
fn flatten_document_symbol_response(response: Option<DocumentSymbolResponse>) -> Vec<LspSymbol> {
    match response {
        None => Vec::new(),
        Some(DocumentSymbolResponse::Flat(infos)) => infos
            .into_iter()
            .map(|info| LspSymbol { name: info.name, container_name: info.container_name, start_line: info.location.range.start.line + 1, end_line: info.location.range.end.line + 1 })
            .collect(),
        Some(DocumentSymbolResponse::Nested(symbols)) => {
            let mut out = Vec::new();
            flatten_nested(&symbols, None, &mut out);
            out
        }
    }
}

fn flatten_nested(symbols: &[async_lsp::lsp_types::DocumentSymbol], parent_name: Option<&str>, out: &mut Vec<LspSymbol>) {
    for symbol in symbols {
        out.push(LspSymbol { name: symbol.name.clone(), container_name: parent_name.map(str::to_string), start_line: symbol.range.start.line + 1, end_line: symbol.range.end.line + 1 });
        if let Some(children) = &symbol.children {
            flatten_nested(children, Some(&symbol.name), out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_root() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/impl_trait")
    }

    fn macro_fixture_root() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/macro_impl")
    }

    #[test]
    fn spawn_fails_gracefully_when_rust_analyzer_binary_is_missing() {
        let result = LspClient::spawn_with_binary(&fixture_root(), "definitely-not-a-real-binary-xyz123", Duration::from_secs(5));
        assert!(result.is_err(), "a missing binary must return Err, never panic");
    }

    #[test]
    fn spawn_fails_gracefully_for_a_nonexistent_workspace_root() {
        let result = LspClient::spawn(std::path::Path::new("/definitely/does/not/exist/xyz123"), Duration::from_secs(5));
        assert!(result.is_err(), "a nonexistent workspace root must return Err, never panic");
    }

    // Requires a real `rust-analyzer` on PATH -- skipped (not `#[ignore]`d
    // outright, but tolerant of a missing binary) so this test suite still
    // passes in environments where `rustup component add rust-analyzer`
    // hasn't been run, consistent with this crate's own "best-effort, never
    // fatal" contract for the feature it backs.
    #[test]
    fn document_symbols_disambiguates_inherent_vs_trait_impl_on_the_same_type() {
        let mut client = match LspClient::spawn(&fixture_root(), Duration::from_secs(30)) {
            Ok(client) => client,
            Err(e) => {
                eprintln!("skipping: could not spawn a real rust-analyzer ({e:#}) -- install via `rustup component add rust-analyzer` to run this test");
                return;
            }
        };

        let symbols = client.document_symbols(&fixture_root().join("src/lib.rs"), Duration::from_secs(30)).expect("documentSymbol request");

        // Three `forward` entries total: the trait's own abstract method
        // declaration (container "SomeTrait") plus one per impl block --
        // only the latter two are what this crate actually needs to
        // disambiguate.
        let impl_forwards: Vec<&LspSymbol> = symbols.iter().filter(|s| s.name == "forward" && s.container_name.as_deref().is_some_and(|c| c.starts_with("impl "))).collect();
        assert_eq!(impl_forwards.len(), 2, "both impl blocks' `forward` methods must be reported: {symbols:?}");
        let containers: std::collections::HashSet<_> = impl_forwards.iter().map(|s| s.container_name.as_deref()).collect();
        assert_eq!(containers, std::collections::HashSet::from([Some("impl Foo"), Some("impl SomeTrait for Foo")]), "the inherent and trait impls must be independently named: {symbols:?}");

        client.shutdown();
    }

    // Same "tolerant of a missing rust-analyzer" shape as the test above --
    // these exercise a genuinely different LSP request, not documentSymbol,
    // so they don't reuse that test's client.

    #[test]
    fn expand_macro_returns_the_literal_expansion_for_a_macro_rules_invocation() {
        let mut client = match LspClient::spawn(&macro_fixture_root(), Duration::from_secs(30)) {
            Ok(client) => client,
            Err(e) => {
                eprintln!("skipping: could not spawn a real rust-analyzer ({e:#}) -- install via `rustup component add rust-analyzer` to run this test");
                return;
            }
        };

        // Line 12 (0-indexed): `impl_forward!(Foo);` -- character 2 lands on "impl_forward".
        let expansion = client.expand_macro(&macro_fixture_root().join("src/lib.rs"), 12, 2, Duration::from_secs(30)).expect("expandMacro request");
        assert!(expansion.contains("impl Foo"), "expansion must contain the generated impl block: {expansion:?}");
        assert!(expansion.contains("generated_forward"), "expansion must contain the generated method: {expansion:?}");

        client.shutdown();
    }

    #[test]
    fn expand_macro_returns_err_for_a_derive_macro_position_not_a_panic() {
        let mut client = match LspClient::spawn(&macro_fixture_root(), Duration::from_secs(30)) {
            Ok(client) => client,
            Err(e) => {
                eprintln!("skipping: could not spawn a real rust-analyzer ({e:#}) -- install via `rustup component add rust-analyzer` to run this test");
                return;
            }
        };

        // Line 14 (0-indexed): `#[derive(Debug)]` -- confirmed live (this
        // session's spike) that rust-analyzer returns a JSON `null` result
        // for a derive-macro position, not an expansion. Tried at multiple
        // character offsets during the spike; character 4 (on "derive"
        // itself) is representative.
        let result = client.expand_macro(&macro_fixture_root().join("src/lib.rs"), 14, 4, Duration::from_secs(30));
        assert!(result.is_err(), "a derive-macro position must return Err, never panic or a spurious expansion: {result:?}");

        client.shutdown();
    }
}
