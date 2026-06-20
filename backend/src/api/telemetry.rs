//! Telemetry collector background task.
//!
//! Snapshots the master cache every 5 minutes into a ring buffer of 6 entries.
//! The heartbeat task (Task 4) drains this buffer and sends it to the portal.

use std::collections::VecDeque;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use chrono::Utc;
use tokio::sync::RwLock;
use tokio::time::Instant;
use tracing::debug;

use axum::{extract::State, Extension, Json};
use serde::{Deserialize, Serialize};
use tokio::process::Command;

use crate::api::auth_middleware::{require_admin, SessionUser};
use crate::api::error::ApiError;
use crate::hardware::{TelemetrySnapshot, Technology, WanEntryType, WanModemState};
use crate::state::AppState;

/// Ring buffer holding up to 6 telemetry snapshots (5 min * 6 = 30 min window).
pub type TelemetryBuffer = Arc<RwLock<VecDeque<TelemetrySnapshot>>>;

/// Maximum number of snapshots retained in the buffer.
const BUFFER_CAPACITY: usize = 6;

/// Interval between snapshots.
const SNAPSHOT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5 * 60);

/// Initial delay before collecting (let master cache populate).
const INITIAL_DELAY: std::time::Duration = std::time::Duration::from_secs(90);

/// Create an empty telemetry buffer with capacity 6.
pub fn new_buffer() -> TelemetryBuffer {
    Arc::new(RwLock::new(VecDeque::with_capacity(BUFFER_CAPACITY)))
}

/// Convert a Technology enum to its human-readable string.
fn technology_to_string(tech: &Technology) -> String {
    match tech {
        Technology::Gen2 => "2G".to_string(),
        Technology::Gen3 => "3G".to_string(),
        Technology::Gen4 => "4G".to_string(),
        Technology::Gen5 => "5G".to_string(),
    }
}

/// Read device uptime from /proc/uptime (Linux only).
/// Returns 0 on error (e.g., on non-Linux dev machines).
async fn read_device_uptime_secs() -> u64 {
    match tokio::fs::read_to_string("/proc/uptime").await {
        Ok(contents) => contents
            .split_whitespace()
            .next()
            .and_then(|s| s.parse::<f64>().ok())
            .map(|f| f as u64)
            .unwrap_or(0),
        Err(_) => 0,
    }
}

/// Check if the most recent failover event happened within the last 5 minutes.
async fn check_recent_failover(state: &AppState) -> bool {
    let runtime = state.wan_runtime.read().await;
    if let Some(event) = runtime.failover_history.front() {
        // Parse ISO 8601 timestamp and check if within 5 minutes
        if let Ok(ts) = chrono::DateTime::parse_from_rfc3339(&event.timestamp) {
            let age = Utc::now().signed_duration_since(ts);
            return age.num_seconds() < 300;
        }
    }
    false
}

/// Find the active WAN interface name from WAN config.
async fn find_active_wan(state: &AppState) -> Option<String> {
    let wan_config = state.wan_config.read().await;
    wan_config
        .modem_priority
        .iter()
        .find(|entry| entry.state == WanModemState::Active)
        .map(|entry| entry.interface_name.clone())
}

/// Response for GET /api/telemetry/config
#[derive(serde::Serialize)]
pub struct TelemetryConfigResponse {
    /// Local opt-in toggle
    pub local_enabled: bool,
    /// Portal-side flag (read-only from router perspective)
    pub portal_enabled: bool,
    /// Whether telemetry is actively collecting (both gates on)
    pub active: bool,
}

/// Request for PUT /api/telemetry/config
#[derive(serde::Deserialize)]
pub struct TelemetryConfigRequest {
    pub enabled: bool,
}

/// GET /ctrl-modem/api/telemetry/config
pub async fn get_telemetry_config(
    State(state): State<Arc<AppState>>,
) -> Json<TelemetryConfigResponse> {
    let local_enabled = {
        let config = state.config.read().await;
        config.telemetry_enabled
    };
    let portal_enabled = state.telemetry_portal_enabled.load(Ordering::Relaxed);

    Json(TelemetryConfigResponse {
        local_enabled,
        portal_enabled,
        active: local_enabled && portal_enabled,
    })
}

