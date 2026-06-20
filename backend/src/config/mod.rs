//! Configuration management.
//!
//! Handles loading and saving persistent configuration from/to disk.

pub mod apn_profiles;
pub mod auth;
pub mod env;
pub mod portal;
pub mod sim_slots;
pub mod wan;

pub use env::EnvConfig;
pub use env::current_env;
pub use portal::PortalConfig;

use std::path::{Path, PathBuf};
use tokio::fs;
use tracing::{info, warn};

use crate::hardware::AppConfig;

/// Default configuration file path on OpenWRT.
const DEFAULT_CONFIG_PATH: &str = "/etc/modem-interface/config.toml";

/// Set 0600 (owner read/write only) on a file. Unix only; no-op elsewhere.
///
/// The service runs as root on the router; secret-bearing files (password
/// hashes, license keys, device token, config holding auth/tls secrets) must
/// not follow the umask default (typically 0644, world-readable).
#[cfg(unix)]
fn restrict_permissions(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

/// Write `contents` to `path` 0600-restricted, closing the TOCTOU window.
///
/// On unix we create the file already at mode 0600 (`create_new` + `mode`),
/// so it is never world-readable even momentarily — unlike a write-then-chmod
/// sequence, which leaves a brief umask-perms window. If the file already
/// exists, we truncate-write into it and then re-assert 0600 (an existing
/// secret file is already owner-only from its original create). On non-unix
/// targets this is a plain write (the dev machine doesn't ship secrets).
#[cfg(unix)]
fn write_secret_file_sync(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    // Fast path: file does not exist yet — create it already-restricted so the
    // secret bytes are never visible at umask perms.
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
    {
        Ok(mut f) => f.write_all(contents),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            // File already exists (and was created 0600 originally). Overwrite
            // in place, then re-assert 0600 in case perms drifted.
            std::fs::write(path, contents)?;
            restrict_permissions(path)
        }
        Err(e) => Err(e),
    }
}

/// Write `contents` to `path`, restricted to 0600 (owner-only) on unix.
///
/// Use for any file that holds a secret. On non-unix targets this is a plain
/// write (the dev machine doesn't ship secrets). Async variant for
/// `tokio::fs`-based call sites.
pub async fn write_secret_file(
    path: impl AsRef<Path>,
    contents: impl AsRef<[u8]>,
) -> std::io::Result<()> {
    let path = path.as_ref();
    #[cfg(unix)]
    {
        write_secret_file_sync(path, contents.as_ref())
    }
    #[cfg(not(unix))]
    {
        fs::write(path, contents).await
    }
}

/// Blocking variant of [`write_secret_file`] for `std::fs`-based call sites.
pub fn write_secret_file_blocking(
    path: impl AsRef<Path>,
    contents: impl AsRef<[u8]>,
) -> std::io::Result<()> {
    let path = path.as_ref();
    #[cfg(unix)]
    {
        write_secret_file_sync(path, contents.as_ref())
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, contents)
    }
}

