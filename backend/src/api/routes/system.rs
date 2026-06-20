//! System update API route handlers.
//!
//! Update check is performed directly in Rust (no shell script dependency)
//! to avoid the chicken-and-egg problem where a broken update script prevents
//! deploying a fixed update script. The check downloads the package feed's
//! index file and parses the version directly, bypassing `opkg list` / `apk list`
//! which silently fall back to the installed version when no feed cache exists.
//!
//! Supports both package managers at runtime:
//! - **opkg** (OpenWRT default): reads `/etc/opkg/*.conf`, downloads `Packages.gz`
//! - **apk** (Alpine-based OpenWRT): delegates to native `apk update` +
//!   `apk list --available` (APK v3 `APKINDEX.tar.gz` is an ADB binary, not tar.gz,
//!   so custom tar-based parsing doesn't work)
//!
//! Update *apply* still uses the modem-interface-update shell script for the
//! actual package upgrade, lock file management, and status file writing.

use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::process::Command;
use tracing::{info, warn};

use crate::api::auth_middleware::{require_admin, SessionUser};
use crate::api::error::{ApiError, ApiResult};
use crate::security::audit::AuditEvent;
use crate::state::AppState;

/// Which package manager is available on this system.
#[derive(Debug, Clone, Copy, PartialEq)]
enum PackageManager {
    Apk,
    Opkg,
}

/// Detect the package manager at runtime by checking for the apk binary.
async fn detect_package_manager() -> PackageManager {
    if tokio::fs::metadata("/usr/bin/apk").await.is_ok() {
        PackageManager::Apk
    } else {
        PackageManager::Opkg
    }
}

/// Response for GET /api/system/version
#[derive(Debug, Serialize)]
pub struct VersionInfo {
    pub current_version: String,
}

/// Response for GET /api/system/update/check
#[derive(Debug, Serialize)]
pub struct UpdateCheckResult {
    pub update_available: bool,
    pub installed_version: String,
    pub available_version: Option<String>,
    /// Raw output from the update check/version commands for debug display.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub debug_log: Option<Vec<String>>,
}

/// Response for POST /api/system/update/apply
#[derive(Debug, Serialize)]
pub struct UpdateApplyResult {
    pub accepted: bool,
    pub message: String,
}

/// Response for GET /api/system/update/status
#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateStatus {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
}