/// PUT /ctrl-modem/api/telemetry/config
pub async fn update_telemetry_config(
    State(state): State<Arc<AppState>>,
    Extension(session_user): Extension<SessionUser>,
    Json(body): Json<TelemetryConfigRequest>,
) -> Result<Json<TelemetryConfigResponse>, ApiError> {
    require_admin(&session_user)?;

    {
        let mut config = state.config.write().await;
        config.telemetry_enabled = body.enabled;
    }

    // Persist to disk
    let config = state.config.read().await;
    if let Err(e) = crate::config::save_config(&config).await {
        tracing::warn!("Failed to persist telemetry config: {e}");
    }

    let portal_enabled = state.telemetry_portal_enabled.load(Ordering::Relaxed);

    Ok(Json(TelemetryConfigResponse {
        local_enabled: body.enabled,
        portal_enabled,
        active: body.enabled && portal_enabled,
    }))
}

/// Spawn the telemetry collector background task.
///
/// Every 5 minutes, snapshots all modem caches into the ring buffer.
/// The buffer is capped at 6 entries; oldest entries are dropped when full.
/// Both `config.telemetry_enabled` and `telemetry_portal_enabled` must be true.
pub fn spawn_telemetry_collector(state: Arc<AppState>, buffer: TelemetryBuffer) {
    // Skip entirely in mock/dev mode
    if std::env::var("MOCK_HARDWARE").is_ok() {
        debug!("Telemetry collector disabled in mock/dev mode");
        return;
    }

    tokio::spawn(async move {
        // Wait 30s for master cache to populate, then capture one early snapshot
        // so the first heartbeat (at ~60s) has data available.
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;

        // Track connection uptime per modem: modem_id -> when it became connected
        let mut connection_start: std::collections::HashMap<String, Instant> = std::collections::HashMap::new();

        // Capture the early snapshot (same logic as the loop body)
        capture_snapshot(&state, &buffer, &mut connection_start).await;

        // Wait the remainder of the initial delay
        let remaining = INITIAL_DELAY.saturating_sub(std::time::Duration::from_secs(30));
        tokio::time::sleep(remaining).await;

        let mut interval = tokio::time::interval(SNAPSHOT_INTERVAL);

        loop {
            interval.tick().await;

            capture_snapshot(&state, &buffer, &mut connection_start).await;
        }
    });
}

