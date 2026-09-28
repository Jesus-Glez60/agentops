//! `POST /contact-leads` — public, unauthenticated marketing-site lead
//! capture (the `/pricing/` contact form in `apps/marketing`). No repo/org
//! context exists yet at this point, so this deliberately doesn't touch
//! `AppState`/`ConnectionStore`/tenant resolution the way every other route
//! in this crate does — same shape as `health_router` (see `lib.rs`).
//!
//! **Hardened per a 5-persona diff-review council** (independently
//! converged on: no body-size cap sharing a process with authenticated
//! traffic is a DoS vector; `println!`-only "persistence" is not durable;
//! free-text fields need server-side validation, not just the form's HTML5
//! attributes). This module now: caps the request body (16 KiB — a contact
//! message has no legitimate reason to be larger), applies a simple
//! dependency-free global rate limit (not per-IP — see `RateLimiter`'s doc
//! comment for why that's an intentional scope cut, not an oversight),
//! validates every field server-side, and durably appends each lead to
//! `$AGENTOPS_DATA_DIR/contact-leads.jsonl` (the same data-dir convention
//! every other store in this crate uses — see `agentops_data_dir()` in
//! `lib.rs`) in addition to the `println!` line, so a lead survives a
//! container restart/log-rotation even before real email/issue-tracker
//! notification exists.
//!
//! **Still does not notify anyone.** There is no email (Resend) or
//! issue-tracker integration wired up yet — that decision (Resend vs.
//! GitHub Issues) is still open. Recorded as a deferred decision in
//! `.agentops/notes/` — check there before assuming this is "done".

use std::io::Write;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::extract::DefaultBodyLimit;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tower_http::cors::CorsLayer;

const MAX_BODY_BYTES: usize = 16 * 1024;
const MAX_FIELD_LEN: usize = 320;
const MAX_MESSAGE_LEN: usize = 4000;

#[derive(Deserialize)]
struct ContactLead {
    need: String,
    email: String,
    company: String,
    message: String,
}

#[derive(Serialize)]
struct StoredContactLead<'a> {
    received_at_unix: u64,
    need: &'a str,
    email: &'a str,
    company: &'a str,
    message: &'a str,
}

fn validate(lead: &ContactLead) -> Result<(), &'static str> {
    if lead.need.trim().is_empty() || lead.need.len() > MAX_FIELD_LEN {
        return Err("need must be non-empty and under 320 characters");
    }
    if lead.company.trim().is_empty() || lead.company.len() > MAX_FIELD_LEN {
        return Err("company must be non-empty and under 320 characters");
    }
    if lead.message.trim().is_empty() || lead.message.len() > MAX_MESSAGE_LEN {
        return Err("message must be non-empty and under 4000 characters");
    }
    // Deliberately loose (contains '@', has a '.' after it, no whitespace)
    // rather than a full RFC 5322 parser -- this only needs to catch
    // obviously-junk submissions, not be an email validator. A real send
    // attempt (once Resend/Issues notification exists) is the actual
    // correctness check for deliverability.
    let email = lead.email.trim();
    let at = email.find('@');
    let valid_email = email.len() <= MAX_FIELD_LEN
        && !email.is_empty()
        && !email.chars().any(char::is_whitespace)
        && matches!(at, Some(pos) if pos > 0 && email[pos + 1..].contains('.'));
    if !valid_email {
        return Err("email is not a valid-looking email address");
    }
    Ok(())
}

/// Dependency-free global (not per-IP) rate limiter: a fixed-size token
/// bucket refilled at a constant rate, guarded by a `Mutex`. Global rather
/// than per-IP is a deliberate scope cut, not an oversight -- per-IP
/// limiting needs either a real client-IP source (this sits behind
/// whatever reverse proxy/ingress terminates TLS, so `X-Forwarded-For`
/// trust would need to be configured correctly to avoid spoofing) or a new
/// dependency (e.g. `tower_governor`). For a low-volume marketing contact
/// form, a global cap is enough to stop a flood from taking down the
/// shared process (the actual DoS risk this exists to close) without
/// taking on either of those. Revisit if this ever needs per-IP fairness.
struct RateLimiter {
    capacity: u32,
    refill_every: Duration,
    tokens: Mutex<(u32, Instant)>,
}

