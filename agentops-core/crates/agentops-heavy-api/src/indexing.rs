//! Orchestrates the clone -> scan -> embed -> docgen pipeline that turns a
//! connected repo into something searchable and documented, plus the
//! polled-progress endpoints the "Connect repository" wizard's indexing
//! screen reads from.
//!
//! This is the first detached background task (`tokio::spawn`) anywhere in
//! this codebase's production code -- there's no existing pattern to
//! mirror, so a few things are deliberate here rather than assumed:
//! - the spawned future's body never lets an internal error escape as an
//!   unhandled `Result` -- every fallible step is `match`ed and, on `Err`,
//!   written to `indexing_jobs.status = 'failed'` plus a log line, not
//!   silently dropped;
//! - only cheap-to-clone `Arc`-wrapped state is captured into the `async
//!   move` block (`state.indexing`/`state.store`/`state.secrets` are
//!   already `Arc`s on `AppState`), not a second, independently-opened
//!   store handle;
//! - there is deliberately no graceful-shutdown story yet (this server
//!   doesn't have one at all today) -- a job in flight when the process
//!   exits is left `status = 'running'` forever. The status-poll response
//!   doesn't currently correct for this; a real deployment would want the
//!   frontend (or a future janitor pass) to treat a `running` job whose
//!   `current_stage` hasn't advanced in some time as probably-dead rather
//!   than trusting `running` indefinitely. Flagged, not solved here.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::extract::{Path as AxumPath, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::Mutex as AsyncMutex;

use agentops_accounts::User;
use agentops_heavy_embeddings::SemanticIndex;
use agentops_repo_access::indexing_store::{IndexingStore, JobKind, JobStatus, StageStatus, STAGE_ORDER};
use agentops_repo_access::secrets::SecretsProvider;
use agentops_repo_access::store::{ConnectionStatus, ConnectionStore, RepoConnection};

use crate::{require_session_capability, resolve_tenant, AppState};

/// Exactly the state `create_and_spawn_job`/`run_job` need, decoupled from
/// the full `AppState` -- so `github_app_routes`' webhook receiver (which
/// deliberately opens its own independent second connections to the same
/// underlying SQLite files, matching `LinearModuleState`'s established
/// precedent, rather than trying to share `AppState`'s handles across an
/// unrelated router-construction path) can spawn indexing jobs too, without
/// needing to fabricate an entire `AppState` (accounts/teams/docbrain-dir
/// and all) it has no other use for.
#[derive(Clone)]
pub struct IndexingDeps {
    pub indexing: Arc<Mutex<IndexingStore>>,
    pub connections: Arc<Mutex<ConnectionStore>>,
    pub secrets: Arc<dyn SecretsProvider + Send + Sync>,
    pub search_index: Option<Arc<AsyncMutex<SemanticIndex>>>,
    pub repo_checkouts_dir: PathBuf,
    /// Needed only for cloning `ConnectionMethod::GitHubApp` connections
    /// (a fresh installation token has to be minted for the clone) --
    /// `None` when no GitHub App is registered for this deployment, in
    /// which case a GitHub-App-method job fails cleanly at the clone stage
    /// rather than panicking on a missing config.
    pub github_app_config: Option<crate::github_app_routes::GitHubAppConfig>,
}

impl AppState {
    pub fn indexing_deps(&self) -> IndexingDeps {
        IndexingDeps {
            indexing: self.indexing.clone(),
            connections: self.store.clone(),
            secrets: self.secrets.clone(),
            search_index: self.search_index.clone(),
            repo_checkouts_dir: self.repo_checkouts_dir.clone(),
            github_app_config: self.github_app_config.clone(),
        }
    }
}

/// One `<tenant>/<connection_id>` subdirectory per connection under
/// `repo_checkouts_dir` -- the directory name is `connection_id` itself,
/// deliberately **not** anything derived from `repo_url` or a
/// canonicalized path. This guarantees `agentops_mcp::scan::repo_name()`
/// (which canonicalizes the path and takes its final component) resolves
/// to exactly this same `connection_id` string once the clone exists,
/// without ever needing to canonicalize a not-yet-existing path up front
/// (whose fallback behavior -- the raw path string -- is exactly the
/// raw-path-vs-canonicalized-name mismatch a prior live-tested bug hit).
fn checkout_path(repo_checkouts_dir: &std::path::Path, tenant: &str, connection_id: &str) -> PathBuf {
    repo_checkouts_dir.join(tenant).join(connection_id)
}