/// Capture a single telemetry snapshot from all modems into the buffer.
/// Gated on both config.telemetry_enabled and telemetry_portal_enabled.
async fn capture_snapshot(
    state: &AppState,
    buffer: &TelemetryBuffer,
    connection_start: &mut std::collections::HashMap<String, Instant>,
) {
    // Gate 1: config.telemetry_enabled
    let config_enabled = {
        let config = state.config.read().await;
        config.telemetry_enabled
    };
    if !config_enabled {
        debug!("Telemetry collector skipped: config.telemetry_enabled=false");
        return;
    }

    // Gate 2: portal has enabled telemetry for this device
    if !state.telemetry_portal_enabled.load(Ordering::Relaxed) {
        debug!("Telemetry collector skipped: portal telemetry not enabled");
        return;
    }

    // Collect snapshots from all modems
    let active_wan = find_active_wan(state).await;
    let failover_event = check_recent_failover(state).await;
    let device_uptime_secs = read_device_uptime_secs().await;

    let modems = state.modems.read().await;
    for (modem_id, context) in modems.iter() {
        let cache_guard = context.state_cache.read().await;
        let cache = match cache_guard.as_ref() {
            Some(c) => c,
            None => continue,
        };

        let connected = cache.connection.connected;

        let connection_uptime_secs = if connected {
            let start = connection_start
                .entry(modem_id.clone())
                .or_insert_with(Instant::now);
            start.elapsed().as_secs()
        } else {
            connection_start.remove(modem_id);
            0
        };

        let technology = cache.signal.technology.as_ref().map(technology_to_string);

        // Read IMEI from boot/discovery data
        let wan_id = {
            let discovery = context.discovery.read().await;
            let imei = &discovery.device_info.imei;
            if imei.is_empty() {
                modem_id.clone()
            } else {
                imei.clone()
            }
        };

        // Query extended signal for CA bands (only at telemetry capture, not 60s cache)
        let extended = {
            let lock_result = tokio::time::timeout(
                std::time::Duration::from_secs(2),
                context.handler.lock(),
            ).await;
            match lock_result {
                Ok(modem) => {
                    tokio::time::timeout(
                        std::time::Duration::from_secs(10),
                        modem.get_extended_signal(),
                    ).await.ok().and_then(|r| r.ok())
                }
                Err(_) => None,
            }
        };

        // Build bands array with PCC-first ordering and extract network_type
        let (bands, network_type) = if let Some(ref ext) = extended {
            let mut all_bands: Vec<String> = Vec::new();
            let primary_band = ext.primary.band.clone();
            let secondary_bands: Vec<String> = ext.secondary_cells.iter()
                .map(|s| s.band.clone())
                .filter(|b| !b.is_empty())
                .collect();

            let has_lte = std::iter::once(&primary_band).chain(secondary_bands.iter())
                .any(|b| b.starts_with('B'));
            let has_5g = std::iter::once(&primary_band).chain(secondary_bands.iter())
                .any(|b| b.starts_with('n'));

            if has_lte && has_5g {
                // NSA mode: LTE PCC first, then LTE SCCs, then 5G SCCs
                // LTE is always PCC in NSA, even if AT response says otherwise
                let mut lte_bands: Vec<String> = Vec::new();
                let mut nr_bands: Vec<String> = Vec::new();
                for b in std::iter::once(&primary_band).chain(secondary_bands.iter()) {
                    if b.starts_with('B') && !lte_bands.contains(b) {
                        lte_bands.push(b.clone());
                    } else if b.starts_with('n') && !nr_bands.contains(b) {
                        nr_bands.push(b.clone());
                    }
                }
                all_bands.extend(lte_bands);
                all_bands.extend(nr_bands);
            } else {
                // SA or LTE-only: primary first, then secondaries
                if !primary_band.is_empty() {
                    all_bands.push(primary_band);
                }
                for b in &secondary_bands {
                    if !all_bands.contains(b) {
                        all_bands.push(b.clone());
                    }
                }
            }

            let net_type = if ext.network_type.is_empty() { None } else { Some(ext.network_type.clone()) };
            (all_bands, net_type)
        } else {
            // Fallback: no extended signal available, use cache band
            let mut bands = Vec::new();
            if !cache.signal.band.is_empty() {
                bands.push(cache.signal.band.clone());
            }
            (bands, None)
        };

        let modem_name = Some(context.profile.identity.model.clone());

        let snapshot = TelemetrySnapshot {
            wan_id,
            wan_type: "modem".to_string(),
            bands,
            network_type,
            modem_name,
            recorded_at: Utc::now().to_rfc3339(),
            rsrp: cache.signal.rsrp,
            rsrq: cache.signal.rsrq,
            sinr: cache.signal.sinr,
            rssi: cache.signal.rssi,
            technology,
            operator: cache.connection.operator.clone(),
            connected,
            active_wan: active_wan.clone(),
            failover_event,
            device_uptime_secs,
            connection_uptime_secs,
        };

        let mut buf = buffer.write().await;
        if buf.len() >= BUFFER_CAPACITY {
            buf.pop_front();
        }
        buf.push_back(snapshot);
    }

    drop(modems);

    // Capture Ethernet WAN snapshots
    let wan_config = state.wan_config.read().await;
    for entry in &wan_config.modem_priority {
        if entry.entry_type != WanEntryType::Ethernet {
            continue;
        }

        let connected = entry.state == WanModemState::Active;

        let connection_uptime_secs = if connected {
            let start = connection_start
                .entry(entry.modem_id.clone())
                .or_insert_with(Instant::now);
            start.elapsed().as_secs()
        } else {
            connection_start.remove(&entry.modem_id);
            0
        };

        let snapshot = TelemetrySnapshot {
            wan_id: entry.modem_id.clone(),
            wan_type: "ethernet".to_string(),
            bands: vec![],
            network_type: None,
            modem_name: None,
            recorded_at: Utc::now().to_rfc3339(),
            rsrp: 0.0,
            rsrq: 0.0,
            sinr: 0.0,
            rssi: 0.0,
            technology: None,
            operator: None,
            connected,
            active_wan: Some(entry.interface_name.clone()),
            failover_event,
            device_uptime_secs,
            connection_uptime_secs,
        };

        let mut buf = buffer.write().await;
        if buf.len() >= BUFFER_CAPACITY {
            buf.pop_front();
        }
        buf.push_back(snapshot);
    }

    debug!("Telemetry collector: captured snapshot");
}