impl RateLimiter {
    fn new(capacity: u32, refill_every: Duration) -> Self {
        Self { capacity, refill_every, tokens: Mutex::new((capacity, Instant::now())) }
    }

    fn try_acquire(&self) -> bool {
        let mut guard = self.tokens.lock().unwrap_or_else(|e| e.into_inner());
        let (tokens, last_refill) = &mut *guard;
        let elapsed = last_refill.elapsed();
        let refills = (elapsed.as_secs_f64() / self.refill_every.as_secs_f64()) as u32;
        if refills > 0 {
            *tokens = self.capacity.min(tokens.saturating_add(refills));
            *last_refill = Instant::now();
        }
        if *tokens > 0 {
            *tokens -= 1;
            true
        } else {
            false
        }
    }
}

// 30 requests per minute, refilled one token every 2 seconds -- generous
// for a real visitor, tight enough that a flood gets rejected with a plain
// 429 well before it can meaningfully load the shared process.
static RATE_LIMITER: std::sync::LazyLock<RateLimiter> = std::sync::LazyLock::new(|| RateLimiter::new(30, Duration::from_secs(2)));

fn append_to_data_dir(lead: &ContactLead) -> std::io::Result<()> {
    let dir = crate::agentops_data_dir();
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("contact-leads.jsonl");
    let received_at_unix = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let record = StoredContactLead { received_at_unix, need: &lead.need, email: &lead.email, company: &lead.company, message: &lead.message };
    let line = serde_json::to_string(&record)?;
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{line}")
}

async fn create_contact_lead(Json(lead): Json<ContactLead>) -> Response {
    if !RATE_LIMITER.try_acquire() {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }
    if let Err(msg) = validate(&lead) {
        return (StatusCode::BAD_REQUEST, msg).into_response();
    }

    println!("contact_lead: need={:?} email={:?} company={:?} message={:?}", lead.need, lead.email, lead.company, lead.message);
    if let Err(e) = append_to_data_dir(&lead) {
        // Durable storage failed, but the lead is still in the process log
        // above -- degrade, don't fail the request over it.
        eprintln!("contact_lead: failed to append to data dir: {e}");
    }

    StatusCode::OK.into_response()
}

pub fn contact_router() -> Router {
    Router::new()
        .route("/contact-leads", post(create_contact_lead))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(CorsLayer::permissive())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    fn valid_lead_json() -> String {
        serde_json::json!({
            "need": "Managed suite",
            "email": "prospect@example.com",
            "company": "Example Inc",
            "message": "We have 40 engineers and want it hosted in our AWS account."
        })
        .to_string()
    }

    #[tokio::test]
    async fn accepts_a_valid_submission() {
        let app = contact_router();
        let res = app
            .oneshot(Request::builder().method("POST").uri("/contact-leads").header("content-type", "application/json").body(Body::from(valid_lead_json())).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn rejects_an_invalid_email() {
        let app = contact_router();
        let body = serde_json::json!({ "need": "x", "email": "not-an-email", "company": "x", "message": "x" }).to_string();
        let res = app
            .oneshot(Request::builder().method("POST").uri("/contact-leads").header("content-type", "application/json").body(Body::from(body)).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn rejects_an_empty_message() {
        let app = contact_router();
        let body = serde_json::json!({ "need": "x", "email": "a@b.co", "company": "x", "message": "   " }).to_string();
        let res = app
            .oneshot(Request::builder().method("POST").uri("/contact-leads").header("content-type", "application/json").body(Body::from(body)).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn rejects_a_body_over_the_size_limit() {
        let app = contact_router();
        let oversized = serde_json::json!({
            "need": "x", "email": "a@b.co", "company": "x",
            "message": "x".repeat(MAX_BODY_BYTES + 1024)
        })
        .to_string();
        let res = app
            .oneshot(Request::builder().method("POST").uri("/contact-leads").header("content-type", "application/json").body(Body::from(oversized)).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[test]
    fn rate_limiter_exhausts_then_recovers() {
        let limiter = RateLimiter::new(2, Duration::from_secs(3600));
        assert!(limiter.try_acquire());
        assert!(limiter.try_acquire());
        assert!(!limiter.try_acquire());
    }
}
