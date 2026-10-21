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

#[cfg(test)]
mod tests {
    use super::*;

    /// D03 budgets are independent clocks. The accept cutoff is the inline
    /// `Duration::from_secs(60)` in `prune` (`>`), not `SESSION_TTL`.
    #[test]
    fn accept_and_fingerprint_failure_budgets_expire_independently() {
        assert_eq!(MAX_ACCEPTS_PER_MINUTE, 10);
        assert_eq!(MAX_FAILURES_PER_FINGERPRINT, 5);
        assert_eq!(FAILURE_WINDOW, Duration::from_secs(15 * 60));

        let t0 = Instant::now();
        let mut rates = RateLimits::default();

        for _ in 0..MAX_FAILURES_PER_FINGERPRINT - 1 {
            rates.record_failure("fp-a", t0);
            assert!(rates.allow_failure_budget("fp-a", t0));
        }
        rates.record_failure("fp-a", t0);
        assert!(!rates.allow_failure_budget("fp-a", t0));
        // A second fingerprint still has a full failure budget.
        assert!(rates.allow_failure_budget("fp-b", t0));
        for _ in 0..MAX_FAILURES_PER_FINGERPRINT {
            rates.record_failure("fp-b", t0);
        }
        assert!(!rates.allow_failure_budget("fp-b", t0));
        assert!(rates.allow_failure_budget("fp-c", t0));

        // Filling either failure bucket does not spend accept slots.
        for _ in 0..MAX_ACCEPTS_PER_MINUTE {
            assert!(rates.allow_accept(t0));
        }
        assert!(!rates.allow_accept(t0));
        assert!(!rates.allow_failure_budget("fp-a", t0));
        assert!(rates.allow_failure_budget("fp-c", t0));

        // Exact 60s is still inside the accept window (`>`).
        let accept_closed = t0 + Duration::from_secs(60);
        assert!(!rates.allow_accept(accept_closed));
        assert!(!rates.allow_failure_budget("fp-a", accept_closed));

        let accept_open = accept_closed + Duration::from_nanos(1);
        assert!(
            rates.allow_accept(accept_open),
            "accepts older than 60s must expire without a sleep"
        );
        assert!(
            !rates.allow_failure_budget("fp-a", accept_open),
            "accept expiry must not clear the 15-minute fingerprint budget"
        );
        assert!(rates.allow_failure_budget("fp-c", accept_open));

        let failure_closed = t0 + FAILURE_WINDOW;
        assert!(!rates.allow_failure_budget("fp-a", failure_closed));
        assert!(!rates.allow_failure_budget("fp-b", failure_closed));

        let failure_open = failure_closed + Duration::from_nanos(1);
        assert!(rates.allow_failure_budget("fp-a", failure_open));
        assert!(rates.allow_failure_budget("fp-b", failure_open));
        assert!(rates.allow_failure_budget("fp-c", failure_open));
        // The accept recorded at `accept_open` is itself outside 60s by now.
        assert!(rates.allow_accept(failure_open));
    }
}
