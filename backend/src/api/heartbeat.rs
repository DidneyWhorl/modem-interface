//! Portal heartbeat background task.
//!
//! Periodically POSTs device status to the CTRL-Cloud portal so it knows
//! the device is alive. Runs every 30 minutes, only when licensed.

use std::sync::Arc;
use tokio::process::Command;
use tokio::sync::OnceCell;
use tracing::{debug, info, warn};

use crate::security::device_auth::DeviceAuth;
use crate::state::AppState;

/// Default heartbeat interval (30 minutes). Actual interval is read from state.
const DEFAULT_HEARTBEAT_INTERVAL_SECS: u64 = 30 * 60;

/// Initial delay before the first heartbeat (let the system stabilize).
const INITIAL_DELAY: std::time::Duration = std::time::Duration::from_secs(60);

/// Ordered HTTP request headers for an outgoing heartbeat POST.
///
/// Returns `Content-Type`, then the Item #3 Phase 2 enrollment headers
/// (`X-Device-Public-Key`, `X-Device-Key-Id`), then the Phase 3 signed-envelope
/// headers (`X-Device-Nonce`, `X-Device-Sig`), all sourced from the device's
/// Ed25519 keypair. The portal pins the uploaded public key first-write-wins
/// (TOFU); once enrolled it requires a valid signature over the canonical
/// request bytes plus the single-use nonce. `nonce` is the device's current
/// nonce or the empty string after a reboot (which drives the `nonce_stale`
/// recovery path); `sig` is `device_auth.sign(canonical_http(...))`. Pure +
/// ordered so the wire contract is unit-testable without spawning a subprocess.
///
/// When `nonce` is empty the `X-Device-Nonce` header is OMITTED entirely (not
/// sent with an empty value): busybox HTTP clients on the router
/// (uclient-fetch/wget, no curl) reject a header with an empty value before the
/// request is sent, which would permanently wedge post-reboot recovery. The
/// signature is still computed over the empty-nonce canonical bytes, and the
/// portal treats an absent `X-Device-Nonce` header as the empty string `""`, so
/// the canonical bytes still match — parity holds.
fn heartbeat_headers(
    device_auth: &DeviceAuth,
    nonce: &str,
    sig: &str,
) -> Vec<(&'static str, String)> {
    let mut headers = vec![
        ("Content-Type", "application/json".to_string()),
        ("X-Device-Public-Key", device_auth.public_key_b64.clone()),
        ("X-Device-Key-Id", device_auth.key_id.clone()),
    ];
    if !nonce.is_empty() {
        headers.push(("X-Device-Nonce", nonce.to_string()));
    }
    headers.push(("X-Device-Sig", sig.to_string()));
    headers
}

/// Extract the portal-issued `next_nonce` from a heartbeat response body.
///
/// Returns `Some(nonce)` when the body is JSON with a string `next_nonce`
/// field, `None` otherwise (missing field or non-JSON) — never panics. The
/// portal seeds the first nonce in the TOFU enrollment response (Item #3
/// Phase 2); Phase 3 will sign with it.
fn extract_next_nonce(body: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v.get("next_nonce")
                .and_then(|n| n.as_str())
                .map(|s| s.to_string())
        })
}

/// Cached system info (never changes after boot).
struct SystemInfo {
    board_model: Option<String>,
    openwrt_version: Option<String>,
}

/// Read the board model from OpenWRT system info files.
async fn read_board_model() -> Option<String> {
    // Try /tmp/sysinfo/board_name first (OpenWRT standard)
    if let Ok(contents) = tokio::fs::read_to_string("/tmp/sysinfo/board_name").await {
        let trimmed = contents.trim().to_string();
        if !trimmed.is_empty() {
            return Some(trimmed);
        }
    }

    // Fallback: parse /etc/board.json for model.id
    if let Ok(contents) = tokio::fs::read_to_string("/etc/board.json").await {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&contents) {
            if let Some(model_id) = json.get("model").and_then(|m| m.get("id")).and_then(|v| v.as_str()) {
                let trimmed = model_id.trim().to_string();
                if !trimmed.is_empty() {
                    return Some(trimmed);
                }
            }
        }
    }

    None
}

