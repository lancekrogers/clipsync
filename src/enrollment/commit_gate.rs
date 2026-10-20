//! Linearize cancel vs durable trust commit (no best-effort rollback).
//!
//! States:
//! - Open: cancel or commit may still win
//! - Cancelled: cancel won before publish — do not write
//! - Committing: under flock, about to / did save — cancel must not revoke
//! - Committed: durable write published — never return Cancelled, never remove pin

use std::{
    sync::atomic::{AtomicU8, Ordering},
    time::Instant,
};

const OPEN: u8 = 0;
const CANCELLED: u8 = 1;
const COMMITTING: u8 = 2;
const COMMITTED: u8 = 3;

#[derive(Debug)]
pub struct CommitGate {
    state: AtomicU8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitAbort {
    Cancelled,
    Expired,
}

impl Default for CommitGate {
    fn default() -> Self {
        Self::new()
    }
}

impl CommitGate {
    pub fn new() -> Self {
        Self {
            state: AtomicU8::new(OPEN),
        }
    }

    /// Cancel wins only while still Open. Returns true if this call cancelled.
    pub fn try_cancel(&self) -> bool {
        self.state
            .compare_exchange(OPEN, CANCELLED, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    pub fn is_committed(&self) -> bool {
        matches!(self.state.load(Ordering::SeqCst), COMMITTED | COMMITTING)
    }

    pub fn is_cancelled(&self) -> bool {
        self.state.load(Ordering::SeqCst) == CANCELLED
    }

    /// Under auth flock, before mutating keys: claim commit or abort.
    pub fn begin_commit(&self, expires_at: Instant) -> Result<(), CommitAbort> {
        if Instant::now() >= expires_at {
            let _ =
                self.state
                    .compare_exchange(OPEN, CANCELLED, Ordering::SeqCst, Ordering::SeqCst);
            return Err(CommitAbort::Expired);
        }
        match self
            .state
            .compare_exchange(OPEN, COMMITTING, Ordering::SeqCst, Ordering::SeqCst)
        {
            Ok(_) => Ok(()),
            Err(CANCELLED) => Err(CommitAbort::Cancelled),
            Err(COMMITTED) | Err(COMMITTING) => Ok(()),
            Err(_) => Err(CommitAbort::Cancelled),
        }
    }

    /// After successful durable save (still ideally under flock).
    pub fn finish_commit(&self) {
        self.state.store(COMMITTED, Ordering::SeqCst);
    }

    /// Save failed after begin_commit — reopen for a clean cancel path.
    pub fn abort_commit_attempt(&self) {
        let _ = self
            .state
            .compare_exchange(COMMITTING, OPEN, Ordering::SeqCst, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn cancel_first_blocks_commit() {
        let g = CommitGate::new();
        assert!(g.try_cancel());
        assert!(matches!(
            g.begin_commit(Instant::now() + Duration::from_secs(60)),
            Err(CommitAbort::Cancelled)
        ));
    }

    #[test]
    fn commit_first_blocks_cancel() {
        let g = CommitGate::new();
        g.begin_commit(Instant::now() + Duration::from_secs(60))
            .unwrap();
        assert!(!g.try_cancel());
        g.finish_commit();
        assert!(g.is_committed());
        assert!(!g.try_cancel());
    }

    #[test]
    fn deadline_checked_at_begin_commit() {
        let g = CommitGate::new();
        assert!(matches!(
            g.begin_commit(Instant::now() - Duration::from_millis(1)),
            Err(CommitAbort::Expired)
        ));
    }
}