/// GET /api/system/version — return currently running version.
pub async fn get_version() -> Json<VersionInfo> {
    Json(VersionInfo {
        current_version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

/// GET /api/system/update/check — check if a package update is available.
///
/// Performs the check directly in Rust instead of shelling out to the update
/// script. This avoids the chicken-and-egg problem (broken script can't be
/// updated) and bypasses `opkg list` which silently falls back to showing
/// the installed version when no feed cache exists.
pub async fn check_update() -> ApiResult<Json<UpdateCheckResult>> {
    let compiled_version = env!("CARGO_PKG_VERSION");

    // Mock mode for development
    if std::env::var("MOCK_HARDWARE").is_ok() {
        return Ok(Json(UpdateCheckResult {
            update_available: true,
            installed_version: compiled_version.to_string(),
            available_version: Some("99.99.99-1".to_string()),
            debug_log: Some(vec![
                "MOCK: simulated update available".to_string(),
                format!("MOCK: installed={compiled_version}"),
                "MOCK: available=99.99.99-1".to_string(),
            ]),
        }));
    }

    let mut debug = Vec::<String>::new();
    debug.push(format!("Running binary version: {compiled_version}"));

    // Step 1: Read feed URL from opkg config
    let feed_url = match read_feed_url().await {
        Ok(url) => {
            debug.push(format!("Feed URL: {url}"));
            url
        }
        Err(e) => {
            debug.push(format!("ERROR reading feed URL: {e}"));
            warn!("Update check failed: {e}");
            return Ok(Json(UpdateCheckResult {
                update_available: false,
                installed_version: compiled_version.to_string(),
                available_version: None,
                debug_log: Some(debug),
            }));
        }
    };

    // Step 2: Download Packages from feed and parse version
    let feed_version = match download_and_parse_feed_version(&feed_url, &mut debug).await {
        Ok(v) => {
            debug.push(format!("Feed package version: {v}"));
            v
        }
        Err(e) => {
            debug.push(format!("ERROR getting feed version: {e}"));
            warn!("Update check failed: {e}");
            return Ok(Json(UpdateCheckResult {
                update_available: false,
                installed_version: compiled_version.to_string(),
                available_version: None,
                debug_log: Some(debug),
            }));
        }
    };

    // Step 3: Compare versions
    // Feed version has release suffix (e.g. "0.8.4-1"), compiled version doesn't ("0.8.4")
    let feed_base = strip_release_suffix(&feed_version);
    let update_available = feed_base != compiled_version;

    debug.push(format!(
        "Comparison: feed_base={feed_base} vs compiled={compiled_version} → update_available={update_available}"
    ));

    info!(
        "Update check: running={compiled_version}, feed={feed_version} (base={feed_base}), update_available={update_available}",
    );

    Ok(Json(UpdateCheckResult {
        update_available,
        installed_version: compiled_version.to_string(),
        available_version: Some(feed_version),
        debug_log: Some(debug),
    }))
}

/// Read the custom feed URL from package manager configuration files.
///
/// For **opkg**: scans `/etc/opkg/customfeeds.conf` and `distfeeds.conf`, matching
/// on the URL field (3rd whitespace-separated token).
///
/// For **apk**: scans `/etc/apk/repositories` and drop-in files in
/// `/etc/apk/repositories.d/*.list`, matching on URLs containing our package name.
async fn read_feed_url() -> Result<String, String> {
    match detect_package_manager().await {
        PackageManager::Apk => read_feed_url_apk().await,
        PackageManager::Opkg => read_feed_url_opkg().await,
    }
}

/// Read feed URL from opkg configuration.
async fn read_feed_url_opkg() -> Result<String, String> {
    for conf_path in &["/etc/opkg/customfeeds.conf", "/etc/opkg/distfeeds.conf"] {
        if let Ok(content) = tokio::fs::read_to_string(conf_path).await {
            for line in content.lines() {
                // Format: src/gz <name> <url>
                if let Some(url) = line.split_whitespace().nth(2) {
                    if url.contains("modem-interface") || url.contains("ctrl-modem") {
                        return Ok(url.to_string());
                    }
                }
            }
        }
    }
    Err("Cannot find CTRL-Modem feed URL in opkg config (expected URL containing 'modem-interface' or 'ctrl-modem')".to_string())
}

/// Detect the APK device architecture.
///
/// Priority order (first non-empty wins):
/// 1. `/etc/apk/arch` — OpenWrt's source of truth for the subtarget arch
/// 2. `DISTRIB_ARCH` from `/etc/openwrt_release`
/// 3. `apk --print-arch` — last resort; returns CPU family (e.g. `aarch64`)
///    rather than the full OpenWrt subtarget arch (e.g. `aarch64_cortex-a53`),
///    so it's unreliable for feed URL construction.
async fn get_apk_arch() -> Option<String> {
    // 1. /etc/apk/arch
    if let Ok(content) = tokio::fs::read_to_string("/etc/apk/arch").await {
        let arch = content.trim();
        if !arch.is_empty() {
            return Some(arch.to_string());
        }
    }

    // 2. DISTRIB_ARCH from /etc/openwrt_release
    if let Ok(content) = tokio::fs::read_to_string("/etc/openwrt_release").await {
        for line in content.lines() {
            let trimmed = line.trim();
            // Match `DISTRIB_ARCH=...` exactly (not DISTRIB_ARCHITECTURE etc.)
            if let Some(value) = trimmed.strip_prefix("DISTRIB_ARCH=") {
                let value = value.trim();
                let value = value
                    .strip_prefix('\'')
                    .and_then(|v| v.strip_suffix('\''))
                    .or_else(|| value.strip_prefix('"').and_then(|v| v.strip_suffix('"')))
                    .unwrap_or(value)
                    .trim();
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
        }
    }

    // 3. Last resort: apk --print-arch (returns CPU family, not full subtarget)
    let output = Command::new("apk")
        .arg("--print-arch")
        .output()
        .await
        .ok()?;
    if output.status.success() {
        let arch = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !arch.is_empty() {
            return Some(arch);
        }
    }
    None
}

/// Read feed URL from apk repositories configuration.
///
/// APK's own `apk update` auto-appends the device architecture to bare feed
/// URLs, but our download code doesn't.  We detect the arch via
/// `apk --print-arch` and append it so all downstream code (version check,
/// download URL construction) gets the correct arch-specific path.
async fn read_feed_url_apk() -> Result<String, String> {
    let mut paths = vec!["/etc/apk/repositories".to_string()];

    // Collect drop-in files from /etc/apk/repositories.d/*.list
    if let Ok(mut entries) = tokio::fs::read_dir("/etc/apk/repositories.d").await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("list") {
                if let Some(p) = path.to_str() {
                    paths.push(p.to_string());
                }
            }
        }
    }

    for conf_path in &paths {
        if let Ok(content) = tokio::fs::read_to_string(conf_path).await {
            for line in content.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if line.contains("modem-interface") || line.contains("ctrl-modem") {
                    // Append device arch so the URL points to the correct
                    // arch-specific subdirectory (e.g. .../apk/aarch64_cortex-a53).
                    if let Some(arch) = get_apk_arch().await {
                        let base = line.trim_end_matches('/');
                        if !base.ends_with(&arch) {
                            return Ok(format!("{base}/{arch}"));
                        }
                    }
                    return Ok(line.to_string());
                }
            }
        }
    }
    Err("Cannot find CTRL-Modem feed URL in apk repositories (expected URL containing 'modem-interface' or 'ctrl-modem')".to_string())
}

/// Download a file using the best available HTTP client on OpenWRT.
/// Tries uclient-fetch (OpenWRT native), then wget, then curl.
///
/// TLS certificates are validated against the system CA bundle (the
/// `ca-bundle` package is a hard dependency); none of the tools below skip
/// certificate verification.
async fn download_file(url: &str, dest: &str, debug: &mut Vec<String>) -> Result<(), String> {
    // Try uclient-fetch first
    debug.push(format!("Trying uclient-fetch: {url}"));
    let result = Command::new("uclient-fetch")
        .args(["-q", "-O", dest])
        .arg(url)
        .output()
        .await;
    if let Ok(output) = result {
        if output.status.success() {
            debug.push("uclient-fetch succeeded".to_string());
            return Ok(());
        }
        let err = String::from_utf8_lossy(&output.stderr);
        debug.push(format!("uclient-fetch failed: {}", err.trim()));
    } else {
        debug.push("uclient-fetch not available".to_string());
    }

    // Fallback: wget
    debug.push(format!("Trying wget: {url}"));
    let result = Command::new("wget")
        .args(["-q", "-T", "30", "-O", dest])
        .arg(url)
        .output()
        .await;
    if let Ok(output) = result {
        if output.status.success() {
            debug.push("wget succeeded".to_string());
            return Ok(());
        }
        let err = String::from_utf8_lossy(&output.stderr);
        debug.push(format!("wget failed: {}", err.trim()));
    } else {
        debug.push("wget not available".to_string());
    }

    // Fallback: curl
    debug.push(format!("Trying curl: {url}"));
    let result = Command::new("curl")
        .args(["-sS", "-o", dest, "--connect-timeout", "30"])
        .arg(url)
        .output()
        .await;
    if let Ok(output) = result {
        if output.status.success() {
            debug.push("curl succeeded".to_string());
            return Ok(());
        }
        let err = String::from_utf8_lossy(&output.stderr);
        debug.push(format!("curl failed: {}", err.trim()));
    } else {
        debug.push("curl not available".to_string());
    }

    Err(format!("All download methods failed for {url}"))
}

/// Fetch the available modem-interface version from the configured feed.
///
/// For **opkg**: downloads `Packages.gz` (or `Packages`), parses opkg stanza format.
/// For **apk**: runs native `apk update` + `apk list --available modem-interface`
/// (APK v3 APKINDEX is ADB binary, which our custom tar parser cannot read).
async fn download_and_parse_feed_version(
    feed_url: &str,
    debug: &mut Vec<String>,
) -> Result<String, String> {
    match detect_package_manager().await {
        PackageManager::Apk => download_and_parse_feed_version_apk(feed_url, debug).await,
        PackageManager::Opkg => download_and_parse_feed_version_opkg(feed_url, debug).await,
    }
}

/// Download and parse opkg Packages feed.
async fn download_and_parse_feed_version_opkg(
    feed_url: &str,
    debug: &mut Vec<String>,
) -> Result<String, String> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let tmp_gz = "/tmp/modem-interface-feed-check.gz";
    let tmp_txt = "/tmp/modem-interface-feed-check.txt";

    // Try Packages.gz first (smaller download)
    let gz_url = format!("{feed_url}/Packages.gz?t={timestamp}");
    if download_file(&gz_url, tmp_gz, debug).await.is_ok() {
        // Decompress via gunzip -c (writes to stdout, works on BusyBox and GNU)
        let result = Command::new("sh")
            .args(["-c", &format!("gunzip -c {tmp_gz} 2>/dev/null")])
            .output()
            .await;

        let _ = tokio::fs::remove_file(tmp_gz).await;

        if let Ok(output) = result {
            if output.status.success() {
                let text = String::from_utf8_lossy(&output.stdout);
                if let Some(version) = parse_version_from_packages(&text) {
                    debug.push(format!("Parsed version from Packages.gz: {version}"));
                    return Ok(version);
                }
                debug.push("Packages.gz downloaded but no modem-interface version found".to_string());
            } else {
                debug.push("gunzip decompression failed".to_string());
            }
        }
    }

    // Fallback: try uncompressed Packages
    let pkg_url = format!("{feed_url}/Packages?t={timestamp}");
    if download_file(&pkg_url, tmp_txt, debug).await.is_ok() {
        if let Ok(text) = tokio::fs::read_to_string(tmp_txt).await {
            let _ = tokio::fs::remove_file(tmp_txt).await;
            if let Some(version) = parse_version_from_packages(&text) {
                debug.push(format!("Parsed version from Packages: {version}"));
                return Ok(version);
            }
            debug.push("Packages downloaded but no modem-interface version found".to_string());
        } else {
            let _ = tokio::fs::remove_file(tmp_txt).await;
            debug.push("Failed to read downloaded Packages file".to_string());
        }
    }

    Err("Failed to download or parse feed Packages file".to_string())
}

