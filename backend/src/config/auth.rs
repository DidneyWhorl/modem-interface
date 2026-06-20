//! Authentication configuration.

use serde::{Deserialize, Serialize};

/// Authentication configuration, persisted in config.toml under `[auth]`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    /// Legacy single-user password hash (v0.3.0). Migrated to users.json on startup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_hash: Option<String>,

    /// Whether authentication is required.
    #[serde(default = "default_auth_enabled")]
    pub enabled: bool,

    /// Session expiry in hours.
    #[serde(default = "default_session_hours")]
    pub session_expiry_hours: u64,

    /// Path to the users JSON file.
    #[serde(default = "default_users_file")]
    pub users_file: String,
}

fn default_auth_enabled() -> bool {
    true
}

fn default_session_hours() -> u64 {
    24
}

fn default_users_file() -> String {
    "/etc/modem-interface/users.json".to_string()
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            password_hash: None,
            enabled: default_auth_enabled(),
            session_expiry_hours: default_session_hours(),
            users_file: default_users_file(),
        }
    }
}