/// 16 random bytes, hex-encoded -- same shape as `team.rs`'s
/// `new_random_tenant_id`, a small deliberate duplication of the pattern
/// rather than a shared dependency for one more call site.
fn new_random_job_id() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("system randomness must be available");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Creates the job row (+ all 9 stage rows) and spawns the orchestration
/// task. Returns the new job's id immediately -- the caller does not wait
/// for the job to finish.
pub fn create_and_spawn_job(deps: &IndexingDeps, tenant: String, connection: RepoConnection, kind: JobKind) -> anyhow::Result<String> {
    let local_path = checkout_path(&deps.repo_checkouts_dir, &tenant, &connection.id);
    // Pre-clone: this equals connection.id today (see checkout_path's doc
    // comment) -- computed via the same derivation `agentops_mcp::scan::
    // repo_name()` will independently arrive at post-clone, not by calling
    // that function on a path that doesn't exist yet.
    let repo_name = connection.id.clone();

    let job_id = new_random_job_id();
    {
        let store = deps.indexing.lock().unwrap();
        store.create_job(&tenant, &job_id, &connection.id, &repo_name, &local_path.to_string_lossy(), kind)?;
    }

    let deps = deps.clone();
    let job_id_for_task = job_id.clone();
    tokio::spawn(async move {
        run_job(deps, tenant, job_id_for_task, connection, local_path, repo_name).await;
    });

    Ok(job_id)
}