/// Query available modem-interface version using native `apk` commands.
///
/// APK v3 (OpenWrt 24.10+, apk-tools 3.x) changed `APKINDEX.tar.gz` from a
/// gzipped tar archive to an ADB binary format (magic `ADBd`). Our previous
/// custom parser that ran `tar xzf` on it now fails with "tar: invalid magic".
///
/// The fix is to delegate to `apk` itself, which natively understands the ADB
/// format. We run `apk update` to refresh apk's own cache from all configured
/// repositories, then `apk list --available modem-interface` and parse the
/// first line.
///
/// `apk list` output format:
/// ```text
/// modem-interface-1.0.148-r1 aarch64_cortex-a53 {modem-interface} (proprietary) [installed]
/// ```
///
/// The `_feed_url` parameter is unused — apk reads its own repository config
/// from `/etc/apk/repositories{,.d/*.list}` — but we still log it for debug
/// visibility.
async fn download_and_parse_feed_version_apk(
    _feed_url: &str,
    debug: &mut Vec<String>,
) -> Result<String, String> {
    debug.push(
        "Using native apk (feed URL from /etc/apk/repositories, not downloaded directly)"
            .to_string(),
    );

    // Step 1: Refresh apk's cache. Non-zero exit is logged but not fatal —
    // a partial cache may still contain our package from a previous run.
    match Command::new("apk").arg("update").output().await {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stdout.trim().is_empty() {
                debug.push(format!("apk update stdout: {}", stdout.trim()));
            }
            if !stderr.trim().is_empty() {
                debug.push(format!("apk update stderr: {}", stderr.trim()));
            }
            if !output.status.success() {
                debug.push(format!(
                    "apk update exited non-zero ({}), continuing with possibly stale cache",
                    output.status
                ));
            }
        }
        Err(e) => {
            debug.push(format!("apk update failed to spawn: {e}"));
        }
    }

    // Step 2: List available modem-interface package.
    let list_output = Command::new("apk")
        .args(["list", "--available", "modem-interface"])
        .output()
        .await
        .map_err(|e| format!("Failed to run `apk list --available modem-interface`: {e}"))?;

    let stdout = String::from_utf8_lossy(&list_output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&list_output.stderr);
    if !stderr.trim().is_empty() {
        debug.push(format!("apk list stderr: {}", stderr.trim()));
    }

    if !list_output.status.success() {
        return Err(format!(
            "`apk list --available modem-interface` exited with {}: {}",
            list_output.status,
            stderr.trim()
        ));
    }

    // Step 3: Parse the first non-empty line.
    // Format: `modem-interface-<version>-<release> <arch> {<origin>} (<license>) [installed]`
    // First whitespace-separated token is `modem-interface-<version>-<release>`.
    let first_line = stdout.lines().find(|l| !l.trim().is_empty()).ok_or_else(|| {
        format!(
            "`apk list --available modem-interface` produced no output (stdout: {:?})",
            stdout.trim()
        )
    })?;

    let token = first_line.split_whitespace().next().ok_or_else(|| {
        format!("Failed to tokenize first line of apk list output: {first_line:?}")
    })?;

    let version = token.strip_prefix("modem-interface-").ok_or_else(|| {
        format!("First token {token:?} does not start with 'modem-interface-' (full line: {first_line:?})")
    })?;

    if version.is_empty() {
        return Err(format!("Parsed empty version from apk list token: {token:?}"));
    }

    debug.push(format!("Parsed version from `apk list`: {version}"));
    Ok(version.to_string())
}