// ---------------------------------------------------------------------------
// Polling control API endpoints
// ---------------------------------------------------------------------------

/// Available fast-mode intervals (seconds).
const FAST_INTERVALS: [u64; 3] = [120, 300, 600];

/// Default heartbeat interval (30 minutes).
const DEFAULT_HEARTBEAT_INTERVAL: u64 = 30 * 60;

/// Fast mode auto-revert duration (30 minutes).
const FAST_MODE_DURATION: std::time::Duration = std::time::Duration::from_secs(30 * 60);

/// Response for GET /api/telemetry/polling
#[derive(Serialize)]
pub struct PollingStateResponse {
    pub mode: String,
    pub interval_secs: u64,
    pub fast_mode_remaining_secs: Option<u64>,
    pub options: Vec<u64>,
}

/// Request for PUT /api/telemetry/polling
#[derive(Deserialize)]
pub struct PollingModeRequest {
    pub mode: String,
    pub interval_secs: Option<u64>,
}

/// Response for POST /api/telemetry/poll-now
#[derive(Serialize)]
pub struct PollNowResponse {
    pub queued: bool,
}

/// GET /ctrl-modem/api/telemetry/polling
pub async fn get_telemetry_polling(
    State(state): State<Arc<AppState>>,
) -> Json<PollingStateResponse> {
    let interval_secs = state.heartbeat_interval_secs.load(Ordering::Relaxed);
    let fast_mode_until = state.fast_mode_until.read().await;

    let (mode, remaining) = if let Some(until) = *fast_mode_until {
        let remaining = until.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            ("normal".to_string(), None)
        } else {
            ("fast".to_string(), Some(remaining.as_secs()))
        }
    } else {
        ("normal".to_string(), None)
    };

    Json(PollingStateResponse {
        mode,
        interval_secs,
        fast_mode_remaining_secs: remaining,
        options: FAST_INTERVALS.to_vec(),
    })
}

/// PUT /ctrl-modem/api/telemetry/polling
pub async fn update_telemetry_polling(
    State(state): State<Arc<AppState>>,
    Extension(session_user): Extension<SessionUser>,
    Json(body): Json<PollingModeRequest>,
) -> Result<Json<PollingStateResponse>, ApiError> {
    require_admin(&session_user)?;

    match body.mode.as_str() {
        "fast" => {
            let interval = body.interval_secs.unwrap_or(300);
            // Validate interval is one of the allowed options
            let interval = if FAST_INTERVALS.contains(&interval) {
                interval
            } else {
                300 // default to 5 min if invalid
            };
            state.heartbeat_interval_secs.store(interval, Ordering::Relaxed);
            let until = Instant::now() + FAST_MODE_DURATION;
            *state.fast_mode_until.write().await = Some(until);
            tracing::info!("Telemetry polling: fast mode enabled ({}s interval, 30min auto-revert)", interval);

            Ok(Json(PollingStateResponse {
                mode: "fast".to_string(),
                interval_secs: interval,
                fast_mode_remaining_secs: Some(FAST_MODE_DURATION.as_secs()),
                options: FAST_INTERVALS.to_vec(),
            }))
        }
        _ => {
            // Normal mode — revert to default
            state.heartbeat_interval_secs.store(DEFAULT_HEARTBEAT_INTERVAL, Ordering::Relaxed);
            *state.fast_mode_until.write().await = None;
            tracing::info!("Telemetry polling: reverted to normal mode (30min interval)");

            Ok(Json(PollingStateResponse {
                mode: "normal".to_string(),
                interval_secs: DEFAULT_HEARTBEAT_INTERVAL,
                fast_mode_remaining_secs: None,
                options: FAST_INTERVALS.to_vec(),
            }))
        }
    }
}

/// POST /ctrl-modem/api/telemetry/poll-now
pub async fn trigger_poll_now(
    State(state): State<Arc<AppState>>,
    Extension(session_user): Extension<SessionUser>,
) -> Result<Json<PollNowResponse>, ApiError> {
    require_admin(&session_user)?;
    state.poll_now_pending.store(true, Ordering::Relaxed);
    tracing::info!("Telemetry polling: poll-now queued");
    Ok(Json(PollNowResponse { queued: true }))
}

