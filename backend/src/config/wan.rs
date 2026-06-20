//! WAN manager configuration persistence.
//!
//! Loads and saves the multi-modem WAN priority config from/to disk.
//! Follows the same pattern as `sim_slots.rs` for load/save.

use crate::hardware::{WanConfig, WanWatchdogLogEntry};

/// Default path for WAN config on OpenWRT.
const WAN_CONFIG_FILE: &str = "/etc/modem-interface/wan-config.json";

/// Default path for watchdog recovery log on OpenWRT.
const WAN_WATCHDOG_LOG_FILE: &str = "/var/log/modem-interface/wan-watchdog.log";

/// Load WAN config from disk. Returns defaults if file doesn't exist or is corrupt.
pub async fn load_wan_config() -> WanConfig {
    let path = std::env::var("WAN_CONFIG_PATH")
        .unwrap_or_else(|_| WAN_CONFIG_FILE.to_string());

    match tokio::fs::read_to_string(&path).await {
        Ok(content) => match serde_json::from_str::<WanConfig>(&content) {
            Ok(config) => {
                tracing::info!("Loaded WAN config from {}", path);
                config
            }
            Err(e) => {
                tracing::warn!("Failed to parse WAN config from {}: {e}", path);
                WanConfig::default()
            }
        },
        Err(_) => {
            tracing::info!("No WAN config at {}, using defaults", path);
            WanConfig::default()
        }
    }
}

/// Save WAN config to disk. Creates parent directories if needed.
pub async fn save_wan_config(config: &WanConfig) -> Result<(), String> {
    let path = std::env::var("WAN_CONFIG_PATH")
        .unwrap_or_else(|_| WAN_CONFIG_FILE.to_string());

    let json = serde_json::to_string_pretty(config)
        .map_err(|e| format!("Failed to serialize WAN config: {e}"))?;

    if let Some(parent) = std::path::Path::new(&path).parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("Failed to create directory: {e}"))?;
    }

    crate::config::write_secret_file(&path, json)
        .await
        .map_err(|e| format!("Failed to write WAN config: {e}"))?;

    tracing::info!("Saved WAN config to {}", path);
    Ok(())
}

// ============================================================================
// Watchdog Recovery Log
// ============================================================================

fn watchdog_log_path() -> String {
    std::env::var("WAN_WATCHDOG_LOG_PATH")
        .unwrap_or_else(|_| WAN_WATCHDOG_LOG_FILE.to_string())
}

/// Append a recovery event line to the watchdog log.
pub async fn append_watchdog_log(line: &str) -> Result<(), String> {
    let path = watchdog_log_path();

    if let Some(parent) = std::path::Path::new(&path).parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("Failed to create log directory: {e}"))?;
    }

    use tokio::io::AsyncWriteExt;
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .await
        .map_err(|e| format!("Failed to open watchdog log: {e}"))?;

    file.write_all(format!("{line}\n").as_bytes())
        .await
        .map_err(|e| format!("Failed to write watchdog log: {e}"))?;

    Ok(())
}

/// Parse a single log line into a `WanWatchdogLogEntry`.
/// Expected format: `{timestamp} {ACTION} {details...}`
fn parse_log_line(line: &str) -> Option<WanWatchdogLogEntry> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    // Timestamp is the first space-delimited token (ISO 8601)
    let (timestamp, rest) = line.split_once(' ')?;
    // Action is the next token
    let (action, details) = rest.split_once(' ').unwrap_or((rest, ""));
    Some(WanWatchdogLogEntry {
        timestamp: timestamp.to_string(),
        action: action.to_string(),
        details: details.to_string(),
    })
}

/// Read log entries, filtering to those within the retention window.
pub async fn read_watchdog_log(retention_days: u32) -> Vec<WanWatchdogLogEntry> {
    let path = watchdog_log_path();
    let content = match tokio::fs::read_to_string(&path).await {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    let cutoff = chrono::Utc::now() - chrono::Duration::days(retention_days as i64);
    let cutoff_str = cutoff.to_rfc3339();

    content
        .lines()
        .filter_map(parse_log_line)
        .filter(|entry| entry.timestamp >= cutoff_str)
        .collect()
}

/// Clear the watchdog log file.
pub async fn clear_watchdog_log() -> Result<(), String> {
    let path = watchdog_log_path();
    // Truncate or remove the file
    tokio::fs::write(&path, "")
        .await
        .map_err(|e| format!("Failed to clear watchdog log: {e}"))
}

/// Read the raw log file content for download.
pub async fn read_watchdog_log_raw() -> String {
    let path = watchdog_log_path();
    tokio::fs::read_to_string(&path).await.unwrap_or_default()
}
