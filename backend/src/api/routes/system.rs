//! System update API route handlers.
//!
//! Update check is performed directly in Rust (no shell script dependency)
//! to avoid the chicken-and-egg problem where a broken update script prevents
//! deploying a fixed update script.
//!
//! The "is an update available?" decision delegates to the package manager's
//! OWN upgrade detection rather than comparing a feed version against the
//! running binary's `CARGO_PKG_VERSION`:
//!
//! - **apk** (Alpine-based OpenWRT): `apk update` + `apk list --upgradable modem-interface`
//! - **opkg** (OpenWRT default): `opkg update` + `opkg list-upgradable | grep modem-interface`
//!
//! Why not compare against `CARGO_PKG_VERSION`? The compiled binary and the feed
//! package use DIFFERENT prerelease naming schemes that the package manager's
//! version comparator cannot reconcile:
//!
//! - apk packages a dev build as `1.5.0_alpha13`, but `CARGO_PKG_VERSION` is
//!   `1.5.0-dev.13`. `apk version -t "1.5.0_alpha13" "1.5.0-dev.13"` cannot parse
//!   the `-dev.N` form (apk releases must be `-rN`) → exits non-zero → a lexical
//!   fallback wrongly orders `_alpha` (0x5F) above `-dev` (0x2D), reporting an
//!   "update" that points at the version already installed.
//! - opkg dev builds are versioned `1.3.0~devN`, same scheme-mismatch problem.
//!
//! `apk list --upgradable` / `opkg list-upgradable` use the package manager's
//! OWN installed-version baseline and native version ordering, so they return
//! EMPTY when up-to-date and a line per strictly-newer package otherwise —
//! sidestepping the scheme mismatch entirely. `installed_version` in the
//! response stays the running binary's `CARGO_PKG_VERSION` (honest "what am I
//! running"); `update_available` / `available_version` come from the package
//! manager's upgrade detection.
//!
//! Update *apply* still uses the modem-interface-update shell script for the
//! actual package upgrade, lock file management, and status file writing.

use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
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
/// Decides via the package manager's native upgrade detection
/// (`apk list --upgradable` / `opkg list-upgradable`) rather than a
/// feed-version-vs-`CARGO_PKG_VERSION` comparison. See the module docs for why
/// the direct comparison is unreliable across the dev/feed prerelease naming
/// schemes.
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

    let pm = detect_package_manager().await;
    debug.push(format!("Package manager: {pm:?}"));

    // Ask the package manager whether a strictly-newer modem-interface is
    // available across its configured feeds, using its OWN installed-version
    // baseline + native version ordering.
    let upgrade_target = match pm {
        PackageManager::Apk => detect_apk_upgrade(&mut debug).await,
        PackageManager::Opkg => detect_opkg_upgrade(&mut debug).await,
    };

    match upgrade_target {
        Ok(Some(target)) => {
            debug.push(format!("Upgrade available → {target}"));
            info!("Update check: running={compiled_version}, upgrade available → {target}");
            Ok(Json(UpdateCheckResult {
                update_available: true,
                installed_version: compiled_version.to_string(),
                available_version: Some(target),
                debug_log: Some(debug),
            }))
        }
        Ok(None) => {
            debug.push("No upgrade available — on the latest the feeds offer".to_string());
            info!("Update check: running={compiled_version}, up to date");
            Ok(Json(UpdateCheckResult {
                update_available: false,
                installed_version: compiled_version.to_string(),
                available_version: None,
                debug_log: Some(debug),
            }))
        }
        Err(e) => {
            debug.push(format!("ERROR during upgrade detection: {e}"));
            warn!("Update check failed: {e}");
            // Fail safe: report no update rather than a spurious one.
            Ok(Json(UpdateCheckResult {
                update_available: false,
                installed_version: compiled_version.to_string(),
                available_version: None,
                debug_log: Some(debug),
            }))
        }
    }
}