/// Read the OpenWRT version from /etc/openwrt_release.
///
/// Parses multiple fields to build a useful version string. Some forks
/// (e.g., Q-WRT) put only their brand in DISTRIB_DESCRIPTION, losing the
/// base OpenWrt version. We combine fields for a complete picture:
/// - If DESCRIPTION already contains the release version, use it as-is.
/// - Otherwise, build: "DESCRIPTION (OpenWrt RELEASE, REVISION)"
async fn read_openwrt_version() -> Option<String> {
    let contents = tokio::fs::read_to_string("/etc/openwrt_release").await.ok()?;

    let mut description = None;
    let mut release = None;
    let mut revision = None;

    for line in contents.lines() {
        let parse = |prefix: &str, line: &str| -> Option<String> {
            line.strip_prefix(prefix)
                .map(|v| v.trim_matches(|c| c == '\'' || c == '"').trim().to_string())
                .filter(|v| !v.is_empty())
        };

        if description.is_none() {
            description = parse("DISTRIB_DESCRIPTION=", line);
        }
        if release.is_none() {
            release = parse("DISTRIB_RELEASE=", line);
        }
        if revision.is_none() {
            revision = parse("DISTRIB_REVISION=", line);
        }
    }

    match (description, release, revision) {
        // Description already includes the release info (standard OpenWrt)
        (Some(desc), Some(ref rel), _) if desc.contains(rel) => Some(desc),
        // Description is a brand name (Q-WRT, iStoreOS, etc.) — enrich it
        (Some(desc), Some(rel), Some(rev)) => Some(format!("{desc} (OpenWrt {rel}, {rev})")),
        (Some(desc), Some(rel), None) => Some(format!("{desc} (OpenWrt {rel})")),
        // No description but have release
        (None, Some(rel), Some(rev)) => Some(format!("OpenWrt {rel} {rev}")),
        (None, Some(rel), None) => Some(format!("OpenWrt {rel}")),
        // Just description
        (Some(desc), None, _) => Some(desc),
        _ => None,
    }
}

