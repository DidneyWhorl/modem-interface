//! User management and storage.
//!
//! Multi-user account system with role-based access control.
//! Users are stored in a JSON file on disk. The root user is virtual
//! (never stored) and always exists as SuperAdmin, authenticating via /etc/shadow.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{info, warn};

/// Minimum length (in bytes) for a user-chosen password.
///
/// Raised from 4 to 12 (security review H3). All user-password paths
/// (setup, change-password, create-user, reset-password, CLI --reset-password)
/// enforce this floor.
pub const MIN_PASSWORD_LEN: usize = 12;

/// Returns true if `password` meets the minimum-length policy.
pub fn password_meets_min_len(password: &str) -> bool {
    password.len() >= MIN_PASSWORD_LEN
}

/// User role with hierarchical ordering.
/// ReadOnly < Admin < SuperAdmin
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    ReadOnly,
    Admin,
    SuperAdmin,
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Role::ReadOnly => write!(f, "read_only"),
            Role::Admin => write!(f, "admin"),
            Role::SuperAdmin => write!(f, "super_admin"),
        }
    }
}

/// Per-user UI profile stored server-side.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiProfile {
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default)]
    pub sidebar_collapsed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layouts: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible_panels: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view_presets: Option<serde_json::Value>,
}

fn default_theme() -> String {
    "system".to_string()
}

impl Default for UiProfile {
    fn default() -> Self {
        Self {
            theme: default_theme(),
            sidebar_collapsed: false,
            layouts: None,
            visible_panels: None,
            view_presets: None,
        }
    }
}

/// A user account.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub username: String,
    pub role: Role,
    /// Argon2id password hash. None for root (authenticates via /etc/shadow).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_hash: Option<String>,
    /// Panel restrictions. None = all panels allowed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_panels: Option<Vec<String>>,
    /// Feature restrictions. None = all features allowed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_features: Option<Vec<String>>,
    #[serde(default)]
    pub ui_profile: UiProfile,
    #[serde(default)]
    pub disabled: bool,
}

/// Public user info (no password hash).
#[derive(Debug, Clone, Serialize)]
pub struct UserInfo {
    pub username: String,
    pub role: Role,
    pub allowed_panels: Option<Vec<String>>,
    pub allowed_features: Option<Vec<String>>,
    pub disabled: bool,
}

impl From<&User> for UserInfo {
    fn from(user: &User) -> Self {
        Self {
            username: user.username.clone(),
            role: user.role,
            allowed_panels: user.allowed_panels.clone(),
            allowed_features: user.allowed_features.clone(),
            disabled: user.disabled,
        }
    }
}

/// Persistent on-disk user store.
pub struct UserStore {
    users: RwLock<HashMap<String, User>>,
    file_path: PathBuf,
}

impl UserStore {
    /// Load users from file or create empty store.
    pub async fn load(file_path: impl Into<PathBuf>) -> Self {
        let file_path = file_path.into();
        let users = match tokio::fs::read_to_string(&file_path).await {
            Ok(content) => {
                match serde_json::from_str::<Vec<User>>(&content) {
                    Ok(user_list) => {
                        let map: HashMap<String, User> = user_list
                            .into_iter()
                            .map(|u| (u.username.clone(), u))
                            .collect();
                        info!("Loaded {} user(s) from {}", map.len(), file_path.display());
                        map
                    }
                    Err(e) => {
                        warn!("Failed to parse users file {}: {e}", file_path.display());
                        HashMap::new()
                    }
                }
            }
            Err(_) => {
                info!("No users file at {}, starting fresh", file_path.display());
                HashMap::new()
            }
        };

        Self {
            users: RwLock::new(users),
            file_path,
        }
    }

    /// Get a user by username. Returns root as virtual SuperAdmin.
    pub async fn get_user(&self, username: &str) -> Option<User> {
        if username == "root" {
            return Some(Self::virtual_root());
        }
        self.users.read().await.get(username).cloned()
    }

    /// List all users (including virtual root), without password hashes.
    pub async fn list_users(&self) -> Vec<UserInfo> {
        let users = self.users.read().await;
        let mut list: Vec<UserInfo> = vec![UserInfo::from(&Self::virtual_root())];
        list.extend(
            users
                .values()
                .filter(|u| u.username != "__root_profile__")
                .map(UserInfo::from),
        );
        list.sort_by(|a, b| a.username.cmp(&b.username));
        list
    }

    /// Create a new user. Returns error if username already exists or is "root".
    pub async fn create_user(&self, user: User) -> Result<(), String> {
        if user.username == "root" {
            return Err("Cannot create user with reserved name 'root'".to_string());
        }
        if user.username.is_empty() || user.username.len() > 32 {
            return Err("Username must be 1-32 characters".to_string());
        }
        if !user
            .username
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
        {
            return Err("Username may only contain alphanumeric characters, hyphens, and underscores".to_string());
        }

        let mut users = self.users.write().await;
        if users.contains_key(&user.username) {
            return Err(format!("User '{}' already exists", user.username));
        }
        users.insert(user.username.clone(), user);
        drop(users);
        self.save().await
    }

