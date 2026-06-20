//! Short-lived, single-use WebSocket authentication tokens.
//!
//! Tokens are issued via `POST /api/auth/ws-token` (requires session cookie)
//! and consumed as the first message on a WebSocket connection.
//! This replaces cookie-based auth on the WS upgrade, which is blocked by
//! `SameSite=Strict` in cross-origin deployments.

use std::collections::HashMap;
use std::fmt::Write;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

use super::users::Role;

/// TTL for WebSocket tokens.
const WS_TOKEN_TTL: Duration = Duration::from_secs(30);

/// Maximum tokens per session (username) in a sliding 60-second window.
const MAX_TOKENS_PER_SESSION: usize = 10;

/// Rate limit window for per-session token issuance.
const RATE_LIMIT_WINDOW: Duration = Duration::from_secs(60);

/// Identity recovered from a consumed WebSocket token.
#[derive(Debug, Clone)]
pub struct WsTokenInfo {
    pub username: String,
    #[allow(dead_code)]
    pub role: Role,
}

/// Internal token entry.
struct WsTokenEntry {
    username: String,
    role: Role,
    created_at: Instant,
    used: bool,
}

/// In-memory store for single-use WebSocket authentication tokens.
pub struct WsTokenStore {
    tokens: RwLock<HashMap<String, WsTokenEntry>>,
}

impl WsTokenStore {
    /// Create an empty token store.
    pub fn new() -> Self {
        Self {
            tokens: RwLock::new(HashMap::new()),
        }
    }

    /// Create a new single-use token for the given session identity.
    ///
    /// Returns `Ok(token)` on success, or `Err(retry_after_secs)` if the
    /// per-session rate limit (10 tokens per 60 seconds) is exceeded.
    pub async fn create(&self, username: String, role: Role) -> Result<String, u64> {
        let now = Instant::now();
        let cutoff = now.checked_sub(RATE_LIMIT_WINDOW).unwrap_or(now);

        let mut tokens = self.tokens.write().await;

        // Per-session rate limit: count recent tokens for this username
        let recent_count = tokens
            .values()
            .filter(|e| e.username == username && e.created_at >= cutoff)
            .count();

        if recent_count >= MAX_TOKENS_PER_SESSION {
            let oldest_in_window = tokens
                .values()
                .filter(|e| e.username == username && e.created_at >= cutoff)
                .map(|e| e.created_at)
                .min();
            let retry_after = oldest_in_window
                .map(|oldest| {
                    let expires = oldest + RATE_LIMIT_WINDOW;
                    if expires > now {
                        (expires - now).as_secs() + 1
                    } else {
                        1
                    }
                })
                .unwrap_or(1);
            return Err(retry_after);
        }

        // Generate 32-byte random token (64 hex chars)
        use rand::Rng;
        let bytes: [u8; 32] = rand::thread_rng().gen();
        let token: String = bytes.iter().fold(String::with_capacity(64), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        });

        tokens.insert(
            token.clone(),
            WsTokenEntry {
                username,
                role,
                created_at: now,
                used: false,
            },
        );

        Ok(token)
    }

    /// Validate and consume a token (single-use).
    ///
    /// Returns the associated identity if the token exists, is not expired,
    /// and has not already been used. The token is marked used atomically
    /// under a write lock to prevent races.
    pub async fn validate_and_consume(&self, token: &str) -> Option<WsTokenInfo> {
        let now = Instant::now();
        let mut tokens = self.tokens.write().await;

        let entry = tokens.get_mut(token)?;

        if entry.used {
            return None;
        }

        if now.duration_since(entry.created_at) > WS_TOKEN_TTL {
            return None;
        }

        entry.used = true;

        Some(WsTokenInfo {
            username: entry.username.clone(),
            role: entry.role,
        })
    }

    /// Purge expired and used tokens. Called periodically by background task.
    pub async fn purge_expired(&self) {
        let now = Instant::now();
        self.tokens
            .write()
            .await
            .retain(|_, entry| now.duration_since(entry.created_at) <= WS_TOKEN_TTL);
    }
}