/// Parse the Version field for modem-interface from an opkg Packages file.
///
/// Packages file format:
/// ```text
/// Package: modem-interface
/// Version: 0.8.4-1
/// Depends: ...
/// ```
fn parse_version_from_packages(content: &str) -> Option<String> {
    let mut in_our_package = false;
    for line in content.lines() {
        if line == "Package: modem-interface" {
            in_our_package = true;
            continue;
        }
        if in_our_package {
            if line.is_empty() {
                break; // End of package stanza
            }
            if let Some(version) = line.strip_prefix("Version: ") {
                return Some(version.trim().to_string());
            }
        }
    }
    None
}

/// Strip the release suffix (e.g. "0.8.4-1" → "0.8.4", "1.0.138-r1" → "1.0.138").
fn strip_release_suffix(version: &str) -> &str {
    version.rsplit_once('-').map_or(version, |(base, _)| base)
}

/// POST /api/system/update/apply — trigger opkg upgrade.
///
/// Spawns the update as a detached background process with a 2-second delay
/// so the HTTP response can be sent before opkg kills this process.
///
/// Tries three spawn strategies in order:
/// 1. `setsid` — creates a new session, child survives procd cleanup (preferred)
/// 2. `nohup` — ignores SIGHUP, common alternative on OpenWRT forks lacking setsid
/// 3. Plain `sh` — last resort, may get killed by procd but worth trying
pub async fn apply_update(
    axum::Extension(session_user): axum::Extension<SessionUser>,
) -> ApiResult<Json<UpdateApplyResult>> {
    require_admin(&session_user)?;

    // Mock mode for development
    if std::env::var("MOCK_HARDWARE").is_ok() {
        return Ok(Json(UpdateApplyResult {
            accepted: true,
            message: "Mock update accepted (no real update in dev mode)".to_string(),
        }));
    }

    info!("Update apply requested — spawning background update process");

    let update_cmd = "sleep 2 && /usr/bin/modem-interface-update apply";
    let null_io = || {
        (
            std::process::Stdio::null(),
            std::process::Stdio::null(),
            std::process::Stdio::null(),
        )
    };

    // Try setsid first — new session keeps child alive when procd kills our
    // process group during opkg upgrade.
    let (stdin, stdout, stderr) = null_io();
    let result = Command::new("setsid")
        .arg("sh")
        .arg("-c")
        .arg(update_cmd)
        .stdin(stdin)
        .stdout(stdout)
        .stderr(stderr)
        .spawn();

    let result = match result {
        Ok(child) => Ok(child),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // setsid not available (e.g., Q-WRT) — try nohup instead.
            warn!("setsid not found, falling back to nohup");
            let (stdin, stdout, stderr) = null_io();
            Command::new("nohup")
                .arg("sh")
                .arg("-c")
                .arg(update_cmd)
                .stdin(stdin)
                .stdout(stdout)
                .stderr(stderr)
                .spawn()
        }
        Err(e) => Err(e),
    };

    let result = match result {
        Ok(child) => Ok(child),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Neither setsid nor nohup available — try plain sh as last resort.
            warn!("nohup not found, falling back to plain sh");
            let (stdin, stdout, stderr) = null_io();
            Command::new("sh")
                .arg("-c")
                .arg(update_cmd)
                .stdin(stdin)
                .stdout(stdout)
                .stderr(stderr)
                .spawn()
        }
        Err(e) => Err(e),
    };

    match result {
        Ok(_child) => {
            info!("Update process spawned, service will restart in ~2 seconds");
            Ok(Json(UpdateApplyResult {
                accepted: true,
                message: "Update process started. Service will restart.".to_string(),
            }))
        }
        Err(e) => {
            warn!("Failed to spawn update process: {}", e);
            Err(ApiError::internal(format!("Failed to start update: {e}")))
        }
    }
}