/// Runs every stage in `STAGE_ORDER` in sequence, recording progress as it
/// goes. Never panics past its own boundary by design (see module doc) --
/// every step's error is caught and turned into a `fail_stage`/`finish_job`
/// call, not propagated as an unhandled `Result` out of an `async` task
/// nothing awaits.
async fn run_job(deps: IndexingDeps, tenant: String, job_id: String, connection: RepoConnection, local_path: PathBuf, repo_name: String) {
    macro_rules! log {
        ($($arg:tt)*) => {{
            let line = format!($($arg)*);
            let store = deps.indexing.lock().unwrap();
            let _ = store.append_log(&tenant, &job_id, &line);
        }};
    }
    macro_rules! start_stage {
        ($stage:expr) => {{
            let store = deps.indexing.lock().unwrap();
            let _ = store.start_stage(&tenant, &job_id, $stage);
        }};
    }
    macro_rules! finish_stage {
        ($stage:expr, $current:expr, $total:expr) => {{
            let store = deps.indexing.lock().unwrap();
            let _ = store.finish_stage(&tenant, &job_id, $stage, $current, $total);
        }};
    }
    macro_rules! fail_and_return {
        ($stage:expr, $reason:expr) => {{
            let reason = $reason;
            log!("FAILED at {}: {reason}", $stage);
            let store = deps.indexing.lock().unwrap();
            let _ = store.fail_stage(&tenant, &job_id, $stage, &reason);
            let _ = store.finish_job(&tenant, &job_id, JobStatus::Failed);
            drop(store);
            let conn_store = deps.connections.lock().unwrap();
            let _ = conn_store.set_status(&tenant, &connection.id, ConnectionStatus::Failed(reason));
            return;
        }};
    }

    // Stage 1: connection verified. For an SSH connection reaching this
    // job at all, `verify_repo`'s own key check already ran successfully
    // (or this is a reindex/retry of an already-`Active` connection) --
    // there's nothing further to check here, this stage exists in the
    // stage list purely to mirror the wizard's own mental model of "auth
    // confirmed" as its own step.
    start_stage!(STAGE_ORDER[0]);
    log!("connection verified");
    finish_stage!(STAGE_ORDER[0], None, None);

    // Stage 2: repository cloned -- the real clone into this job's own
    // working tree (see this fn's module doc + create_and_spawn_job's
    // comment on why this is a second clone, not a reuse of verify_repo's
    // throwaway one).
    start_stage!(STAGE_ORDER[1]);
    log!("cloning {} into {}", connection.repo_url, local_path.display());
    if let Some(parent) = local_path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            fail_and_return!(STAGE_ORDER[1], format!("creating checkout parent directory: {e}"));
        }
    }
    match &connection.method {
        agentops_repo_access::store::ConnectionMethod::Ssh => {
            let Some(encrypted_key) = &connection.encrypted_private_key_openssh else {
                fail_and_return!(STAGE_ORDER[1], "connection has no SSH key".to_string());
            };
            let unlocked = match agentops_repo_access::UnlockedKey::unlock_for_repo(deps.secrets.as_ref(), &tenant, &connection.id, encrypted_key) {
                Ok(k) => k,
                Err(e) => fail_and_return!(STAGE_ORDER[1], format!("unlocking deploy key: {e}")),
            };
            if let Err(e) = agentops_repo_access::clone_repo(&connection.repo_url, &local_path, &unlocked, agentops_repo_access::GITHUB_KNOWN_HOSTS) {
                fail_and_return!(STAGE_ORDER[1], format!("{e}"));
            }
        }
        agentops_repo_access::store::ConnectionMethod::GitHubApp => {
            let Some(config) = &deps.github_app_config else {
                fail_and_return!(STAGE_ORDER[1], "no GitHub App is configured for this deployment".to_string());
            };
            let Some(installation_id) = &connection.installation_id else {
                fail_and_return!(STAGE_ORDER[1], "connection has no installation id".to_string());
            };
            let installation_id: u64 = match installation_id.parse() {
                Ok(v) => v,
                Err(_) => fail_and_return!(STAGE_ORDER[1], "connection's installation id is not numeric".to_string()),
            };
            let token = match crate::github_app_routes::fresh_installation_token(config, installation_id).await {
                Ok(t) => t,
                Err(e) => fail_and_return!(STAGE_ORDER[1], format!("minting installation token: {e}")),
            };
            if let Err(e) = agentops_repo_access::clone_repo_https(&connection.repo_url, &local_path, &token) {
                fail_and_return!(STAGE_ORDER[1], format!("{e}"));
            }
        }
    }
    log!("clone complete");
    finish_stage!(STAGE_ORDER[1], None, None);

    // Stage 3: files discovered.
    start_stage!(STAGE_ORDER[2]);
    let report = match agentops_scanner::scan_repo(&local_path) {
        Ok(r) => r,
        Err(e) => fail_and_return!(STAGE_ORDER[2], format!("scanning repo: {e}")),
    };
    log!("discovered {} files", report.files.len());
    finish_stage!(STAGE_ORDER[2], Some(report.files.len() as i64), Some(report.files.len() as i64));

    // Stages 4-6: symbols extracted / dependencies mapped / knowledge nodes
    // created -- one `persist()` call under the hood (see this module's
    // doc comment and the plan: `persist()` is monolithic internally, so
    // these three stages complete back-to-back right after it returns, no
    // live sub-progress within it for this pass. This is also where
    // `scan_history`/`scan_history_entries` gets written automatically, as
    // a normal side effect of `persist()` -- no separate code needed to
    // keep that table populated.
    start_stage!(STAGE_ORDER[3]);
    let summary = match agentops_mcp::scan::persist(&local_path, &report, false) {
        Ok(s) => s,
        Err(e) => fail_and_return!(STAGE_ORDER[3], format!("persisting scan: {e}")),
    };
    log!("extracted {} symbols", summary.symbols);
    finish_stage!(STAGE_ORDER[3], Some(summary.symbols as i64), Some(summary.symbols as i64));

    start_stage!(STAGE_ORDER[4]);
    log!("mapped {} dependency edges, {} reference edges", summary.dependency_edges, summary.reference_edges);
    finish_stage!(STAGE_ORDER[4], None, None);

    start_stage!(STAGE_ORDER[5]);
    log!("knowledge nodes created ({} files, {} pruned)", summary.files, summary.pruned_files);
    finish_stage!(STAGE_ORDER[5], None, None);

    // Stage 7: embeddings generated -- reuses /search/index's own internals
    // directly rather than a self-HTTP-call. Skipped (marked done
    // instantly) when Qdrant isn't configured for this deployment, matching
    // the existing "optional feature, don't fail the job" posture
    // `search_not_configured` already establishes for /search* routes.
    start_stage!(STAGE_ORDER[6]);
    match &deps.search_index {
        None => {
            log!("semantic search not configured for this deployment -- skipping embeddings");
            finish_stage!(STAGE_ORDER[6], Some(0), Some(0));
        }
        Some(search_index) => {
            let items = {
                let db_path = agentops_mcp::scan::graph_db_path(&local_path);
                match agentops_graph::SqliteGraphStore::open(&db_path) {
                    Ok(graph_store) => match agentops_heavy_embeddings::collect_index_items(&graph_store, &repo_name) {
                        Ok(items) => Some(items),
                        Err(e) => {
                            fail_and_return!(STAGE_ORDER[6], format!("collecting items to embed: {e}"));
                        }
                    },
                    Err(e) => {
                        fail_and_return!(STAGE_ORDER[6], format!("opening graph store: {e}"));
                    }
                }
            };
            if let Some(items) = items {
                let mut index = search_index.lock().await;
                match index.index_items(&items).await {
                    Ok(count) => {
                        log!("embedded {count} items");
                        finish_stage!(STAGE_ORDER[6], Some(count as i64), Some(items.len() as i64));
                    }
                    Err(e) => {
                        drop(index);
                        fail_and_return!(STAGE_ORDER[6], format!("indexing embeddings: {e}"));
                    }
                }
            }
        }
    }

    // Stage 8: documentation generated -- AGENTS.md-style onboarding doc
    // (what `agentops install` already produces), not `docbrain-ingest`
    // (which needs an external docs URL most freshly-connected code repos
    // won't have).
    start_stage!(STAGE_ORDER[7]);
    match generate_onboarding_doc(&local_path, &repo_name) {
        Ok(out_path) => {
            log!("wrote {}", out_path.display());
            finish_stage!(STAGE_ORDER[7], None, None);
        }
        Err(e) => {
            // Best-effort, matching `persist()`'s own "only a genuine
            // failure to build/persist the doc page propagates" posture for
            // its LLM-assisted labeling step -- a docgen failure shouldn't
            // sink an otherwise-successful index.
            log!("documentation generation failed (non-fatal): {e}");
            finish_stage!(STAGE_ORDER[7], None, None);
        }
    }

    // Stage 9: index ready.
    start_stage!(STAGE_ORDER[8]);
    log!("index ready");
    finish_stage!(STAGE_ORDER[8], None, None);
    {
        let store = deps.indexing.lock().unwrap();
        let _ = store.finish_job(&tenant, &job_id, JobStatus::Succeeded);
    }
    {
        let conn_store = deps.connections.lock().unwrap();
        let _ = conn_store.set_status(&tenant, &connection.id, ConnectionStatus::Active);
    }
}