/// Refresh the apk cache then query `apk list --upgradable modem-interface`.
///
/// Returns `Ok(Some(version))` when a strictly-newer version is offered (apk's
/// own decision), `Ok(None)` when up to date (empty output), `Err` only when the
/// list command could not be run at all.
async fn detect_apk_upgrade(debug: &mut Vec<String>) -> Result<Option<String>, String> {
    // Step 1: Refresh apk's cache. Non-zero exit is logged but not fatal —
    // a partial cache may still let `list --upgradable` produce a useful answer.
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

    // Step 2: Native upgrade detection.
    let list_output = Command::new("apk")
        .args(["list", "--upgradable", "modem-interface"])
        .output()
        .await
        .map_err(|e| format!("Failed to run `apk list --upgradable modem-interface`: {e}"))?;

    let stdout = String::from_utf8_lossy(&list_output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&list_output.stderr);
    if !stderr.trim().is_empty() {
        debug.push(format!("apk list --upgradable stderr: {}", stderr.trim()));
    }
    debug.push(format!(
        "apk list --upgradable modem-interface → {:?}",
        stdout.trim()
    ));

    if !list_output.status.success() {
        return Err(format!(
            "`apk list --upgradable modem-interface` exited with {}: {}",
            list_output.status,
            stderr.trim()
        ));
    }

    Ok(parse_apk_upgradable_target(&stdout))
}

/// Parse `apk list --upgradable modem-interface` output into an optional target
/// version.
///
/// Empty output (no `modem-interface` line) → `None` (up to date). Otherwise the
/// first matching line looks like:
/// `modem-interface-<version>-<release> <arch> {<origin>} (<license>) [upgradable from: <old>]`
/// and we return `<version>-<release>` (everything after the `modem-interface-`
/// prefix in the leading token), robust to the trailing `[upgradable from: ...]`
/// annotation.
fn parse_apk_upgradable_target(stdout: &str) -> Option<String> {
    stdout.lines().find_map(|line| {
        let token = line.split_whitespace().next()?;
        let version = token.strip_prefix("modem-interface-")?;
        if version.is_empty() {
            None
        } else {
            Some(version.to_string())
        }
    })
}

/// Refresh the opkg cache then query `opkg list-upgradable` for modem-interface.
///
/// Returns `Ok(Some(version))` when opkg reports a strictly-newer version,
/// `Ok(None)` when up to date (no modem-interface line), `Err` only when the
/// list command could not be run at all.
///
/// opkg dev builds are versioned `1.3.0~devN`; like apk's `_alphaN`, this would
/// mis-compare against the binary's `1.3.0-devN` `CARGO_PKG_VERSION`. Delegating
/// to opkg's own list-upgradable sidesteps the scheme mismatch.
async fn detect_opkg_upgrade(debug: &mut Vec<String>) -> Result<Option<String>, String> {
    // Step 1: Refresh opkg's package lists. Non-zero exit logged, not fatal.
    match Command::new("opkg").arg("update").output().await {
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stderr.trim().is_empty() {
                debug.push(format!("opkg update stderr: {}", stderr.trim()));
            }
            if !output.status.success() {
                debug.push(format!(
                    "opkg update exited non-zero ({}), continuing with possibly stale lists",
                    output.status
                ));
            }
        }
        Err(e) => {
            debug.push(format!("opkg update failed to spawn: {e}"));
        }
    }

    // Step 2: Native upgrade detection.
    let list_output = Command::new("opkg")
        .arg("list-upgradable")
        .output()
        .await
        .map_err(|e| format!("Failed to run `opkg list-upgradable`: {e}"))?;

    let stdout = String::from_utf8_lossy(&list_output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&list_output.stderr);
    if !stderr.trim().is_empty() {
        debug.push(format!("opkg list-upgradable stderr: {}", stderr.trim()));
    }

    if !list_output.status.success() {
        return Err(format!(
            "`opkg list-upgradable` exited with {}: {}",
            list_output.status,
            stderr.trim()
        ));
    }

    let target = parse_opkg_upgradable_target(&stdout);
    debug.push(format!(
        "opkg list-upgradable (modem-interface) → {target:?}"
    ));
    Ok(target)
}

