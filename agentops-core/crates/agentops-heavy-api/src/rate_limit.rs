//! In-process fixed-window rate limiting for the unauthenticated auth
//! routes (login, 2FA, signup) and device approval. Kept in memory: the
//! server already runs as a single instance (SQLite platform stores), and
//! a restart resetting the counters is acceptable for a brute-force brake.
//!
//! `tower_governor` was considered and not used: it needs connect info and
//! a custom key extractor here anyway (its `SmartIpKeyExtractor` trusts the
//! first `X-Forwarded-For` hop, which clients control behind nginx), and
//! these limits are per account and per challenge as well as per address.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Once the map holds this many keys, expired windows are dropped on the
/// next check, so memory stays bounded by recent traffic.
const PRUNE_THRESHOLD: usize = 10_000;

#[derive(Default)]
pub(crate) struct FixedWindowLimiter {
    windows: Mutex<HashMap<String, (u32, Instant)>>,
}

impl FixedWindowLimiter {
    /// Counts one attempt against `key`. Returns `Err(retry_after)` once
    /// `max` attempts have been made within the current `window`.
    pub(crate) fn check(&self, key: &str, max: u32, window: Duration) -> Result<(), Duration> {
        let now = Instant::now();
        let mut windows = self.windows.lock().unwrap();
        if windows.len() >= PRUNE_THRESHOLD {
            windows.retain(|_, (_, started)| now.duration_since(*started) < window);
        }
        let entry = windows.entry(key.to_string()).or_insert((0, now));
        if now.duration_since(entry.1) >= window {
            *entry = (0, now);
        }
        if entry.0 >= max {
            return Err(window.saturating_sub(now.duration_since(entry.1)));
        }
        entry.0 += 1;
        Ok(())
    }

    /// Forgets `key`, e.g. after the thing it counted (a 2FA challenge) is gone.
    pub(crate) fn clear(&self, key: &str) {
        self.windows.lock().unwrap().remove(key);
    }
}

/// A `429 Too Many Requests` response with `Retry-After` in whole seconds.
pub(crate) fn too_many_requests(retry_after: Duration) -> axum::response::Response {
    use axum::response::IntoResponse;
    let secs = retry_after.as_secs().max(1);
    (
        axum::http::StatusCode::TOO_MANY_REQUESTS,
        [(axum::http::header::RETRY_AFTER, secs.to_string())],
        axum::Json(serde_json::json!({ "error": format!("too many attempts; retry in {secs}s") })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_up_to_max_then_refuses_until_the_window_ends() {
        let limiter = FixedWindowLimiter::default();
        let window = Duration::from_millis(200);
        for _ in 0..3 {
            assert!(limiter.check("k", 3, window).is_ok());
        }
        let retry = limiter.check("k", 3, window).unwrap_err();
        assert!(retry <= window);
        assert!(limiter.check("other", 3, window).is_ok(), "keys are independent");
        std::thread::sleep(window + Duration::from_millis(20));
        assert!(limiter.check("k", 3, window).is_ok(), "a new window starts after the old one ends");
    }

    #[test]
    fn clear_resets_a_key() {
        let limiter = FixedWindowLimiter::default();
        let window = Duration::from_secs(60);
        assert!(limiter.check("k", 1, window).is_ok());
        assert!(limiter.check("k", 1, window).is_err());
        limiter.clear("k");
        assert!(limiter.check("k", 1, window).is_ok());
    }
}