    /// Update an existing user (non-root). Does NOT update password_hash — use dedicated method.
    pub async fn update_user(
        &self,
        username: &str,
        role: Option<Role>,
        allowed_panels: Option<Option<Vec<String>>>,
        allowed_features: Option<Option<Vec<String>>>,
        disabled: Option<bool>,
    ) -> Result<(), String> {
        if username == "root" {
            return Err("Cannot modify root user".to_string());
        }

        let mut users = self.users.write().await;
        let user = users
            .get_mut(username)
            .ok_or_else(|| format!("User '{username}' not found"))?;

        if let Some(r) = role {
            user.role = r;
        }
        if let Some(panels) = allowed_panels {
            user.allowed_panels = panels;
        }
        if let Some(features) = allowed_features {
            user.allowed_features = features;
        }
        if let Some(d) = disabled {
            user.disabled = d;
        }

        drop(users);
        self.save().await
    }

    /// Update a user's password hash.
    pub async fn set_password_hash(&self, username: &str, hash: String) -> Result<(), String> {
        if username == "root" {
            return Err("Cannot change root password here — use 'passwd' via SSH".to_string());
        }

        let mut users = self.users.write().await;
        let user = users
            .get_mut(username)
            .ok_or_else(|| format!("User '{username}' not found"))?;

        user.password_hash = Some(hash);
        drop(users);
        self.save().await
    }

    /// Update a user's UI profile.
    pub async fn update_profile(&self, username: &str, profile: UiProfile) -> Result<(), String> {
        if username == "root" {
            // Root profile is stored in a special entry
            let mut users = self.users.write().await;
            let root_profile = users.entry("__root_profile__".to_string()).or_insert(User {
                username: "__root_profile__".to_string(),
                role: Role::SuperAdmin,
                password_hash: None,
                allowed_panels: None,
                allowed_features: None,
                ui_profile: UiProfile::default(),
                disabled: true, // marker, not a real user
            });
            root_profile.ui_profile = profile;
            drop(users);
            return self.save().await;
        }

        let mut users = self.users.write().await;
        let user = users
            .get_mut(username)
            .ok_or_else(|| format!("User '{username}' not found"))?;

        user.ui_profile = profile;
        drop(users);
        self.save().await
    }

    /// Get a user's UI profile.
    pub async fn get_profile(&self, username: &str) -> Option<UiProfile> {
        if username == "root" {
            let users = self.users.read().await;
            return users
                .get("__root_profile__")
                .map(|u| u.ui_profile.clone())
                .or_else(|| Some(UiProfile::default()));
        }
        self.users
            .read()
            .await
            .get(username)
            .map(|u| u.ui_profile.clone())
    }

    /// Delete a user.
    pub async fn delete_user(&self, username: &str) -> Result<(), String> {
        if username == "root" {
            return Err("Cannot delete root user".to_string());
        }

        let mut users = self.users.write().await;
        if users.remove(username).is_none() {
            return Err(format!("User '{username}' not found"));
        }
        drop(users);
        self.save().await
    }

    /// Check if any non-root users exist.
    pub async fn has_users(&self) -> bool {
        let users = self.users.read().await;
        users.keys().any(|k| k != "__root_profile__")
    }

