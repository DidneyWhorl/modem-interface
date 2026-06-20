//! SIM slot configuration persistence.
//!
//! Loads and saves per-modem SIM slot configs from/to disk.
//! Keyed by "VID:PID" (e.g. "2c7c:0122") so configs don't bleed across modems.
//!
//! Backwards-compatible: old flat SimSlotConfig format is migrated on load
//! under a "legacy" key.

use crate::hardware::{AllSimSlotConfig, SimSlotConfig};

/// Default path for SIM slot config on OpenWRT.
const SIM_SLOT_CONFIG_FILE: &str = "/etc/modem-interface/sim-slot-config.json";

/// Load SIM slot config from disk. Returns defaults if file doesn't exist or is corrupt.
///
/// Handles two JSON formats:
/// - **New**: `{ "modems": { "2c7c:0122": { ... }, ... } }`
/// - **Old** (flat): `{ "slot1_profile_id": "...", "slot2_profile_id": "..." }`
///   → migrated under the `"legacy"` key and re-saved.
pub async fn load_sim_slot_config() -> AllSimSlotConfig {
    let path = std::env::var("SIM_SLOT_CONFIG_PATH")
        .unwrap_or_else(|_| SIM_SLOT_CONFIG_FILE.to_string());

    let content = match tokio::fs::read_to_string(&path).await {
        Ok(c) => c,
        Err(_) => {
            tracing::info!("No SIM slot config at {}, using defaults", path);
            return AllSimSlotConfig::default();
        }
    };

    // Try new format first
    if let Ok(config) = serde_json::from_str::<AllSimSlotConfig>(&content) {
        if !config.modems.is_empty() {
            tracing::info!("Loaded per-modem SIM slot config from {} ({} modems)", path, config.modems.len());
            return config;
        }
    }

    // Try old flat format and migrate
    if let Ok(old) = serde_json::from_str::<SimSlotConfig>(&content) {
        if old.slot1_profile_id.is_some() || old.slot2_profile_id.is_some() {
            tracing::info!("Migrating old flat SIM slot config to per-modem format");
            let mut config = AllSimSlotConfig::default();
            config.modems.insert("legacy".to_string(), old);

            // Re-save in new format
            if let Err(e) = save_sim_slot_config(&config).await {
                tracing::warn!("Failed to save migrated SIM slot config: {e}");
            }
            return config;
        }
    }

    tracing::info!("SIM slot config at {} is empty or unparseable, using defaults", path);
    AllSimSlotConfig::default()
}

/// Save SIM slot config to disk. Creates parent directories if needed.
pub async fn save_sim_slot_config(config: &AllSimSlotConfig) -> Result<(), String> {
    let path = std::env::var("SIM_SLOT_CONFIG_PATH")
        .unwrap_or_else(|_| SIM_SLOT_CONFIG_FILE.to_string());

    let json = serde_json::to_string_pretty(config)
        .map_err(|e| format!("Failed to serialize SIM slot config: {e}"))?;

    if let Some(parent) = std::path::Path::new(&path).parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("Failed to create directory: {e}"))?;
    }

    crate::config::write_secret_file(&path, json)
        .await
        .map_err(|e| format!("Failed to write SIM slot config: {e}"))?;

    tracing::info!("Saved SIM slot config to {}", path);
    Ok(())
}
