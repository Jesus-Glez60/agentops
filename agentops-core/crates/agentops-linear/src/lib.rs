//! Two-way Linear sync for Module 7's hybrid task manager — see
//! `~/Vaults/agentops-vnext/decisions/hybrid-task-manager-linear.md`.
//! Poll-based, not webhook-based, for this first pass: no inbound-webhook
//! infrastructure exists anywhere in the codebase today (same gap already
//! flagged against Module 4's hosted-rescan design). Follows the same
//! `ureq`-based external-call precedent `agentops-llm` established for the
//! Anthropic API.
//!
//! `external_id` is Linear's human-readable `identifier` (e.g. `"ENG-123"`),
//! not its internal UUID — Linear's GraphQL API resolves both interchangeably
//! wherever an issue `id` is expected, and the identifier is what a human
//! would actually reference.

use agentops_graph::{GraphStore, NewTask, TaskStatus};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

const DEFAULT_API_URL: &str = "https://api.linear.app/graphql";
const EXTERNAL_SOURCE: &str = "linear";

/// Linear API configuration — `AGENTOPS_LINEAR_API_KEY` follows the
/// `AGENTOPS_<SUBSYSTEM>_<PURPOSE>` env var convention already established
/// for `AGENTOPS_ANTHROPIC_API_KEY`.
#[derive(Debug, Clone)]
pub struct LinearConfig {
    pub api_key: String,
    /// Overridable so tests can point at a `wiremock` server instead of the
    /// real API.
    pub api_url: String,
}

impl LinearConfig {
    pub fn from_env() -> Result<Self> {
        let api_key = std::env::var("AGENTOPS_LINEAR_API_KEY").context("AGENTOPS_LINEAR_API_KEY is not set — Linear sync is opt-in and requires your own Linear API key")?;
        Ok(Self { api_key, api_url: DEFAULT_API_URL.to_string() })
    }
}

#[derive(Serialize)]
struct GraphQlRequest<'a> {
    query: &'a str,
    variables: serde_json::Value,
}

fn call_linear(config: &LinearConfig, query: &str, variables: serde_json::Value) -> Result<serde_json::Value> {
    let request = GraphQlRequest { query, variables };
    let mut response = ureq::post(&config.api_url)
        .header("Authorization", &config.api_key)
        .header("Content-Type", "application/json")
        .config()
        .http_status_as_error(false)
        .build()
        .send_json(&request)
        .context("calling the Linear GraphQL API")?;

    let status = response.status();
    if status.as_u16() == 401 {
        anyhow::bail!("Linear API rejected the key (401) — check AGENTOPS_LINEAR_API_KEY");
    }
    if !status.is_success() {
        let body = response.body_mut().read_to_string().unwrap_or_default();
        anyhow::bail!("Linear API returned {status}: {body}");
    }

    let body: serde_json::Value = response.body_mut().read_json().context("parsing Linear API response")?;
    if let Some(errors) = body.get("errors") {
        anyhow::bail!("Linear API returned GraphQL errors: {errors}");
    }
    body.get("data").cloned().ok_or_else(|| anyhow::anyhow!("Linear API response had no 'data' field: {body}"))
}

/// Maps Linear's `WorkflowState.type` (one of `triage`, `backlog`,
/// `unstarted`, `started`, `completed`, `canceled` — Linear's own fixed
/// enum, not a value this crate invents) to `TaskStatus`. `started` is
/// ambiguous between `InProgress`/`InReview` — Linear has no separate type
/// for "in review", teams model it as a `started`-type state named "In
/// Review" — so the state's `name` is checked first, falling back to
/// `InProgress` for any other `started` state.
fn task_status_from_linear_state(state_type: &str, state_name: &str) -> TaskStatus {
    match state_type {
        "completed" => TaskStatus::Done,
        "canceled" => TaskStatus::Cancelled,
        "started" if state_name.to_lowercase().contains("review") => TaskStatus::InReview,
        "started" => TaskStatus::InProgress,
        _ => TaskStatus::Todo,
    }
}