/// POST the heartbeat JSON payload using the best available HTTP client.
/// Mirrors the download_file pattern from system.rs but does a POST instead.
///
/// TLS certificates are validated against the system CA bundle (the
/// `ca-bundle` package is a hard dependency); none of the tools below skip
/// certificate verification.
async fn post_heartbeat(
    url: &str,
    json_payload: &str,
    headers: &[(&'static str, String)],
) -> Result<String, String> {
    // Write payload to a temp file for tools that need file input
    let tmp_path = "/tmp/ctrl-modem-heartbeat.json";
    tokio::fs::write(tmp_path, json_payload)
        .await
        .map_err(|e| format!("Failed to write temp payload: {e}"))?;

    // Pre-render the per-tool header arguments. uclient-fetch/wget use
    // `--header "Name: value"`; curl uses `-H "Name: value"`. Same ordered set
    // (Content-Type + the Item #3 X-Device-* enrollment headers) for all three.
    let mut header_wget: Vec<String> = Vec::with_capacity(headers.len() * 2);
    let mut header_curl: Vec<String> = Vec::with_capacity(headers.len() * 2);
    for (name, value) in headers {
        let pair = format!("{name}: {value}");
        header_wget.push("--header".to_string());
        header_wget.push(pair.clone());
        header_curl.push("-H".to_string());
        header_curl.push(pair);
    }

    // Try uclient-fetch first (OpenWRT native)
    let result = Command::new("uclient-fetch")
        .arg("-q")
        .args(["--post-file", tmp_path])
        .args(&header_wget)
        .args(["-O", "-"])
        .arg(url)
        .output()
        .await;
    if let Ok(output) = result {
        if output.status.success() {
            let _ = tokio::fs::remove_file(tmp_path).await;
            return Ok(String::from_utf8_lossy(&output.stdout).to_string());
        }
    }

    // Fallback: wget
    let result = Command::new("wget")
        .arg("-q")
        .args(["--post-file", tmp_path])
        .args(&header_wget)
        .args(["-T", "30", "-O", "-"])
        .arg(url)
        .output()
        .await;
    if let Ok(output) = result {
        if output.status.success() {
            let _ = tokio::fs::remove_file(tmp_path).await;
            return Ok(String::from_utf8_lossy(&output.stdout).to_string());
        }
    }

    // Fallback: curl
    let result = Command::new("curl")
        .args(["-sS", "-X", "POST"])
        .args(&header_curl)
        .args(["-d", &format!("@{tmp_path}")])
        .args(["--connect-timeout", "30"])
        .arg(url)
        .output()
        .await;
    if let Ok(output) = result {
        if output.status.success() {
            let _ = tokio::fs::remove_file(tmp_path).await;
            return Ok(String::from_utf8_lossy(&output.stdout).to_string());
        }
        let err = String::from_utf8_lossy(&output.stderr);
        let _ = tokio::fs::remove_file(tmp_path).await;
        return Err(format!("curl failed: {}", err.trim()));
    }

    let _ = tokio::fs::remove_file(tmp_path).await;
    Err("All HTTP methods failed for heartbeat POST".to_string())
}

/// Spawn the portal heartbeat background task.
///
/// Sends a heartbeat every 30 minutes when the device is licensed.
/// Skips silently if unlicensed or in mock/dev mode.
pub fn spawn_heartbeat_task(state: Arc<AppState>) {
    // Skip heartbeat entirely in mock/dev mode
    if std::env::var("MOCK_HARDWARE").is_ok() {
        debug!("Heartbeat task disabled in mock/dev mode");
        return;
    }

    tokio::spawn(async move {
        // Cache system info (read once, never changes)
        let sys_info: OnceCell<SystemInfo> = OnceCell::new();

        // Initial delay — let the system stabilize after boot
        tokio::time::sleep(INITIAL_DELAY).await;

        loop {
            // Check if fast mode has expired — revert to default interval
            {
                let mut fast_until = state.fast_mode_until.write().await;
                if let Some(until) = *fast_until {
                    if until <= tokio::time::Instant::now() {
                        *fast_until = None;
                        state.heartbeat_interval_secs.store(
                            DEFAULT_HEARTBEAT_INTERVAL_SECS,
                            std::sync::atomic::Ordering::Relaxed,
                        );
                        info!("Fast mode expired, reverted to normal heartbeat interval");
                    }
                }
            }

            // Check for poll-now one-shot
            let poll_now = state.poll_now_pending.swap(false, std::sync::atomic::Ordering::Relaxed);

            if !poll_now {
                // Sleep for the dynamic interval
                let interval_secs = state.heartbeat_interval_secs.load(std::sync::atomic::Ordering::Relaxed);
                tokio::time::sleep(std::time::Duration::from_secs(interval_secs)).await;

                // Re-check poll_now after waking (may have been set during sleep)
                let _ = state.poll_now_pending.swap(false, std::sync::atomic::Ordering::Relaxed);

                // Re-check fast mode expiry after sleep
                {
                    let mut fast_until = state.fast_mode_until.write().await;
                    if let Some(until) = *fast_until {
                        if until <= tokio::time::Instant::now() {
                            *fast_until = None;
                            state.heartbeat_interval_secs.store(
                                DEFAULT_HEARTBEAT_INTERVAL_SECS,
                                std::sync::atomic::Ordering::Relaxed,
                            );
                            info!("Fast mode expired, reverted to normal heartbeat interval");
                        }
                    }
                }
            } else {
                info!("Poll-now triggered, sending immediate heartbeat");
            }

            // Only heartbeat if licensed
            let is_licensed = {
                let license = state.license_state.read().await;
                matches!(*license, crate::security::license::LicenseState::Valid { .. })
            };

            if !is_licensed {
                debug!("Heartbeat skipped: device not licensed");
                continue;
            }

            // Get or initialize cached system info
            let info = sys_info
                .get_or_init(|| async {
                    SystemInfo {
                        board_model: read_board_model().await,
                        openwrt_version: read_openwrt_version().await,
                    }
                })
                .await;

            // Build payload
            let mut payload = serde_json::json!({
                "device_token": state.device_token,
            });

            if let Some(ref model) = info.board_model {
                payload["board_model"] = serde_json::json!(model);
            }
            if let Some(ref version) = info.openwrt_version {
                payload["openwrt_version"] = serde_json::json!(version);
            }
            payload["software_version"] = serde_json::json!(env!("CARGO_PKG_VERSION"));

            // Drain telemetry buffer
            let telemetry_snapshots: Vec<crate::hardware::TelemetrySnapshot> = {
                let mut buf = state.telemetry_buffer.write().await;
                buf.drain(..).collect()
            };

            if !telemetry_snapshots.is_empty() {
                payload["telemetry"] = serde_json::to_value(&telemetry_snapshots)
                    .unwrap_or(serde_json::Value::Null);
            }

            // Drain speedtest results buffer
            let speedtest_results: Vec<crate::hardware::SpeedtestResult> = {
                let mut buf = state.speedtest_buffer.write().await;
                buf.drain(..).collect()
            };

            if !speedtest_results.is_empty() {
                payload["speedtest_results"] = serde_json::to_value(&speedtest_results)
                    .unwrap_or(serde_json::Value::Null);
            }

            // Build the heartbeat body bytes ONCE (the telemetry/speedtest
            // buffers are already drained above). On a nonce_stale retry we
            // re-sign and re-POST these SAME bytes — never re-drain.
            let json_str = payload.to_string();

            let heartbeat_url = state.config.read().await.portal.heartbeat_url();

            // Item #3 Phase 3: sign the canonical request bytes with the
            // device's Ed25519 key. The current nonce (empty string after a
            // reboot) is folded into the signed bytes; the empty-nonce case is
            // what triggers the portal's `nonce_stale` recovery reply.
            let send_signed = |nonce: String| {
                let url = heartbeat_url.clone();
                let body = json_str.clone();
                let device_auth = state.device_auth.clone();
                let token = state.device_token.clone();
                async move {
                    let canonical = crate::security::device_auth::canonical_http(
                        &token,
                        &nonce,
                        "POST",
                        "/api/v1/heartbeat",
                        body.as_bytes(),
                    );
                    let sig = device_auth.sign(&canonical);
                    let headers = heartbeat_headers(&device_auth, &nonce, &sig);
                    post_heartbeat(&url, &body, &headers).await
                }
            };

            let nonce = state.device_nonce.read().await.clone().unwrap_or_default();

            let send_result = match send_signed(nonce).await {
                Ok(body) => {
                    // Item #3 Phase 3 recovery (spec §13): a 200 `nonce_stale`
                    // body means our nonce was stale/empty/replayed. Re-sign the
                    // SAME canonical request with the fresh nonce and retry ONCE.
                    if let Some(fresh) =
                        crate::security::device_auth::parse_nonce_stale(&body)
                    {
                        debug!("Portal replied nonce_stale; re-signing with fresh nonce and retrying once");
                        *state.device_nonce.write().await = Some(fresh.clone());
                        send_signed(fresh).await
                    } else {
                        Ok(body)
                    }
                }
                Err(e) => Err(e),
            };

            match send_result {
                Ok(response_body) => {
                    info!("Portal heartbeat sent successfully");

                    // Item #3 Phase 2/3: store the portal-issued replay nonce
                    // (rotated on every successful signed heartbeat) in memory
                    // for the next request's signature. debug-only (short-lived).
                    if let Some(nonce) = extract_next_nonce(&response_body) {
                        debug!("Portal issued next_nonce: {nonce}");
                        *state.device_nonce.write().await = Some(nonce);
                    }

                    // Parse telemetry_enabled from response
                    if let Ok(resp) = serde_json::from_str::<serde_json::Value>(&response_body) {
                        if let Some(enabled) = resp.get("telemetry_enabled").and_then(|v| v.as_bool()) {
                            state.telemetry_portal_enabled.store(enabled, std::sync::atomic::Ordering::Relaxed);
                            debug!("Portal telemetry_enabled: {enabled}");
                        }
                        // Check for license key update from portal
                        if let Some(new_key) = resp.get("license_key").and_then(|v| v.as_str()) {
                            // Compare with stored license — only update if different
                            let stored_key = tokio::fs::read_to_string("/etc/modem-interface/license.key")
                                .await
                                .unwrap_or_default();

                            if new_key.trim() != stored_key.trim() {
                                // Resolve env once for verify + store; lock guard scope is
                                // intentionally narrow because current_env returns &'static str.
                                let env_name = {
                                    let cfg = state.config.read().await;
                                    crate::config::current_env(&cfg.portal.base_url)
                                };
                                // Verify the new key before accepting
                                let new_state = crate::security::license::verify_license(
                                    new_key,
                                    &state.device_token,
                                    env_name,
                                );
                                match &new_state {
                                    crate::security::license::LicenseState::Valid { .. } => {
                                        // Store to disk + sidecar (best-effort sidecar sync per spec §3)
                                        if let Err(e) = crate::security::license::store_license(new_key, env_name).await {
                                            warn!("Failed to store updated license: {e}");
                                        } else {
                                            // Update in-memory state
                                            let mut ls = state.license_state.write().await;
                                            *ls = new_state;
                                            drop(ls);
                                            info!("License updated from portal heartbeat");
                                        }
                                    }
                                    other => {
                                        warn!("Portal sent invalid license key in heartbeat: {:?}", other);
                                    }
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!("Portal heartbeat failed: {e}");
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_key_path(tag: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        p.push(format!("ctrl-hb-headers-{tag}-{n}.key"));
        p
    }

    #[test]
    fn heartbeat_headers_yields_content_type_then_enrollment_then_signed_headers() {
        let path = temp_key_path("hdrs");
        let da = DeviceAuth::load_or_create(&path).unwrap();
        let nonce = "NONCE-XYZ";
        let sig = "SIG-ABC";
        let headers = heartbeat_headers(&da, nonce, sig);

        assert_eq!(headers.len(), 5);
        assert_eq!(headers[0], ("Content-Type", "application/json".to_string()));
        assert_eq!(
            headers[1],
            ("X-Device-Public-Key", da.public_key_b64.clone())
        );
        assert_eq!(headers[2], ("X-Device-Key-Id", da.key_id.clone()));
        assert_eq!(headers[3], ("X-Device-Nonce", nonce.to_string()));
        assert_eq!(headers[4], ("X-Device-Sig", sig.to_string()));

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn heartbeat_headers_omit_nonce_header_when_device_has_none() {
        // Post-reboot recovery case: the device has no nonce. Busybox HTTP
        // clients (uclient-fetch/wget) REJECT a header sent with an empty value
        // (`X-Device-Nonce: ` → usage error, request never sent), so the empty
        // nonce must OMIT the X-Device-Nonce header entirely rather than send it
        // empty. The signature is still computed over the empty-nonce canonical
        // bytes (the portal treats an absent header as ""), so parity holds.
        let path = temp_key_path("hdrs-empty");
        let da = DeviceAuth::load_or_create(&path).unwrap();
        let headers = heartbeat_headers(&da, "", "SIG");
        assert!(
            !headers.iter().any(|(n, _)| *n == "X-Device-Nonce"),
            "X-Device-Nonce must be absent when the nonce is empty: {headers:?}"
        );
        // The other signed-envelope headers are unaffected.
        assert_eq!(headers.len(), 4);
        assert_eq!(headers[0], ("Content-Type", "application/json".to_string()));
        assert_eq!(
            headers[1],
            ("X-Device-Public-Key", da.public_key_b64.clone())
        );
        assert_eq!(headers[2], ("X-Device-Key-Id", da.key_id.clone()));
        assert_eq!(headers[3], ("X-Device-Sig", "SIG".to_string()));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn heartbeat_signature_round_trips_against_public_key() {
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
        use ed25519_dalek::{Signature, Verifier, VerifyingKey};

        let path = temp_key_path("hbsig");
        let da = DeviceAuth::load_or_create(&path).unwrap();
        let nonce = "NONCE456";
        let body = br#"{"device_token":"TOKEN123"}"#;
        let canonical = crate::security::device_auth::canonical_http(
            "TOKEN123",
            nonce,
            "POST",
            "/api/v1/heartbeat",
            body,
        );
        let sig_b64 = da.sign(&canonical);

        // The signature carried in X-Device-Sig must verify against the public
        // key over the exact canonical bytes (mirrors the portal verifier).
        let headers = heartbeat_headers(&da, nonce, &sig_b64);
        assert_eq!(headers[4].0, "X-Device-Sig");
        assert_eq!(headers[4].1, sig_b64);

        let pk_bytes: [u8; 32] = URL_SAFE_NO_PAD
            .decode(&da.public_key_b64)
            .unwrap()
            .try_into()
            .unwrap();
        let vk = VerifyingKey::from_bytes(&pk_bytes).unwrap();
        let sig_bytes: [u8; 64] = URL_SAFE_NO_PAD
            .decode(&sig_b64)
            .unwrap()
            .try_into()
            .unwrap();
        assert!(vk
            .verify_strict(&canonical, &Signature::from_bytes(&sig_bytes))
            .is_ok());

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn extract_next_nonce_present() {
        let body = r#"{"status":"ok","next_nonce":"ABC123"}"#;
        assert_eq!(extract_next_nonce(body), Some("ABC123".to_string()));
    }

    #[test]
    fn extract_next_nonce_missing_field() {
        let body = r#"{"status":"ok","telemetry_enabled":true}"#;
        assert_eq!(extract_next_nonce(body), None);
    }

    #[test]
    fn extract_next_nonce_non_json_does_not_panic() {
        assert_eq!(extract_next_nonce("not json at all"), None);
        assert_eq!(extract_next_nonce(""), None);
    }
}
