//! In-memory session store for authentication.
//!
//! Sessions carry user identity (username + role) and expiry tracking.

use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

use super::users::Role;

/// A session token (hex-encoded 256-bit random value).
pub type SessionToken = String;

/// Session info returned on successful validation.
#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub username: String,
    pub role: Role,
}

struct Session {
    username: String,
    role: Role,
    expires_at: Instant,
}

/// In-memory session store with automatic expiry.
pub struct SessionStore {
    sessions: RwLock<HashMap<SessionToken, Session>>,
    default_expiry: Duration,
}

impl SessionStore {
    /// Create a new session store with the given expiry duration.
    pub fn new(expiry_hours: u64) -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            default_expiry: Duration::from_secs(expiry_hours * 3600),
        }
    }

    /// Create a new session for a user, returning the token.
    pub async fn create(&self, username: String, role: Role) -> SessionToken {
        use rand::Rng;
        let bytes: [u8; 32] = rand::thread_rng().gen();
        let token: String = bytes.iter().fold(String::with_capacity(64), |mut s, b| {
            use std::fmt::Write;
            let _ = write!(s, "{b:02x}");
            s
        });

        let session = Session {
            username,
            role,
            expires_at: Instant::now() + self.default_expiry,
        };

        self.sessions.write().await.insert(token.clone(), session);
        token
    }

    /// Validate a token. Returns session info if valid and not expired.
    pub async fn validate(&self, token: &str) -> Option<SessionInfo> {
        let sessions = self.sessions.read().await;
        match sessions.get(token) {
            Some(session) if Instant::now() < session.expires_at => Some(SessionInfo {
                username: session.username.clone(),
                role: session.role,
            }),
            _ => None,
        }
    }

    /// Remove a session (logout).
    pub async fn remove(&self, token: &str) {
        self.sessions.write().await.remove(token);
    }

    /// Purge expired sessions. Called periodically by background task.
    pub async fn purge_expired(&self) {
        let now = Instant::now();
        self.sessions
            .write()
            .await
            .retain(|_, s| now < s.expires_at);
    }
}
