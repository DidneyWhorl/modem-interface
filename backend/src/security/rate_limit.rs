//! Per-IP sliding window rate limiter.
//!
//! Protects login, setup, and general API endpoints from brute-force
//! and abuse. Uses in-memory buckets with automatic cleanup.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::net::IpAddr;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

use crate::hardware::RateLimitConfig;

/// Category of rate-limited request.
#[derive(Debug, Clone, Copy, Hash, Eq, PartialEq)]
pub enum RateCategory {
    /// Login attempts: strict limit (default 5 per 15 min).
    Login,
    /// Setup attempts: very strict (default 3 per 60 min).
    Setup,
    /// General API requests: generous (default 100 per min).
    General,
}

/// Per-IP sliding window rate limiter.
pub struct RateLimiter {
    buckets: RwLock<HashMap<(IpAddr, RateCategory), VecDeque<Instant>>>,
    login_limit: u32,
    login_window: Duration,
    setup_limit: u32,
    setup_window: Duration,
    general_limit: u32,
    general_window: Duration,
    enabled: bool,
}

impl RateLimiter {
    pub fn new(config: &RateLimitConfig) -> Self {
        Self {
            buckets: RwLock::new(HashMap::new()),
            login_limit: config.login_max_attempts,
            login_window: Duration::from_secs(config.login_window_secs),
            setup_limit: config.setup_max_attempts,
            setup_window: Duration::from_secs(config.setup_window_secs),
            general_limit: config.general_max_requests,
            general_window: Duration::from_secs(config.general_window_secs),
            enabled: config.enabled,
        }
    }

    fn limit_for(&self, cat: RateCategory) -> (u32, Duration) {
        match cat {
            RateCategory::Login => (self.login_limit, self.login_window),
            RateCategory::Setup => (self.setup_limit, self.setup_window),
            RateCategory::General => (self.general_limit, self.general_window),
        }
    }

    /// Check and record an attempt. Returns `Ok(())` if allowed,
    /// or `Err(retry_after_secs)` if the rate limit is exceeded.
    pub async fn check(&self, ip: IpAddr, category: RateCategory) -> Result<(), u64> {
        if !self.enabled {
            return Ok(());
        }

        let (limit, window) = self.limit_for(category);
        let now = Instant::now();
        let cutoff = now.checked_sub(window).unwrap_or(now);

        let mut buckets = self.buckets.write().await;
        let entries = buckets.entry((ip, category)).or_default();

        // Remove expired entries
        while entries.front().is_some_and(|&t| t < cutoff) {
            entries.pop_front();
        }

        if entries.len() >= limit as usize {
            let retry_after = entries
                .front()
                .map(|&oldest| {
                    let expires = oldest + window;
                    if expires > now {
                        (expires - now).as_secs() + 1
                    } else {
                        1
                    }
                })
                .unwrap_or(1);
            return Err(retry_after);
        }

        entries.push_back(now);
        Ok(())
    }

    /// Purge stale entries older than any window. Called periodically.
    pub async fn cleanup(&self) {
        let mut buckets = self.buckets.write().await;
        let now = Instant::now();
        // Use 1 hour as max age (covers all windows)
        let cutoff = now.checked_sub(Duration::from_secs(3600)).unwrap_or(now);

        buckets.retain(|_, entries| {
            while entries.front().is_some_and(|&t| t < cutoff) {
                entries.pop_front();
            }
            !entries.is_empty()
        });
    }
}
