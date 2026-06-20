//! Per-account failed-login lockout with temporary exponential backoff.
//!
//! The per-IP [`RateLimiter`](crate::security::rate_limit::RateLimiter) limits
//! brute force from a single source, but an attacker rotating IPs can still get
//! unlimited guesses against a single account (notably the `root` operator
//! account). This module adds an account-keyed temporary lockout: after a few
//! consecutive failures the account is locked for a backoff interval that grows
//! exponentially up to a hard cap, then auto-clears.
//!
//! ## CRITICAL SAFETY CONSTRAINT
//!
//! The lockout is **temporary and self-clearing** — never permanent. `root` is
//! the operator's ONLY recovery account; a permanent lock would brick admin
//! access to the router. Backoff is capped at [`MAX_BACKOFF`] (a few minutes)
//! and the lock expiry always lies in the (near) future, so an attacker is
//! merely slowed, and a legitimate operator who walks away regains access after
//! at most `MAX_BACKOFF`. A successful login clears the counter immediately.
//!
//! ## Interface (called by the API auth handler — see PENDING-CHANGES)
//!
//! ```ignore
//! if let Some(remaining) = lockout.check_locked("root").await {
//!     // reject with 429 / "try again in {remaining:?}"
//! }
//! // ... verify credentials ...
//! if ok { lockout.record_success("root").await; }
//! else  { lockout.record_failure("root").await; }
//! ```

use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// Consecutive failures tolerated before the first lockout kicks in.
const FREE_ATTEMPTS: u32 = 5;

/// Base backoff applied at the first lockout (doubles each subsequent failure).
const BASE_BACKOFF: Duration = Duration::from_secs(5);

/// Hard cap on the backoff interval. Keeps the lock temporary so `root` can
/// always recover. A few minutes is plenty to defeat online brute force.
const MAX_BACKOFF: Duration = Duration::from_secs(300);

/// State tracked per account key.
#[derive(Debug, Clone)]
struct Attempts {
    /// Count of consecutive failures (reset on success).
    failures: u32,
    /// Instant after which the account is no longer locked, if currently locked.
    locked_until: Option<Instant>,
}

/// Account-keyed failed-login lockout with temporary exponential backoff.
///
/// Cheap to construct; holds an in-memory map behind an async `RwLock`,
/// mirroring [`RateLimiter`](crate::security::rate_limit::RateLimiter).
///
/// NOTE: the public methods are currently unused in-tree — the API auth handler
/// wiring is logged in `docs/PENDING-CHANGES.md` ("[SECURITY → BACKEND] login
/// lockout wiring"). `#[allow(dead_code)]` keeps `-D warnings` green until the
/// backend session calls these; remove the allow once the call sites land.
#[derive(Default)]
#[allow(dead_code)]
pub struct LoginLockout {
    accounts: RwLock<HashMap<String, Attempts>>,
}

#[allow(dead_code)]
impl LoginLockout {
    /// Create an empty lockout tracker.
    pub fn new() -> Self {
        Self::default()
    }

    /// Compute the backoff for the Nth lockout (1-based steps past the free
    /// attempts), exponential with a hard cap. Pure function — easy to test.
    fn backoff_for(locked_steps: u32) -> Duration {
        // locked_steps == 1 -> BASE_BACKOFF, then doubles, capped at MAX_BACKOFF.
        let shift = locked_steps.saturating_sub(1).min(31);
        let secs = BASE_BACKOFF
            .as_secs()
            .saturating_mul(1u64 << shift)
            .min(MAX_BACKOFF.as_secs());
        Duration::from_secs(secs)
    }

    /// If `account` is currently locked, return the remaining lockout duration.
    /// Returns `None` if the account may attempt a login now. Expired locks are
    /// treated as cleared (the temporary-lock guarantee).
    pub async fn check_locked(&self, account: &str) -> Option<Duration> {
        let now = Instant::now();
        let accounts = self.accounts.read().await;
        let entry = accounts.get(account)?;
        let locked_until = entry.locked_until?;
        if locked_until > now {
            Some(locked_until - now)
        } else {
            None
        }
    }