// ---------------------------------------------------------------------------
// Remote config poll
// ---------------------------------------------------------------------------

/// Interval between remote config polls (5 minutes).
const REMOTE_CONFIG_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5 * 60);

/// Response from the portal's poll-config endpoint.
#[derive(Deserialize)]
struct RemoteConfigResponse {
    fast_mode: bool,
    poll_now: bool,
    interval_secs: Option<u64>,
}

/// Extract the portal-issued `next_nonce` from a poll-config response body.
///
/// Returns `Some(nonce)` when the body is JSON with a string `next_nonce`
/// field (the success path piggybacks the rotated nonce), `None` otherwise —
/// never panics. Distinct from `parse_nonce_stale`, which only matches the
/// recoverable `status == "nonce_stale"` reply.
fn extract_next_nonce(body: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v.get("next_nonce")
                .and_then(|n| n.as_str())
                .map(|s| s.to_string())
        })
}

/// Build the ordered signed-envelope headers for the poll-config GET.
///
/// Item #3 Phase 3: the GET carries `X-Device-Token` (identity, still NEVER in
/// the URL query string — it would leak into proxy/access logs), `X-Device-Key-Id`,
/// `X-Device-Nonce` (the device's current nonce, or the empty string after a
/// reboot → drives the `nonce_stale` recovery path), and `X-Device-Sig` (the
/// Ed25519 signature over the canonical GET bytes). Pure + ordered so the wire
/// contract is unit-testable.
///
/// When `nonce` is empty the `X-Device-Nonce` header is OMITTED entirely (not
/// sent with an empty value): busybox HTTP clients on the router
/// (uclient-fetch/wget, no curl) reject a header with an empty value before the
/// request is sent, which would permanently wedge post-reboot recovery. The
/// signature is still over the empty-nonce canonical bytes, and the portal
/// treats an absent `X-Device-Nonce` header as the empty string `""`, so the
/// canonical bytes still match — parity holds.
fn poll_config_headers(
    device_token: &str,
    key_id: &str,
    nonce: &str,
    sig: &str,
) -> Vec<(&'static str, String)> {
    let mut headers = vec![
        ("X-Device-Token", device_token.to_string()),
        ("X-Device-Key-Id", key_id.to_string()),
    ];
    if !nonce.is_empty() {
        headers.push(("X-Device-Nonce", nonce.to_string()));
    }
    headers.push(("X-Device-Sig", sig.to_string()));
    headers
}