/// The inverse direction: which Linear `WorkflowState.type` a local
/// `TaskStatus` push should target. `InReview` maps to `started` (same
/// ambiguity as above) — the actual state picked among same-type
/// candidates prefers one named "review" (see `push_status`).
fn linear_state_type_for(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Todo => "unstarted",
        TaskStatus::InProgress | TaskStatus::InReview => "started",
        TaskStatus::Done => "completed",
        TaskStatus::Cancelled => "canceled",
    }
}

const PULL_QUERY: &str = "query($first: Int!) { issues(first: $first) { nodes { identifier title description priority assignee { name } state { name type } } } }";

/// Pulls up to `limit` issues from Linear and upserts each into `repo`'s
/// graph via `GraphStore::upsert_external_task` — idempotent on
/// `(external_source, external_id)`, so calling this repeatedly (the
/// poll-based sync loop) never duplicates an already-synced issue. Returns
/// the number of issues synced.
pub fn pull_issues(store: &dyn GraphStore, config: &LinearConfig, repo: &str, limit: u32) -> Result<usize> {
    let data = call_linear(config, PULL_QUERY, serde_json::json!({ "first": limit }))?;
    let nodes = data.pointer("/issues/nodes").and_then(|v| v.as_array()).cloned().unwrap_or_default();

    let mut synced = 0;
    for node in &nodes {
        let identifier = node.get("identifier").and_then(|v| v.as_str()).ok_or_else(|| anyhow::anyhow!("Linear issue missing 'identifier': {node}"))?;
        let title = node.get("title").and_then(|v| v.as_str()).unwrap_or("(untitled)").to_string();
        let description = node.get("description").and_then(|v| v.as_str()).map(String::from);
        let priority = node.get("priority").and_then(|v| v.as_number()).map(|n| n.to_string());
        let assignee = node.pointer("/assignee/name").and_then(|v| v.as_str()).map(String::from);
        let state_type = node.pointer("/state/type").and_then(|v| v.as_str()).unwrap_or("unstarted");
        let state_name = node.pointer("/state/name").and_then(|v| v.as_str()).unwrap_or("");

        store.upsert_external_task(NewTask {
            repo: repo.to_string(),
            title,
            description,
            status: task_status_from_linear_state(state_type, state_name),
            priority,
            assignee,
            external_source: Some(EXTERNAL_SOURCE.to_string()),
            external_id: Some(identifier.to_string()),
            session_id: None,
        })?;
        synced += 1;
    }
    Ok(synced)
}

const ISSUE_TEAM_STATES_QUERY: &str = "query($id: String!) { issue(id: $id) { team { states { nodes { id name type } } } } }";
const ISSUE_UPDATE_MUTATION: &str = "mutation($id: String!, $stateId: String!) { issueUpdate(id: $id, input: { stateId: $stateId }) { success } }";

#[derive(Deserialize, Debug)]
struct WorkflowState {
    id: String,
    name: String,
    #[serde(rename = "type")]
    state_type: String,
}

/// Pushes `status` to the Linear issue identified by `external_id` (its
/// `identifier`, e.g. `"ENG-123"`) — the local status transition's
/// counterpart to `pull_issues`. Looks up the issue's team's workflow
/// states first (Linear's `issueUpdate` takes a `stateId`, not a status
/// name) so this works across teams with differently-named/ordered
/// pipelines, rather than hardcoding Linear's default state names.
///
/// `custom_state_name`, when `Some`, matches by the workflow state's exact
/// `name` instead of `linear_state_type_for`'s type-inference — for teams
/// with custom states beyond the generic 5 (e.g. a "Testing" state that
/// isn't any of Linear's own `type` values). `None` preserves today's
/// behavior exactly, so every existing call site is unaffected.
pub fn push_status(config: &LinearConfig, external_id: &str, status: TaskStatus, custom_state_name: Option<&str>) -> Result<()> {
    let data = call_linear(config, ISSUE_TEAM_STATES_QUERY, serde_json::json!({ "id": external_id }))?;
    let states_json = data.pointer("/issue/team/states/nodes").cloned().ok_or_else(|| anyhow::anyhow!("Linear issue {external_id} not found, or has no team/workflow states"))?;
    let states: Vec<WorkflowState> = serde_json::from_value(states_json).context("parsing Linear workflow states")?;

    let target_type = linear_state_type_for(status);
    let candidates: Vec<&WorkflowState> = states.iter().filter(|s| s.state_type == target_type).collect();
    let chosen = if let Some(name) = custom_state_name {
        states.iter().find(|s| s.name.eq_ignore_ascii_case(name)).ok_or_else(|| anyhow::anyhow!("no workflow state named '{name}' found on {external_id}'s team"))?
    } else if status == TaskStatus::InReview {
        candidates.iter().find(|s| s.name.to_lowercase().contains("review")).copied().or_else(|| candidates.first().copied()).ok_or_else(|| anyhow::anyhow!("no workflow state of type '{target_type}' found on {external_id}'s team"))?
    } else {
        candidates.first().copied().ok_or_else(|| anyhow::anyhow!("no workflow state of type '{target_type}' found on {external_id}'s team"))?
    };

    let update_result = call_linear(config, ISSUE_UPDATE_MUTATION, serde_json::json!({ "id": external_id, "stateId": chosen.id }))?;
    if update_result.get("issueUpdate").and_then(|v| v.get("success")).and_then(|v| v.as_bool()) != Some(true) {
        anyhow::bail!("Linear's issueUpdate mutation did not report success for {external_id}: {update_result}");
    }
    Ok(())
}