/// GET /api/system/update/status — read the status file left by the update script.
pub async fn get_update_status() -> Json<UpdateStatus> {
    match tokio::fs::read_to_string("/tmp/modem-interface-update-status.json").await {
        Ok(contents) => match serde_json::from_str::<UpdateStatus>(&contents) {
            Ok(status) => Json(status),
            Err(_) => Json(idle_status()),
        },
        Err(_) => Json(idle_status()),
    }
}

/// GET /api/system/update/log — return last 50 lines of the update log.
pub async fn get_update_log() -> Json<Vec<String>> {
    let contents = tokio::fs::read_to_string("/var/log/modem-interface-update.log")
        .await
        .unwrap_or_default();

    let lines: Vec<String> = contents
        .lines()
        .rev()
        .take(50)
        .map(|s| s.to_string())
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();

    Json(lines)
}

fn idle_status() -> UpdateStatus {
    UpdateStatus {
        status: "idle".to_string(),
        previous_version: None,
        new_version: None,
        timestamp: None,
    }
}

/// GET /api/system/audit — return recent security audit events.
pub async fn get_audit_log(
    State(state): State<Arc<AppState>>,
) -> Json<Vec<AuditEvent>> {
    Json(state.audit.recent(100).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_version_from_packages_found() {
        let content = "\
Package: some-other-package
Version: 2.0.0-1

Package: modem-interface
Version: 1.0.138-1
Depends: libc

Package: another-package
Version: 3.0.0-1
";
        assert_eq!(
            parse_version_from_packages(content),
            Some("1.0.138-1".to_string())
        );
    }

    #[test]
    fn test_parse_version_from_packages_not_found() {
        let content = "\
Package: some-other-package
Version: 2.0.0-1

Package: another-package
Version: 3.0.0-1
";
        assert_eq!(parse_version_from_packages(content), None);
    }

    #[test]
    fn test_strip_release_suffix_opkg() {
        assert_eq!(strip_release_suffix("1.0.138-1"), "1.0.138");
    }

    #[test]
    fn test_strip_release_suffix_apk() {
        assert_eq!(strip_release_suffix("1.0.138-r1"), "1.0.138");
    }

    #[test]
    fn test_strip_release_suffix_none() {
        assert_eq!(strip_release_suffix("1.0.138"), "1.0.138");
    }
}