/// Mirrors `agentops-mcp::docgen::generate_docs` (private to that crate, so
/// reimplemented here rather than exposed cross-crate for one call site) --
/// re-scans read-only purely to recompute the PageRank file ordering
/// (deliberately not persisted anywhere, cheap to recompute), then renders
/// and writes `repo-map.md` under the checkout.
fn generate_onboarding_doc(repo_path: &std::path::Path, repo_name: &str) -> anyhow::Result<PathBuf> {
    let db_path = agentops_mcp::scan::graph_db_path(repo_path);
    let store = agentops_graph::SqliteGraphStore::open(&db_path)?;
    let report = agentops_scanner::scan_repo(repo_path)?;
    let ranked: Vec<PathBuf> = agentops_scanner::rank_files(repo_path, &report.files).into_iter().map(|(p, _)| p).collect();
    let doc = agentops_docgen::render_onboarding_doc(&store, repo_name, &ranked)?;
    let out_path = repo_path.join("repo-map.md");
    agentops_docgen::write_to_file(&doc, &out_path)?;
    Ok(out_path)
}

#[derive(Debug, Deserialize, Default)]
pub struct StartIndexingRequest {
    #[serde(default)]
    tenant: Option<String>,
    #[serde(default)]
    kind: Option<String>,
}