/// Get the configuration file path.
pub fn config_path() -> PathBuf {
    std::env::var("CONFIG_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(DEFAULT_CONFIG_PATH))
}

/// Load configuration from disk, or return defaults if not found.
pub async fn load_config() -> AppConfig {
    let path = config_path();

    match load_config_from(&path).await {
        Ok(config) => {
            info!("Loaded configuration from {}", path.display());
            config
        }
        Err(e) => {
            warn!(
                "Failed to load config from {}: {}, using defaults",
                path.display(),
                e
            );
            AppConfig::default()
        }
    }
}

/// Load configuration from a specific path.
pub async fn load_config_from(path: &Path) -> anyhow::Result<AppConfig> {
    let content = fs::read_to_string(path).await?;
    let config: AppConfig = toml::from_str(&content)?;
    Ok(config)
}

/// Save configuration to disk.
pub async fn save_config(config: &AppConfig) -> anyhow::Result<()> {
    let path = config_path();
    save_config_to(config, &path).await
}

/// Save configuration to a specific path.
pub async fn save_config_to(config: &AppConfig, path: &Path) -> anyhow::Result<()> {
    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await?;
    }

    let content = toml::to_string_pretty(config)?;
    // config.toml holds auth/portal/tls config — write owner-only (0600).
    write_secret_file(path, content).await?;

    info!("Saved configuration to {}", path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_save_load_config() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");

        let config = AppConfig {
            connection: crate::hardware::ConnectionConfig {
                cid: 1,
                apn: "test.apn".to_string(),
                username: Some("user".to_string()),
                password: Some("pass".to_string()),
                auth_type: crate::hardware::AuthType::Pap,
                ip_type: crate::hardware::IpType::Ipv4,
            },
            auto_connect: true,
            signal_poll_interval: 5,
            preferred_bands: vec!["B3".to_string(), "B7".to_string()],
            auth: Default::default(),
            tls: Default::default(),
            rate_limit: Default::default(),
            telemetry_enabled: true,
            tunnel: Default::default(),
            portal: Default::default(),
        };

        save_config_to(&config, &path).await.unwrap();
        let loaded = load_config_from(&path).await.unwrap();

        assert_eq!(loaded.connection.apn, "test.apn");
        assert!(loaded.auto_connect);
        assert_eq!(loaded.signal_poll_interval, 5);
        assert!(loaded.telemetry_enabled);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_write_secret_file_sets_0600() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().unwrap();
        let path = dir.path().join("secret.json");

        write_secret_file(&path, b"top-secret").await.unwrap();

        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(contents, "top-secret");

        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        // Mask to the permission bits; must be owner-only rw (0600).
        assert_eq!(mode & 0o777, 0o600, "secret file must be 0600, got {:o}", mode & 0o777);
    }

    #[cfg(unix)]
    #[test]
    fn test_write_secret_file_blocking_sets_0600() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().unwrap();
        let path = dir.path().join("secret.bin");

        write_secret_file_blocking(&path, b"top-secret").unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "secret file must be 0600, got {:o}", mode & 0o777);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_write_secret_file_created_already_0600_no_toctou_window() {
        // The file must be 0600 at creation time (create_new + mode), not
        // written-then-chmod. We can't observe the intermediate state directly,
        // but we assert the final mode and that a brand-new file is restricted.
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().unwrap();
        let path = dir.path().join("fresh-secret.json");
        assert!(!path.exists());

        write_secret_file(&path, b"secret-v1").await.unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "freshly created secret must be 0600");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "secret-v1");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_write_secret_file_overwrite_stays_0600() {
        // Overwriting an existing secret file must keep it 0600, and must even
        // re-assert 0600 if perms had drifted to world-readable.
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().unwrap();
        let path = dir.path().join("rewritten-secret.json");

        write_secret_file(&path, b"secret-v1").await.unwrap();

        // Simulate perm drift to world-readable, then rewrite.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        write_secret_file(&path, b"secret-v2").await.unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "overwritten secret must be re-restricted to 0600");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "secret-v2");
    }
}

#[cfg(test)]
mod portal_migration_tests {
    use super::*;

    #[test]
    fn old_config_without_portal_section_migrates_to_production_defaults() {
        // Simulate an existing config.toml predating the [portal] section.
        let legacy = r#"
auto_connect = false
signal_poll_interval = 2
preferred_bands = []
telemetry_enabled = true
"#;
        let cfg: AppConfig = toml::from_str(legacy).expect("legacy config must parse");
        assert_eq!(cfg.portal.base_url, "https://portal.ctrl-modem.com");
        assert_eq!(
            cfg.portal.tunnel_url.as_deref(),
            Some("wss://portal.ctrl-modem.com/api/v1/tunnel")
        );
    }

    #[test]
    fn explicit_portal_section_wins_over_defaults() {
        let toml = r#"
auto_connect = false
signal_poll_interval = 2
telemetry_enabled = true

[portal]
base_url = "https://staging.ctrl-modem.com"
tunnel_url = "wss://tunnel-staging.ctrl-modem.com/api/v1/tunnel"
"#;
        let cfg: AppConfig = toml::from_str(toml).expect("config must parse");
        assert_eq!(cfg.portal.base_url, "https://staging.ctrl-modem.com");
    }
}
