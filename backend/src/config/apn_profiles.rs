//! APN profile persistence.
//!
//! Loads and saves APN profiles (saved connection presets) from/to disk.
//! Follows the same pattern as `security/at_whitelist.rs` for load/save.

use crate::hardware::ApnProfile;

/// Default path for APN profiles storage on OpenWRT.
const APN_PROFILES_FILE: &str = "/etc/modem-interface/apn-profiles.json";

/// Load APN profiles from disk. Returns empty vec if file doesn't exist or is corrupt.
pub async fn load_apn_profiles() -> Vec<ApnProfile> {
    let path = std::env::var("APN_PROFILES_PATH")
        .unwrap_or_else(|_| APN_PROFILES_FILE.to_string());

    match tokio::fs::read_to_string(&path).await {
        Ok(content) => match serde_json::from_str::<Vec<ApnProfile>>(&content) {
            Ok(profiles) => {
                tracing::info!("Loaded {} APN profile(s) from {}", profiles.len(), path);
                profiles
            }
            Err(e) => {
                tracing::warn!("Failed to parse APN profiles from {}: {e}", path);
                Vec::new()
            }
        },
        Err(_) => {
            tracing::info!("No APN profiles file at {}, starting empty", path);
            Vec::new()
        }
    }
}

/// Save APN profiles to disk. Creates parent directories if needed.
pub async fn save_apn_profiles(profiles: &[ApnProfile]) -> Result<(), String> {
    let path = std::env::var("APN_PROFILES_PATH")
        .unwrap_or_else(|_| APN_PROFILES_FILE.to_string());

    let json = serde_json::to_string_pretty(profiles)
        .map_err(|e| format!("Failed to serialize APN profiles: {e}"))?;

    if let Some(parent) = std::path::Path::new(&path).parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("Failed to create directory: {e}"))?;
    }

    crate::config::write_secret_file(&path, json)
        .await
        .map_err(|e| format!("Failed to write APN profiles: {e}"))?;

    tracing::info!("Saved {} APN profile(s) to {}", profiles.len(), path);
    Ok(())
}