/// Reads `task_id`'s current status and pushes it to Linear — errors if the
/// task isn't Linear-synced (`external_source != "linear"`). See
/// `push_status` for `custom_state_name`.
pub fn sync_push(store: &dyn GraphStore, config: &LinearConfig, task_id: i64, custom_state_name: Option<&str>) -> Result<()> {
    let task = store.get_task(task_id)?.ok_or_else(|| anyhow::anyhow!("task {task_id} not found"))?;
    let external_id = match task.external_source.as_deref() {
        Some(EXTERNAL_SOURCE) => task.external_id.as_deref().ok_or_else(|| anyhow::anyhow!("task {task_id} has external_source but no external_id"))?,
        _ => anyhow::bail!("task {task_id} is not Linear-synced (external_source: {:?})", task.external_source),
    };
    push_status(config, external_id, task.status, custom_state_name)
}

const USER_EMAIL_QUERY: &str = "query($id: String!) { user(id: $id) { email } }";

/// Resolves a Linear user id (as seen in a webhook payload's `assigneeId`,
/// an opaque UUID) to their email — Phase 6's auto-kickoff whitelist is
/// configured by email (the portable, human-editable identifier a config
/// file can reasonably contain), not Linear's internal id, so the webhook
/// receiver needs this lookup to check a payload's assignee against it.
pub fn user_email(config: &LinearConfig, user_id: &str) -> Result<String> {
    let data = call_linear(config, USER_EMAIL_QUERY, serde_json::json!({ "id": user_id }))?;
    data.pointer("/user/email").and_then(|v| v.as_str()).map(String::from).ok_or_else(|| anyhow::anyhow!("Linear user {user_id} not found, or has no email"))
}

const COMMENT_CREATE_MUTATION: &str = "mutation($issueId: String!, $body: String!) { commentCreate(input: { issueId: $issueId, body: $body }) { success } }";

/// Posts `body` as a comment on the Linear issue identified by
/// `external_id` (its `identifier`, e.g. `"ENG-123"`, which Linear's
/// GraphQL API resolves interchangeably with the internal UUID wherever an
/// issue id is expected). `CommentCreateInput`'s real shape was confirmed
/// via live schema introspection against Linear's actual API (its docs page
/// doesn't show this mutation's input type directly) — `issueId`/`body` are
/// the two fields needed here; other optional fields (`parentId` for
/// threaded replies, `quotedText`, etc.) aren't used by this pass.
pub fn post_comment(config: &LinearConfig, external_id: &str, body: &str) -> Result<()> {
    let result = call_linear(config, COMMENT_CREATE_MUTATION, serde_json::json!({ "issueId": external_id, "body": body }))?;
    if result.get("commentCreate").and_then(|v| v.get("success")).and_then(|v| v.as_bool()) != Some(true) {
        anyhow::bail!("Linear's commentCreate mutation did not report success for {external_id}: {result}");
    }
    Ok(())
}