    /// Save users to disk.
    pub async fn save(&self) -> Result<(), String> {
        let users = self.users.read().await;
        let user_list: Vec<&User> = users.values().collect();
        let json = serde_json::to_string_pretty(&user_list)
            .map_err(|e| format!("Failed to serialize users: {e}"))?;
        drop(users);

        // Ensure parent directory exists
        if let Some(parent) = self.file_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| format!("Failed to create directory: {e}"))?;
        }

        // User store holds password hashes — write owner-only (0600).
        crate::config::write_secret_file(&self.file_path, json)
            .await
            .map_err(|e| format!("Failed to write users file: {e}"))?;

        Ok(())
    }

    /// The virtual root user (always exists, authenticates via /etc/shadow).
    fn virtual_root() -> User {
        User {
            username: "root".to_string(),
            role: Role::SuperAdmin,
            password_hash: None,
            allowed_panels: None,
            allowed_features: None,
            ui_profile: UiProfile::default(),
            disabled: false,
        }
    }

    /// Create a user directly (for migration). Skips validation.
    pub async fn create_user_unchecked(&self, user: User) {
        let mut users = self.users.write().await;
        users.insert(user.username.clone(), user);
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn password_min_len_boundary() {
        // 11 chars rejected, 12 accepted (H3).
        assert_eq!(MIN_PASSWORD_LEN, 12);
        assert!(!password_meets_min_len("")); // empty
        assert!(!password_meets_min_len("abc")); // old 4-char floor no longer enough
        assert!(!password_meets_min_len(&"a".repeat(11))); // 11 — just below
        assert!(password_meets_min_len(&"a".repeat(12))); // 12 — exactly the floor
        assert!(password_meets_min_len(&"a".repeat(20))); // comfortably above
    }

    #[tokio::test]
    async fn test_create_and_get_user() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("users.json");
        let store = UserStore::load(&path).await;

        let user = User {
            username: "testuser".to_string(),
            role: Role::ReadOnly,
            password_hash: Some("hash123".to_string()),
            allowed_panels: None,
            allowed_features: None,
            ui_profile: UiProfile::default(),
            disabled: false,
        };

        store.create_user(user).await.unwrap();

        let fetched = store.get_user("testuser").await.unwrap();
        assert_eq!(fetched.username, "testuser");
        assert_eq!(fetched.role, Role::ReadOnly);
    }

    #[tokio::test]
    async fn test_root_is_virtual() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("users.json");
        let store = UserStore::load(&path).await;

        let root = store.get_user("root").await.unwrap();
        assert_eq!(root.role, Role::SuperAdmin);
        assert!(root.password_hash.is_none());
    }

    #[tokio::test]
    async fn test_cannot_create_root() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("users.json");
        let store = UserStore::load(&path).await;

        let user = User {
            username: "root".to_string(),
            role: Role::Admin,
            password_hash: Some("hash".to_string()),
            allowed_panels: None,
            allowed_features: None,
            ui_profile: UiProfile::default(),
            disabled: false,
        };

        assert!(store.create_user(user).await.is_err());
    }

    #[tokio::test]
    async fn test_role_ordering() {
        assert!(Role::ReadOnly < Role::Admin);
        assert!(Role::Admin < Role::SuperAdmin);
    }

    #[tokio::test]
    async fn test_delete_user() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("users.json");
        let store = UserStore::load(&path).await;

        let user = User {
            username: "tobedeleted".to_string(),
            role: Role::ReadOnly,
            password_hash: Some("hash".to_string()),
            allowed_panels: None,
            allowed_features: None,
            ui_profile: UiProfile::default(),
            disabled: false,
        };

        store.create_user(user).await.unwrap();
        assert!(store.get_user("tobedeleted").await.is_some());

        store.delete_user("tobedeleted").await.unwrap();
        assert!(store.get_user("tobedeleted").await.is_none());
    }

    #[tokio::test]
    async fn test_persistence() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("users.json");

        // Create and save
        {
            let store = UserStore::load(&path).await;
            let user = User {
                username: "persist".to_string(),
                role: Role::Admin,
                password_hash: Some("hash".to_string()),
                allowed_panels: None,
                allowed_features: None,
                ui_profile: UiProfile::default(),
                disabled: false,
            };
            store.create_user(user).await.unwrap();
        }

        // Reload and verify
        {
            let store = UserStore::load(&path).await;
            let user = store.get_user("persist").await.unwrap();
            assert_eq!(user.role, Role::Admin);
        }
    }

    #[tokio::test]
    async fn test_update_user() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("users.json");
        let store = UserStore::load(&path).await;

        let user = User {
            username: "updateme".to_string(),
            role: Role::ReadOnly,
            password_hash: Some("hash".to_string()),
            allowed_panels: None,
            allowed_features: None,
            ui_profile: UiProfile::default(),
            disabled: false,
        };

        store.create_user(user).await.unwrap();

        store
            .update_user(
                "updateme",
                Some(Role::Admin),
                Some(Some(vec!["signal".to_string()])),
                None,
                Some(true),
            )
            .await
            .unwrap();

        let updated = store.get_user("updateme").await.unwrap();
        assert_eq!(updated.role, Role::Admin);
        assert!(updated.disabled);
        assert_eq!(
            updated.allowed_panels,
            Some(vec!["signal".to_string()])
        );
    }

    #[tokio::test]
    async fn test_username_validation() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("users.json");
        let store = UserStore::load(&path).await;

        // Empty username
        let user = User {
            username: "".to_string(),
            role: Role::ReadOnly,
            password_hash: Some("hash".to_string()),
            allowed_panels: None,
            allowed_features: None,
            ui_profile: UiProfile::default(),
            disabled: false,
        };
        assert!(store.create_user(user).await.is_err());

        // Invalid characters
        let user = User {
            username: "bad user!".to_string(),
            role: Role::ReadOnly,
            password_hash: Some("hash".to_string()),
            allowed_panels: None,
            allowed_features: None,
            ui_profile: UiProfile::default(),
            disabled: false,
        };
        assert!(store.create_user(user).await.is_err());
    }

    #[tokio::test]
    async fn test_list_users_includes_root() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("users.json");
        let store = UserStore::load(&path).await;

        let user = User {
            username: "admin".to_string(),
            role: Role::Admin,
            password_hash: Some("hash".to_string()),
            allowed_panels: None,
            allowed_features: None,
            ui_profile: UiProfile::default(),
            disabled: false,
        };
        store.create_user(user).await.unwrap();

        let list = store.list_users().await;
        assert!(list.iter().any(|u| u.username == "root"));
        assert!(list.iter().any(|u| u.username == "admin"));
    }
}