/// GET a remote config JSON using the same HTTP fallback chain as heartbeat.
///
/// The signed envelope (token + key-id + nonce + signature) is passed as
/// request headers (mirroring how `heartbeat.rs` threads custom headers through
/// all three tools); the token is deliberately kept out of `url`. Each arg is a
/// separate argv element — no value is interpolated into a shell string.
///
/// TLS certificates are validated against the system CA bundle (the
/// `ca-bundle` package is a hard dependency); none of the tools below skip
/// certificate verification.
async fn get_remote_config(
    url: &str,
    headers: &[(&'static str, String)],
) -> Result<String, String> {
    // Pre-render the per-tool header arguments. uclient-fetch/wget use
    // `--header "Name: value"`; curl uses `-H "Name: value"`.
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
        .args(["-q"])
        .args(&header_wget)
        .args(["-O", "-"])
        .arg(url)
        .output()
        .await;
    if let Ok(output) = result {
        if output.status.success() {
            return Ok(String::from_utf8_lossy(&output.stdout).to_string());
        }
    }

    // Fallback: wget
    let result = Command::new("wget")
        .args(["-q", "-T", "30"])
        .args(&header_wget)
        .args(["-O", "-"])
        .arg(url)
        .output()
        .await;
    if let Ok(output) = result {
        if output.status.success() {
            return Ok(String::from_utf8_lossy(&output.stdout).to_string());
        }
    }

    // Fallback: curl
    let result = Command::new("curl")
        .args(["-sS", "--connect-timeout", "30"])
        .args(&header_curl)
        .arg(url)
        .output()
        .await;
    if let Ok(output) = result {
        if output.status.success() {
            return Ok(String::from_utf8_lossy(&output.stdout).to_string());
        }
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(format!("curl failed: {}", err.trim()));
    }

    Err("All HTTP methods failed for remote config GET".to_string())
}

/// Spawn the remote config poll background task.
///
/// Every 5 minutes, checks the portal for remote commands (poll-now, fast mode).
/// Skips in mock/dev mode or when not licensed.
pub fn spawn_remote_config_poll(state: Arc<AppState>) {
    if std::env::var("MOCK_HARDWARE").is_ok() {
        debug!("Remote config poll disabled in mock/dev mode");
        return;
    }

    tokio::spawn(async move {
        // Initial delay — let the system stabilize
        tokio::time::sleep(std::time::Duration::from_secs(120)).await;

        loop {
            tokio::time::sleep(REMOTE_CONFIG_POLL_INTERVAL).await;

            // Only poll if licensed
            let is_licensed = {
                let license = state.license_state.read().await;
                matches!(*license, crate::security::license::LicenseState::Valid { .. })
            };
            if !is_licensed {
                continue;
            }

            // Bare poll-config URL — the device token MUST NOT live in the URL
            // (it would leak into proxy/access logs); the full signed envelope
            // (token + key-id + nonce + signature) goes in headers inside
            // get_remote_config instead.
            let url = state.config.read().await.portal.poll_config_url();

            // Item #3 Phase 3: sign the canonical GET bytes with the device key.
            // The current nonce is empty after a reboot, which drives the
            // portal's `nonce_stale` recovery reply.
            let send_signed = |nonce: String| {
                let url = url.clone();
                let device_auth = state.device_auth.clone();
                let token = state.device_token.clone();
                async move {
                    let canonical = crate::security::device_auth::canonical_http(
                        &token,
                        &nonce,
                        "GET",
                        "/api/v1/device/poll-config",
                        b"",
                    );
                    let sig = device_auth.sign(&canonical);
                    let headers = poll_config_headers(&token, &device_auth.key_id, &nonce, &sig);
                    get_remote_config(&url, &headers).await
                }
            };

            let nonce = state.device_nonce.read().await.clone().unwrap_or_default();

            let poll_result = match send_signed(nonce).await {
                Ok(body) => {
                    // Item #3 Phase 3 recovery (spec §13): a 200 `nonce_stale`
                    // body means our nonce was stale/empty/replayed. Store the
                    // fresh nonce, re-sign the SAME canonical GET, and retry ONCE.
                    if let Some(fresh) = crate::security::device_auth::parse_nonce_stale(&body) {
                        debug!("Remote config: portal replied nonce_stale; re-signing and retrying once");
                        *state.device_nonce.write().await = Some(fresh.clone());
                        send_signed(fresh).await
                    } else {
                        Ok(body)
                    }
                }
                Err(e) => Err(e),
            };

            match poll_result {
                Ok(body) => {
                    debug!("Remote config poll response: {body}");

                    // On the final body, store the rotated next_nonce (success
                    // path piggybacks it) for the next request's signature. A
                    // nonce_stale body has no usable config fields — handled by
                    // the parse below falling through to the warn branch.
                    if let Some(next) = extract_next_nonce(&body) {
                        *state.device_nonce.write().await = Some(next);
                    }

                    if let Ok(config) = serde_json::from_str::<RemoteConfigResponse>(&body) {
                        // Handle poll-now command
                        if config.poll_now {
                            state.poll_now_pending.store(true, Ordering::Relaxed);
                            tracing::info!("Remote config: poll-now requested by portal");
                        }
                        // Handle fast mode toggle
                        if config.fast_mode {
                            let interval = config.interval_secs.unwrap_or(300);
                            let interval = if FAST_INTERVALS.contains(&interval) {
                                interval
                            } else {
                                300
                            };
                            state.heartbeat_interval_secs.store(interval, Ordering::Relaxed);
                            let until = Instant::now() + FAST_MODE_DURATION;
                            *state.fast_mode_until.write().await = Some(until);
                            tracing::info!("Remote config: fast mode set ({}s interval)", interval);
                        } else {
                            // Portal says fast mode is off — revert to normal if currently in fast mode
                            let was_fast = state.fast_mode_until.read().await.is_some();
                            if was_fast {
                                state.heartbeat_interval_secs.store(DEFAULT_HEARTBEAT_INTERVAL, Ordering::Relaxed);
                                *state.fast_mode_until.write().await = None;
                                tracing::info!("Remote config: reverted to normal mode ({}s interval)", DEFAULT_HEARTBEAT_INTERVAL);
                            }
                        }
                    } else {
                        tracing::warn!("Remote config: failed to parse response: {body}");
                    }
                }
                Err(e) => {
                    tracing::warn!("Remote config poll failed: {e}");
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_token_header_is_exact() {
        // The envelope carries the static device-identity secret as the
        // `X-Device-Token` header (name + value) so the portal can read it; the
        // token must never appear in the URL query string.
        let headers = poll_config_headers("abc123", "KID", "N", "S");
        assert_eq!(headers[0], ("X-Device-Token", "abc123".to_string()));
    }

    #[test]
    fn poll_config_url_carries_no_token() {
        // Regression guard: the device token must NEVER appear in the poll URL
        // query string (it would leak into proxy/access logs). The live poll
        // loop builds the URL from `portal.poll_config_url()` with no `?token=`.
        let cfg = crate::config::portal::PortalConfig::default();
        let url = cfg.poll_config_url();
        assert!(
            !url.contains("token="),
            "poll-config URL must not contain a token query param: {url}"
        );
    }

    #[test]
    fn poll_config_headers_are_ordered_envelope() {
        // Item #3 Phase 3: the poll-config GET carries the full signed envelope
        // (token + key-id + nonce + signature) in headers, with the token still
        // out of the URL. Order is fixed so the wire contract is testable.
        let headers = poll_config_headers("TOK", "KID", "NONCE-XYZ", "SIG-ABC");
        assert_eq!(headers.len(), 4);
        assert_eq!(headers[0], ("X-Device-Token", "TOK".to_string()));
        assert_eq!(headers[1], ("X-Device-Key-Id", "KID".to_string()));
        assert_eq!(headers[2], ("X-Device-Nonce", "NONCE-XYZ".to_string()));
        assert_eq!(headers[3], ("X-Device-Sig", "SIG-ABC".to_string()));
    }

    #[test]
    fn poll_config_headers_omit_nonce_header_when_empty() {
        // Post-reboot recovery: empty nonce, valid signature. Busybox HTTP
        // clients (uclient-fetch/wget) REJECT a header sent with an empty value
        // (`X-Device-Nonce: ` → usage error, request never sent), so the empty
        // nonce must OMIT the X-Device-Nonce header entirely. The signature is
        // still over the empty-nonce canonical bytes (the portal treats an
        // absent header as ""), so parity holds.
        let headers = poll_config_headers("TOK", "KID", "", "SIG");
        assert!(
            !headers.iter().any(|(n, _)| *n == "X-Device-Nonce"),
            "X-Device-Nonce must be absent when the nonce is empty: {headers:?}"
        );
        // The other signed-envelope headers are unaffected.
        assert_eq!(headers.len(), 3);
        assert_eq!(headers[0], ("X-Device-Token", "TOK".to_string()));
        assert_eq!(headers[1], ("X-Device-Key-Id", "KID".to_string()));
        assert_eq!(headers[2], ("X-Device-Sig", "SIG".to_string()));
    }

    #[test]
    fn poll_config_signature_round_trips_against_public_key() {
        use crate::security::device_auth::{canonical_http, DeviceAuth};
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
        use ed25519_dalek::{Signature, Verifier, VerifyingKey};

        let mut p = std::env::temp_dir();
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        p.push(format!("ctrl-poll-sig-{n}.key"));
        let da = DeviceAuth::load_or_create(&p).unwrap();

        let nonce = "NONCE456";
        let canonical = canonical_http("TOKEN123", nonce, "GET", "/api/v1/device/poll-config", b"");
        let sig_b64 = da.sign(&canonical);

        let headers = poll_config_headers("TOKEN123", &da.key_id, nonce, &sig_b64);
        assert_eq!(headers[3].0, "X-Device-Sig");
        assert_eq!(headers[3].1, sig_b64);

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

        std::fs::remove_file(&p).ok();
    }
}