pub async fn start_indexing(State(state): State<AppState>, user: Option<axum::Extension<User>>, AxumPath(id): AxumPath<String>, body: Option<Json<StartIndexingRequest>>) -> (StatusCode, Json<Value>) {
    let body = body.map(|Json(b)| b).unwrap_or_default();
    let tenant = match resolve_tenant(&user, body.tenant.as_deref()) {
        Ok(t) => t,
        Err(e) => return e,
    };
    if let Err(e) = require_session_capability(&state, &user, &tenant, agentops_teams::CAP_REPOS_REINDEX) {
        return e;
    }

    let connection = {
        let store = state.store.lock().unwrap();
        match store.get_connection(&tenant, &id) {
            Ok(Some(c)) => c,
            Ok(None) => return (StatusCode::NOT_FOUND, Json(json!({ "error": "no such connection for this tenant" }))),
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
        }
    };

    let has_prior_job = { state.indexing.lock().unwrap().latest_job_for_connection(&tenant, &id) };
    let default_kind = match has_prior_job {
        Ok(Some(_)) => JobKind::Reindex,
        Ok(None) => JobKind::Initial,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    };
    let kind = match body.kind.as_deref() {
        Some("initial") => JobKind::Initial,
        Some("reindex") => JobKind::Reindex,
        Some(other) => return (StatusCode::BAD_REQUEST, Json(json!({ "error": format!("unknown kind {other:?}, expected \"initial\" or \"reindex\"") }))),
        None => default_kind,
    };

    match create_and_spawn_job(&state.indexing_deps(), tenant, connection, kind) {
        Ok(job_id) => (StatusCode::ACCEPTED, Json(json!({ "job_id": job_id }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    }
}

#[derive(Debug, Deserialize, Default)]
pub struct IndexStatusQuery {
    #[serde(default)]
    tenant: Option<String>,
    #[serde(default)]
    job_id: Option<String>,
}

pub async fn indexing_status(State(state): State<AppState>, user: Option<axum::Extension<User>>, AxumPath(id): AxumPath<String>, Query(q): Query<IndexStatusQuery>) -> (StatusCode, Json<Value>) {
    let tenant = match resolve_tenant(&user, q.tenant.as_deref()) {
        Ok(t) => t,
        Err(e) => return e,
    };
    if let Err(e) = require_session_capability(&state, &user, &tenant, agentops_teams::CAP_REPOS_VIEW) {
        return e;
    }

    let store = state.indexing.lock().unwrap();
    let job = match &q.job_id {
        Some(job_id) => store.get_job(&tenant, job_id),
        None => store.latest_job_for_connection(&tenant, &id),
    };
    let job = match job {
        Ok(Some(j)) => j,
        Ok(None) => return (StatusCode::NOT_FOUND, Json(json!({ "error": "no indexing job found for this connection" }))),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    };
    let stages = match store.list_stages(&tenant, &job.id) {
        Ok(s) => s,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    };

    // Server-computed, matching `list_repos`'s `can_connect` convention of
    // never asking the frontend to reimplement this logic client-side.
    let done_count = stages.iter().filter(|s| s.status == StageStatus::Done).count();
    let overall_percent = ((done_count as f64 / STAGE_ORDER.len() as f64) * 100.0).round() as i64;

    (
        StatusCode::OK,
        Json(json!({
            "job": {
                "id": job.id,
                "kind": job.kind,
                "status": job.status,
                "current_stage": job.current_stage,
                "created_at": job.created_at,
                "finished_at": job.finished_at,
            },
            "stages": stages,
            "overall_percent": overall_percent,
        })),
    )
}

pub async fn retry_indexing(State(state): State<AppState>, user: Option<axum::Extension<User>>, AxumPath(id): AxumPath<String>, Query(q): Query<IndexStatusQuery>) -> (StatusCode, Json<Value>) {
    let tenant = match resolve_tenant(&user, q.tenant.as_deref()) {
        Ok(t) => t,
        Err(e) => return e,
    };
    if let Err(e) = require_session_capability(&state, &user, &tenant, agentops_teams::CAP_REPOS_REINDEX) {
        return e;
    }

    let connection = {
        let store = state.store.lock().unwrap();
        match store.get_connection(&tenant, &id) {
            Ok(Some(c)) => c,
            Ok(None) => return (StatusCode::NOT_FOUND, Json(json!({ "error": "no such connection for this tenant" }))),
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
        }
    };

    let job = match &q.job_id {
        Some(job_id) => state.indexing.lock().unwrap().get_job(&tenant, job_id),
        None => state.indexing.lock().unwrap().latest_job_for_connection(&tenant, &id),
    };
    match job {
        Ok(Some(j)) if j.status != JobStatus::Failed => {
            return (StatusCode::CONFLICT, Json(json!({ "error": "the referenced job is not in a failed state" })));
        }
        Ok(None) => return (StatusCode::NOT_FOUND, Json(json!({ "error": "no indexing job found for this connection" }))),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
        Ok(Some(_)) => {}
    }

    match create_and_spawn_job(&state.indexing_deps(), tenant, connection, JobKind::Reindex) {
        Ok(job_id) => (StatusCode::ACCEPTED, Json(json!({ "job_id": job_id }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    }
}