/// Parse `opkg list-upgradable` output for the modem-interface upgrade target.
///
/// opkg prints one line per upgradable package:
/// `modem-interface - <installed-version> - <new-version>`
/// (the package name, the currently-installed version, and the available newer
/// version, space-`-`-space separated). We return `<new-version>` for the
/// `modem-interface` line, or `None` if there is no such line (up to date).
fn parse_opkg_upgradable_target(stdout: &str) -> Option<String> {
    stdout.lines().find_map(|line| {
        let mut fields = line.split(" - ");
        let name = fields.next()?.trim();
        if name != "modem-interface" {
            return None;
        }
        // fields: <installed> , <new>
        let _installed = fields.next()?;
        let new_version = fields.next()?.trim();
        if new_version.is_empty() {
            None
        } else {
            Some(new_version.to_string())
        }
    })
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

    // ---- apk --upgradable parsing ---------------------------------------------

    #[test]
    fn test_parse_apk_upgradable_empty_is_none() {
        // Up to date: apk prints nothing.
        assert_eq!(parse_apk_upgradable_target(""), None);
        assert_eq!(parse_apk_upgradable_target("\n\n  \n"), None);
    }

    #[test]
    fn test_parse_apk_upgradable_with_annotation() {
        // Real apk output carries a trailing `[upgradable from: ...]` annotation.
        let line = "modem-interface-1.6.0-r1 aarch64_cortex-a53 {modem-interface} (proprietary) [upgradable from: modem-interface-1.5.0_alpha13-r1]\n";
        assert_eq!(
            parse_apk_upgradable_target(line),
            Some("1.6.0-r1".to_string())
        );
    }

    #[test]
    fn test_parse_apk_upgradable_without_annotation() {
        // Same decision when no trailing annotation is present.
        let line =
            "modem-interface-1.6.0-r1 aarch64_cortex-a53 {modem-interface} (proprietary)\n";
        assert_eq!(
            parse_apk_upgradable_target(line),
            Some("1.6.0-r1".to_string())
        );
    }

    #[test]
    fn test_parse_apk_upgradable_prerelease_target() {
        // Target may itself be a prerelease (newer alpha).
        let line = "modem-interface-1.5.0_alpha14-r1 aarch64_cortex-a53 {modem-interface} (proprietary) [upgradable from: modem-interface-1.5.0_alpha13-r1]";
        assert_eq!(
            parse_apk_upgradable_target(line),
            Some("1.5.0_alpha14-r1".to_string())
        );
    }

    #[test]
    fn test_parse_apk_upgradable_skips_foreign_lines() {
        // Only the modem-interface line matters; foreign packages are ignored.
        let mixed = "\nsome-other-package-2.0.0-r1 aarch64_cortex-a53 {some-other} (GPL) [upgradable from: some-other-package-1.0.0-r1]\nmodem-interface-1.6.0-r1 aarch64_cortex-a53 {modem-interface} (proprietary) [upgradable from: modem-interface-1.5.0_alpha13-r1]\n";
        assert_eq!(
            parse_apk_upgradable_target(mixed),
            Some("1.6.0-r1".to_string())
        );
    }

    // ---- opkg list-upgradable parsing -----------------------------------------

    #[test]
    fn test_parse_opkg_upgradable_empty_is_none() {
        assert_eq!(parse_opkg_upgradable_target(""), None);
        assert_eq!(parse_opkg_upgradable_target("\n\n"), None);
    }

    #[test]
    fn test_parse_opkg_upgradable_target() {
        // opkg format: `<pkg> - <installed> - <new>`.
        let line = "modem-interface - 1.3.0~dev35-1 - 1.3.0~dev36-1\n";
        assert_eq!(
            parse_opkg_upgradable_target(line),
            Some("1.3.0~dev36-1".to_string())
        );
    }

    #[test]
    fn test_parse_opkg_upgradable_skips_foreign_lines() {
        let mixed = "luci - 1.0.0-1 - 1.0.1-1\nmodem-interface - 1.3.0~dev35-1 - 1.4.0-1\nkmod-foo - 5.4-1 - 5.5-1\n";
        assert_eq!(
            parse_opkg_upgradable_target(mixed),
            Some("1.4.0-1".to_string())
        );
    }

    #[test]
    fn test_parse_opkg_upgradable_no_modem_interface_line() {
        // Other packages upgradable but not ours → None.
        let mixed = "luci - 1.0.0-1 - 1.0.1-1\nkmod-foo - 5.4-1 - 5.5-1\n";
        assert_eq!(parse_opkg_upgradable_target(mixed), None);
    }
}