    /// Record a failed login for `account`. After [`FREE_ATTEMPTS`] consecutive
    /// failures the account is locked for an exponentially-growing (but capped)
    /// interval. Always self-clearing — never a permanent lock.
    pub async fn record_failure(&self, account: &str) {
        let now = Instant::now();
        let mut accounts = self.accounts.write().await;
        let entry = accounts.entry(account.to_string()).or_insert(Attempts {
            failures: 0,
            locked_until: None,
        });

        entry.failures = entry.failures.saturating_add(1);

        if entry.failures > FREE_ATTEMPTS {
            let locked_steps = entry.failures - FREE_ATTEMPTS;
            let backoff = Self::backoff_for(locked_steps);
            entry.locked_until = Some(now + backoff);
        }
    }

    /// Clear all failure state for `account` on a successful login.
    pub async fn record_success(&self, account: &str) {
        let mut accounts = self.accounts.write().await;
        accounts.remove(account);
    }

    /// Purge entries whose lock has expired and whose failure count is moot.
    /// Optional housekeeping; safe to call periodically.
    #[allow(dead_code)]
    pub async fn cleanup(&self) {
        let now = Instant::now();
        let mut accounts = self.accounts.write().await;
        accounts.retain(|_, a| match a.locked_until {
            Some(until) => until > now,
            None => a.failures > 0,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn free_attempts_do_not_lock() {
        let lockout = LoginLockout::new();
        for _ in 0..FREE_ATTEMPTS {
            lockout.record_failure("root").await;
            assert!(
                lockout.check_locked("root").await.is_none(),
                "must not lock within the free-attempt budget"
            );
        }
    }

    #[tokio::test]
    async fn lock_engages_after_free_attempts() {
        let lockout = LoginLockout::new();
        for _ in 0..=FREE_ATTEMPTS {
            lockout.record_failure("root").await;
        }
        let remaining = lockout.check_locked("root").await;
        assert!(remaining.is_some(), "account must be locked past the budget");
        assert!(remaining.unwrap() <= BASE_BACKOFF);
    }

    #[tokio::test]
    async fn backoff_grows_then_caps() {
        // Pure backoff curve: exponential, capped at MAX_BACKOFF, never above.
        assert_eq!(LoginLockout::backoff_for(1), BASE_BACKOFF);
        assert_eq!(
            LoginLockout::backoff_for(2),
            Duration::from_secs(BASE_BACKOFF.as_secs() * 2)
        );
        // Far out: must saturate to the cap, never exceed it (temporary lock).
        assert_eq!(LoginLockout::backoff_for(1000), MAX_BACKOFF);
        for steps in 1..50 {
            assert!(
                LoginLockout::backoff_for(steps) <= MAX_BACKOFF,
                "backoff must never exceed the cap (no permanent lock)"
            );
        }
    }

    #[tokio::test]
    async fn success_clears_the_counter() {
        let lockout = LoginLockout::new();
        for _ in 0..=FREE_ATTEMPTS {
            lockout.record_failure("root").await;
        }
        assert!(lockout.check_locked("root").await.is_some());

        lockout.record_success("root").await;
        assert!(
            lockout.check_locked("root").await.is_none(),
            "successful login must clear the lockout"
        );

        // And the failure budget is fully reset afterwards.
        for _ in 0..FREE_ATTEMPTS {
            lockout.record_failure("root").await;
            assert!(lockout.check_locked("root").await.is_none());
        }
    }

    #[tokio::test]
    async fn expired_lock_auto_clears() {
        // A lock in the past must read as unlocked — temporary, self-clearing.
        let lockout = LoginLockout::new();
        {
            let mut accounts = lockout.accounts.write().await;
            accounts.insert(
                "root".to_string(),
                Attempts {
                    failures: FREE_ATTEMPTS + 1,
                    locked_until: Some(Instant::now() - Duration::from_secs(1)),
                },
            );
        }
        assert!(
            lockout.check_locked("root").await.is_none(),
            "an expired lock must auto-clear so root can recover"
        );
    }

    #[tokio::test]
    async fn accounts_are_independent() {
        let lockout = LoginLockout::new();
        for _ in 0..=FREE_ATTEMPTS {
            lockout.record_failure("root").await;
        }
        assert!(lockout.check_locked("root").await.is_some());
        assert!(
            lockout.check_locked("admin").await.is_none(),
            "locking one account must not lock another"
        );
    }
}