const LIST_WEBHOOKS_QUERY: &str = "query($teamId: String!) { team(id: $teamId) { webhooks { nodes { id url enabled secret } } } }";
const CREATE_WEBHOOK_MUTATION: &str = "mutation($url: String!, $teamId: String!, $resourceTypes: [String!]!) { webhookCreate(input: { url: $url, teamId: $teamId, resourceTypes: $resourceTypes }) { success webhook { id enabled secret } } }";

/// `true` if this call created a new webhook, `false` if one already
/// existed for this exact URL — either way, `secret` is the HMAC signing
/// secret to use for verifying that webhook's deliveries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookRegistration {
    pub created: bool,
    pub secret: String,
}

/// Idempotently ensures a webhook pointing at `url` exists for `team_id` —
/// checks first (`team.webhooks`), only calls `webhookCreate` if none of
/// the team's existing webhooks already point at this exact URL. Safe to
/// call on every process startup, not just once.
///
/// **Fully automatic, including the signing secret** — `Webhook.secret` is
/// a real, queryable GraphQL field (confirmed via live schema introspection
/// against Linear's actual API, not just their docs page, which happens to
/// never select this field in its example queries and reads as if the
/// secret weren't API-accessible at all — it is, both on the object
/// `webhookCreate` returns and on any `webhooks`/`team.webhooks` query
/// afterward). No manual "copy the secret from the UI" step needed: this
/// function returns everything the caller needs to start verifying
/// deliveries immediately, whether the webhook was just created or already
/// existed.
pub fn ensure_webhook_registered(config: &LinearConfig, team_id: &str, url: &str, resource_types: &[&str]) -> Result<WebhookRegistration> {
    let existing = call_linear(config, LIST_WEBHOOKS_QUERY, serde_json::json!({ "teamId": team_id }))?;
    let existing_secret = existing.pointer("/team/webhooks/nodes").and_then(|v| v.as_array()).and_then(|nodes| {
        nodes.iter().find(|n| n.get("url").and_then(|u| u.as_str()) == Some(url)).and_then(|n| n.get("secret")).and_then(|s| s.as_str()).map(String::from)
    });
    if let Some(secret) = existing_secret {
        return Ok(WebhookRegistration { created: false, secret });
    }

    let created = call_linear(config, CREATE_WEBHOOK_MUTATION, serde_json::json!({ "url": url, "teamId": team_id, "resourceTypes": resource_types }))?;
    if created.get("webhookCreate").and_then(|v| v.get("success")).and_then(|v| v.as_bool()) != Some(true) {
        anyhow::bail!("Linear's webhookCreate mutation did not report success for team {team_id}: {created}");
    }
    let secret = created
        .pointer("/webhookCreate/webhook/secret")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Linear's webhookCreate response had no secret: {created}"))?
        .to_string();
    Ok(WebhookRegistration { created: true, secret })
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentops_graph::SqliteGraphStore;

    fn mock_config(base_url: &str) -> LinearConfig {
        LinearConfig { api_key: "test-key".into(), api_url: base_url.to_string() }
    }

    #[test]
    fn task_status_from_linear_state_maps_every_known_type() {
        assert_eq!(task_status_from_linear_state("completed", "Done"), TaskStatus::Done);
        assert_eq!(task_status_from_linear_state("canceled", "Canceled"), TaskStatus::Cancelled);
        assert_eq!(task_status_from_linear_state("started", "In Progress"), TaskStatus::InProgress);
        assert_eq!(task_status_from_linear_state("started", "In Review"), TaskStatus::InReview);
        assert_eq!(task_status_from_linear_state("unstarted", "Todo"), TaskStatus::Todo);
        assert_eq!(task_status_from_linear_state("backlog", "Backlog"), TaskStatus::Todo);
        assert_eq!(task_status_from_linear_state("triage", "Triage"), TaskStatus::Todo);
    }

    #[tokio::test]
    async fn pull_issues_upserts_every_issue_as_an_external_task() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": { "issues": { "nodes": [
                    { "identifier": "ENG-1", "title": "Fix login bug", "description": "Users can't log in.", "priority": 2, "assignee": { "name": "Alice" }, "state": { "name": "In Progress", "type": "started" } },
                    { "identifier": "ENG-2", "title": "Add dark mode", "description": null, "priority": 3, "assignee": null, "state": { "name": "Todo", "type": "unstarted" } }
                ] } }
            })))
            .mount(&server)
            .await;

        let store = SqliteGraphStore::open_in_memory().unwrap();
        let config = mock_config(&server.uri());
        let synced = pull_issues(&store, &config, "demo", 50).unwrap();

        assert_eq!(synced, 2);
        let tasks = store.list_tasks("demo").unwrap();
        assert_eq!(tasks.len(), 2);
        let eng1 = tasks.iter().find(|t| t.external_id.as_deref() == Some("ENG-1")).unwrap();
        assert_eq!(eng1.title, "Fix login bug");
        assert_eq!(eng1.status, TaskStatus::InProgress);
        assert_eq!(eng1.assignee.as_deref(), Some("Alice"));
        assert_eq!(eng1.external_source.as_deref(), Some("linear"));
    }

    #[tokio::test]
    async fn pulling_the_same_issue_twice_does_not_duplicate_the_task() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": { "issues": { "nodes": [
                    { "identifier": "ENG-1", "title": "Fix login bug", "description": null, "priority": 2, "assignee": null, "state": { "name": "Todo", "type": "unstarted" } }
                ] } }
            })))
            .mount(&server)
            .await;

        let store = SqliteGraphStore::open_in_memory().unwrap();
        let config = mock_config(&server.uri());
        pull_issues(&store, &config, "demo", 50).unwrap();
        pull_issues(&store, &config, "demo", 50).unwrap();

        assert_eq!(store.list_tasks("demo").unwrap().len(), 1, "re-pulling the same issue must update in place, not duplicate");
    }

    #[tokio::test]
    async fn sync_push_looks_up_the_matching_workflow_state_and_updates_the_issue() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::body_string_contains("team"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": { "issue": { "team": { "states": { "nodes": [
                    { "id": "state-todo", "name": "Todo", "type": "unstarted" },
                    { "id": "state-progress", "name": "In Progress", "type": "started" },
                    { "id": "state-done", "name": "Done", "type": "completed" }
                ] } } } }
            })))
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::body_string_contains("issueUpdate"))
            .and(wiremock::matchers::body_string_contains("state-done"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({ "data": { "issueUpdate": { "success": true } } })))
            .mount(&server)
            .await;

        let store = SqliteGraphStore::open_in_memory().unwrap();
        let config = mock_config(&server.uri());
        let task_id = store
            .upsert_external_task(NewTask {
                repo: "demo".into(),
                title: "Fix login bug".into(),
                description: None,
                status: TaskStatus::Done,
                priority: None,
                assignee: None,
                external_source: Some("linear".into()),
                external_id: Some("ENG-1".into()),
                session_id: None,
            })
            .unwrap();

        sync_push(&store, &config, task_id, None).unwrap();
    }

    /// Extension 1 (Phase 6b): a custom state name (e.g. a team's own
    /// "Testing" state, not any of Linear's fixed `type` values) must be
    /// matched by exact name, bypassing type-inference entirely.
    #[tokio::test]
    async fn push_status_with_a_custom_state_name_matches_by_name_not_by_type() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::body_string_contains("team"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": { "issue": { "team": { "states": { "nodes": [
                    { "id": "state-todo", "name": "Todo", "type": "unstarted" },
                    { "id": "state-testing", "name": "Testing", "type": "started" }
                ] } } } }
            })))
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::body_string_contains("issueUpdate"))
            .and(wiremock::matchers::body_string_contains("state-testing"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({ "data": { "issueUpdate": { "success": true } } })))
            .mount(&server)
            .await;

        let config = mock_config(&server.uri());
        // status is Todo (would normally target "unstarted"/state-todo) —
        // custom_state_name must override that inference entirely.
        push_status(&config, "ENG-1", TaskStatus::Todo, Some("Testing")).unwrap();
    }

    #[tokio::test]
    async fn post_comment_sends_the_body_and_succeeds_on_a_successful_mutation() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::body_string_contains("commentCreate"))
            .and(wiremock::matchers::body_string_contains("Work summary"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({ "data": { "commentCreate": { "success": true } } })))
            .mount(&server)
            .await;

        let config = mock_config(&server.uri());
        post_comment(&config, "ENG-1", "Work summary: done.").unwrap();
    }

    #[tokio::test]
    async fn post_comment_errors_when_linear_reports_success_false() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({ "data": { "commentCreate": { "success": false } } })))
            .mount(&server)
            .await;

        let config = mock_config(&server.uri());
        let err = post_comment(&config, "ENG-1", "body").unwrap_err();
        assert!(err.to_string().contains("did not report success"));
    }

    #[tokio::test]
    async fn sync_push_refuses_a_task_that_is_not_linear_synced() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let config = mock_config("http://127.0.0.1:1");
        let task_id = store
            .create_task(NewTask { repo: "demo".into(), title: "native task".into(), description: None, status: TaskStatus::Todo, priority: None, assignee: None, external_source: None, external_id: None, session_id: None })
            .unwrap();

        let err = sync_push(&store, &config, task_id, None).unwrap_err();
        assert!(err.to_string().contains("not Linear-synced"));
    }

    /// Regression test for a real gap caught live-testing against a real
    /// Linear workspace: Linear can return HTTP 200 with no GraphQL
    /// `errors` array yet still report `success: false` on the mutation
    /// payload itself (e.g. a permission issue) — `push_status` must
    /// surface that as a real error, not silently report success.
    #[tokio::test]
    async fn sync_push_errors_when_linear_reports_success_false() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::body_string_contains("team"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": { "issue": { "team": { "states": { "nodes": [
                    { "id": "state-done", "name": "Done", "type": "completed" }
                ] } } } }
            })))
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::body_string_contains("issueUpdate"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({ "data": { "issueUpdate": { "success": false } } })))
            .mount(&server)
            .await;

        let store = SqliteGraphStore::open_in_memory().unwrap();
        let config = mock_config(&server.uri());
        let task_id = store
            .upsert_external_task(NewTask {
                repo: "demo".into(),
                title: "Fix login bug".into(),
                description: None,
                status: TaskStatus::Done,
                priority: None,
                assignee: None,
                external_source: Some("linear".into()),
                external_id: Some("ENG-1".into()),
                session_id: None,
            })
            .unwrap();

        let err = sync_push(&store, &config, task_id, None).unwrap_err();
        assert!(err.to_string().contains("did not report success"), "{err}");
    }

    #[tokio::test]
    async fn user_email_resolves_a_linear_user_id_to_their_email() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({ "data": { "user": { "email": "dev@example.com" } } })))
            .mount(&server)
            .await;

        let email = user_email(&mock_config(&server.uri()), "user-123").unwrap();
        assert_eq!(email, "dev@example.com");
    }

    #[tokio::test]
    async fn ensure_webhook_registered_skips_creation_and_returns_the_existing_secret_when_a_webhook_for_this_url_already_exists() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": { "team": { "webhooks": { "nodes": [
                    { "id": "wh-1", "url": "https://example.com/webhooks/linear", "enabled": true, "secret": "existing-secret-123" }
                ] } } }
            })))
            .mount(&server)
            .await;

        let result = ensure_webhook_registered(&mock_config(&server.uri()), "team-1", "https://example.com/webhooks/linear", &["Issue"]).unwrap();
        assert!(!result.created, "must not create a duplicate webhook for the same URL");
        assert_eq!(result.secret, "existing-secret-123");
    }

    #[tokio::test]
    async fn ensure_webhook_registered_creates_one_and_returns_its_secret_when_none_exists_for_this_url() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::body_string_contains("query("))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({ "data": { "team": { "webhooks": { "nodes": [] } } } })))
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::body_string_contains("webhookCreate"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": { "webhookCreate": { "success": true, "webhook": { "id": "wh-new", "enabled": true, "secret": "new-secret-456" } } }
            })))
            .mount(&server)
            .await;

        let result = ensure_webhook_registered(&mock_config(&server.uri()), "team-1", "https://example.com/webhooks/linear", &["Issue"]).unwrap();
        assert!(result.created);
        assert_eq!(result.secret, "new-secret-456");
    }
}
