//! Enrollment rate limits and expiry (D03).
use std::{
    collections::{HashMap, VecDeque},
    time::{Duration, Instant},
};

pub const SESSION_TTL: Duration = Duration::from_secs(5 * 60);
pub const MAX_CONCURRENT_SESSIONS: usize = 3;
pub const MAX_ACCEPTS_PER_MINUTE: usize = 10;
pub const MAX_FAILURES_PER_FINGERPRINT: usize = 5;
pub const FAILURE_WINDOW: Duration = Duration::from_secs(15 * 60);
/// Bound retained terminal session history (Failed/Cancelled/Expired/…).
pub const MAX_TERMINAL_SESSIONS: usize = 32;
/// Bound retained consumed session IDs for replay rejection.
pub const MAX_CONSUMED_IDS: usize = 128;

#[derive(Debug, Default)]
pub struct RateLimits {
    accepts: VecDeque<Instant>,
    failures: HashMap<String, VecDeque<Instant>>,
}

impl RateLimits {
    pub fn prune(&mut self, now: Instant) {
        while self
            .accepts
            .front()
            .is_some_and(|t| now.duration_since(*t) > Duration::from_secs(60))
        {
            self.accepts.pop_front();
        }
        self.failures.retain(|_, times| {
            while times
                .front()
                .is_some_and(|t| now.duration_since(*t) > FAILURE_WINDOW)
            {
                times.pop_front();
            }
            !times.is_empty()
        });
    }

    pub fn allow_accept(&mut self, now: Instant) -> bool {
        self.prune(now);
        if self.accepts.len() >= MAX_ACCEPTS_PER_MINUTE {
            return false;
        }
        self.accepts.push_back(now);
        true
    }

    pub fn allow_failure_budget(&mut self, fingerprint: &str, now: Instant) -> bool {
        self.prune(now);
        let times = self.failures.entry(fingerprint.to_string()).or_default();
        while times
            .front()
            .is_some_and(|t| now.duration_since(*t) > FAILURE_WINDOW)
        {
            times.pop_front();
        }
        times.len() < MAX_FAILURES_PER_FINGERPRINT
    }

    pub fn record_failure(&mut self, fingerprint: &str, now: Instant) {
        self.prune(now);
        self.failures
            .entry(fingerprint.to_string())
            .or_default()
            .push_back(now);
    }
}
