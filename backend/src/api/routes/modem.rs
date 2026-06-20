//! Modem API route handlers.
//!
//! Handlers for /api/modem/* endpoints including status, signal, connect/disconnect,
//! and AT command execution.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;

use crate::api::error::{ApiError, ApiResult};
use crate::hardware::{
    AirplaneModeRequest, AntennaMetrics, AtCommandRequest, AtCommandResponse, AuthType,
    BandConfigRequest, BandConfigResponse, ConnectionConfig, ConnectionState, DataStats,
    DetectedModem, DeviceInfo, DiscoveryInfo, ExtendedSignalInfo, GpsInfo, IpType,
    MbnActionResult, MbnAutoSelectRequest, MbnProfile, MbnSelectRequest, ModemEvent,
    ModemHardware, ModemHealth, ModemHealthState, ModemStatus, NetworkModeOption,
    RegistrationState, SignalHistory, SignalInfo, SimStatus,
};
use crate::api::auth_middleware::{require_admin, SessionUser};
use crate::security::audit::AuditEventType;
use crate::security::users::Role;
use crate::security::{
    get_merged_whitelist, save_overrides, validate_command_with_context,
    CommandSafety, MergedWhitelist, WhitelistOverrides,
};
use crate::state::AppState;
use axum::Extension;

/// Timeout durations for different operation classes.
const QUICK_TIMEOUT: Duration = Duration::from_secs(5);
const STATE_CHANGE_TIMEOUT: Duration = Duration::from_secs(15);

/// Reject operator-supplied APN/auth strings that could break out of the quoted
/// AT argument or smuggle a second AT command onto the serial wire.
///
/// `apn`/`username`/`password` are interpolated into quoted positional fields of
/// an `AT+QICSGP=...,"<apn>","<user>","<password>",...` (or `AT+CGDCONT`) write.
/// An embedded double-quote `"` closes the quoted field; an embedded CR/LF (or
/// any ASCII control character) terminates the AT line and lets a crafted value
/// inject a second command. We reject both at the API input boundary so the
/// caller gets a clean 400 instead of a silent hardware-layer error. (The
/// hardware layer adds the authoritative serial-write control-char backstop
/// separately; this is the friendly front-door check.)
///
/// `field` is used only to build the error message.
fn validate_at_arg(value: &str, field: &str) -> Result<(), ApiError> {
    if value.contains('"') {
        return Err(ApiError::bad_request(format!(
            "{field} must not contain a double-quote (\") character"
        )));
    }
    if value.chars().any(|c| c.is_ascii_control()) {
        return Err(ApiError::bad_request(format!(
            "{field} must not contain control characters (including CR/LF)"
        )));
    }
    Ok(())
}

/// Validate the AT-bound fields of a [`ConnectionConfig`] (apn + optional
/// username/password) at an API write boundary. These values are later written
/// into a quoted `AT+QICSGP`/`AT+CGDCONT` argument, so a `"` or control char
/// must be rejected before the value is stored or sent.
fn validate_connection_at_args(conn: &ConnectionConfig) -> Result<(), ApiError> {
    validate_at_arg(&conn.apn, "APN")?;
    if let Some(ref u) = conn.username {
        validate_at_arg(u, "Username")?;
    }
    if let Some(ref p) = conn.password {
        validate_at_arg(p, "Password")?;
    }
    Ok(())
}

/// Reduced, unauthenticated status shape for the public login-screen route.
///
/// Deliberately omits `ip_address` (H1): the public `/modem/:id/status` route
/// is reachable without auth, so an unauthenticated caller who guesses a
/// modem_id must not be able to read the modem's IP. The login UI only needs
/// operator/technology/connected/signal.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PublicModemStatus {
    pub connected: bool,
    pub technology: Option<crate::hardware::Technology>,
    pub operator: Option<String>,
    pub signal_strength: i32,
}

/// Reduced, unauthenticated signal shape for the public login-screen route.
///
/// Deliberately omits `cell_id` (2026-06-19): the public `/modem/:id/signal`
/// route is reachable without auth, so an unauthenticated caller must not be
/// able to read the serving-cell identifier — a coarsely-geolocatable value.
/// The authenticated routes (`/modem/signal` compat,
/// `/modem/:id/signal/extended`, and the WebSocket `signal` payload) still
/// return the full [`SignalInfo`] including `cell_id`. Mirrors the
/// [`PublicModemStatus`] (H1) treatment.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PublicSignalInfo {
    pub rssi: f64,
    pub rsrp: f64,
    pub rsrq: f64,
    pub sinr: f64,
    pub band: String,
    // cell_id intentionally omitted pre-auth.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub technology: Option<crate::hardware::Technology>,
}

impl From<&SignalInfo> for PublicSignalInfo {
    fn from(s: &SignalInfo) -> Self {
        Self {
            rssi: s.rssi,
            rsrp: s.rsrp,
            rsrq: s.rsrq,
            sinr: s.sinr,
            band: s.band.clone(),
            technology: s.technology,
        }
    }
}

/// Response for GET /api/modems - lists all modems with their contexts.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ModemListItem {
    pub id: String,
    pub detected: DetectedModem,
    pub health: ModemHealth,
    pub last_signal: Option<SignalInfo>,
    pub discovery: DiscoveryInfo,
}

/// Fast-fail guard for read-only modem routes.
///
/// Checks modem health BEFORE acquiring the mutex lock. When a modem
/// USB-disconnects, AT commands queue behind the mutex with 5s timeouts
/// each — creating a cascading multi-minute queue. This helper checks
/// health and returns the handler Arc, which the caller can then lock.
///
/// Returns Arc to the modem handler after verifying modem exists and is available.
pub(crate) async fn require_modem_available(
    state: &AppState,
    modem_id: &str,
) -> Result<Arc<tokio::sync::Mutex<Box<dyn ModemHardware + Send>>>, ApiError> {
    let modems = state.modems.read().await;
    let context = modems.get(modem_id).ok_or_else(|| {
        ApiError::not_found(format!("Modem not found: {modem_id}"))
    })?;

    let health = context.health.read().await;
    if !health.available {
        return Err(ApiError::service_unavailable_with_retry("Modem unavailable", 2));
    }
    drop(health);

    // Clone the Arc so caller can lock it after we drop the modems lock
    let handler = Arc::clone(&context.handler);
    drop(modems);

    Ok(handler)
}

/// Helper to get a modem context by ID.
///
/// Returns a NOT_FOUND error if the modem doesn't exist.
pub(crate) async fn get_modem_context(
    state: &AppState,
    modem_id: &str,
) -> Result<Arc<tokio::sync::Mutex<Box<dyn ModemHardware + Send>>>, ApiError> {
    let modems = state.modems.read().await;
    let context = modems.get(modem_id).ok_or_else(|| {
        ApiError::not_found(format!("Modem not found: {modem_id}"))
    })?;

    let handler = Arc::clone(&context.handler);
    drop(modems);

    Ok(handler)
}

/// GET /api/modems
///
/// List all modems on the system with their IDs, detection info, health, and cached signal.
pub async fn list_modems(State(state): State<Arc<AppState>>) -> ApiResult<Json<Vec<ModemListItem>>> {
    let modems = state.modems.read().await;
    let mut items = Vec::new();

    for (id, context) in modems.iter() {
        let health = context.health.read().await.clone();
        let last_signal = context.last_signal.read().await.clone();
        let discovery = context.discovery.read().await.clone();

        items.push(ModemListItem {
            id: id.clone(),
            detected: context.detected.clone(),
            health,
            last_signal,
            discovery,
        });
    }

    Ok(Json(items))
}

/// GET /api/modem/:modem_id/detect
///
/// Detect connected modems and available protocols.
/// Returns the list of detected modems with profile match information.
pub async fn detect(State(state): State<Arc<AppState>>) -> ApiResult<Json<Vec<DetectedModem>>> {
    let detected = state.detected_modems.read().await;
    Ok(Json(detected.clone()))
}

/// GET /api/modem/:modem_id/info
///
/// Get device identification info from the discovery cache (populated at boot).
pub async fn info(
    Path(modem_id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<DeviceInfo>> {
    let modems = state.modems.read().await;
    let context = modems.get(&modem_id).ok_or_else(|| {
        ApiError::not_found(format!("Modem not found: {modem_id}"))
    })?;
    let discovery = context.discovery.read().await;
    Ok(Json(discovery.device_info.clone()))
}

/// GET /api/modem/:modem_id/status
///
/// Get current modem status from the 60-second master cache.
pub async fn status(
    Path(modem_id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<ModemStatus>> {
    let modems = state.modems.read().await;
    let context = modems.get(&modem_id).ok_or_else(|| {
        ApiError::not_found(format!("Modem not found: {modem_id}"))
    })?;

    let cache = context.state_cache.read().await;
    match cache.as_ref() {
        Some(c) => Ok(Json(ModemStatus {
            connected: c.connection.connected,
            technology: c.connection.technology,
            operator: c.connection.operator.clone(),
            signal_strength: c.signal_strength,
            ip_address: c.connection.ip_address.clone(),
        })),
        None => Err(ApiError::service_unavailable_with_retry(
            "Cache not yet initialized, retry shortly",
            60,
        )),
    }
}

/// GET /api/modem/:modem_id/status (PUBLIC — unauthenticated login screen)
///
/// Reduced status shape that omits `ip_address` (H1). Authenticated dashboard
/// views use [`status`] / [`status_compat`] which still return the IP.
pub async fn public_status(
    Path(modem_id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<PublicModemStatus>> {
    let modems = state.modems.read().await;
    let context = modems.get(&modem_id).ok_or_else(|| {
        ApiError::not_found(format!("Modem not found: {modem_id}"))
    })?;

    let cache = context.state_cache.read().await;
    match cache.as_ref() {
        Some(c) => Ok(Json(PublicModemStatus {
            connected: c.connection.connected,
            technology: c.connection.technology,
            operator: c.connection.operator.clone(),
            signal_strength: c.signal_strength,
        })),
        None => Err(ApiError::service_unavailable_with_retry(
            "Cache not yet initialized, retry shortly",
            60,
        )),
    }
}

/// GET /api/modem/:modem_id/signal
///
/// Get detailed signal metrics from the 60-second master cache.
pub async fn signal(
    Path(modem_id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<SignalInfo>> {
    let modems = state.modems.read().await;
    let context = modems.get(&modem_id).ok_or_else(|| {
        ApiError::not_found(format!("Modem not found: {modem_id}"))
    })?;

    let cache = context.state_cache.read().await;
    match cache.as_ref() {
        Some(c) => Ok(Json(c.signal.clone())),
        None => Err(ApiError::service_unavailable_with_retry(
            "Cache not yet initialized, retry shortly",
            60,
        )),
    }
}

/// GET /api/modem/:modem_id/signal (PUBLIC — unauthenticated login screen)
///
/// Reduced signal shape that omits `cell_id` (2026-06-19). Authenticated views
/// use [`signal_compat`] / [`signal_extended`] (and the WebSocket `signal`
/// payload) which still return the full [`SignalInfo`] including `cell_id`.
pub async fn public_signal(
    Path(modem_id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<PublicSignalInfo>> {
    let modems = state.modems.read().await;
    let context = modems.get(&modem_id).ok_or_else(|| {
        ApiError::not_found(format!("Modem not found: {modem_id}"))
    })?;

    let cache = context.state_cache.read().await;
    match cache.as_ref() {
        Some(c) => Ok(Json(PublicSignalInfo::from(&c.signal))),
        None => Err(ApiError::service_unavailable_with_retry(
            "Cache not yet initialized, retry shortly",
            60,
        )),
    }
}

/// GET /api/modem/stats
///
/// Get data transfer statistics.
/// GET /api/modem/:modem_id/stats
///
/// Get data usage statistics.
pub async fn stats(
    Path(modem_id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<DataStats>> {
    let handler_arc = require_modem_available(&state, &modem_id).await?;
    let modem = timeout(Duration::from_secs(2), handler_arc.lock())
        .await
        .map_err(|_| ApiError::service_unavailable_with_retry("Modem busy", 1))?;

    let stats = timeout(QUICK_TIMEOUT, modem.get_data_stats())
        .await
        .map_err(|_| ApiError::service_unavailable_with_retry("Request timed out", 1))?
        .map_err(ApiError::from)?;

    Ok(Json(stats))
}

/// POST /api/modem/connect
///
/// Establish a data connection with the provided APN configuration.
/// POST /api/modem/:modem_id/connect
///
/// Establish data connection with the specified APN configuration.
pub async fn connect(
    Path(modem_id): Path<String>,
    State(state): State<Arc<AppState>>,
    Extension(session_user): Extension<SessionUser>,
    Json(config): Json<ConnectionConfig>,
) -> ApiResult<Json<ModemStatus>> {
    require_admin(&session_user)?;

    // Validate APN
    if config.apn.is_empty() {
        return Err(ApiError::bad_request("APN is required"));
    }
    if config.apn.len() > 100 {
        return Err(ApiError::bad_request("APN too long"));
    }
    if config.cid == 0 || config.cid > 8 {
        return Err(ApiError::bad_request("CID must be 1-8"));
    }
    // AT-injection front door: reject `"`/control chars before these reach the
    // quoted AT+CGDCONT/QICSGP write.
    validate_connection_at_args(&config)?;

    let handler_arc = get_modem_context(&state, &modem_id).await?;
    let modem = timeout(Duration::from_secs(2), handler_arc.lock())
        .await
        .map_err(|_| ApiError::service_unavailable_with_retry("Modem busy", 1))?;

    // Attempt connection with longer timeout
    timeout(STATE_CHANGE_TIMEOUT, modem.connect(&config))
        .await
        .map_err(|_| ApiError::service_unavailable_with_retry("Connection timed out", 5))?
        .map_err(ApiError::from)?;

    // Save last-used connection settings to per-modem config (memory + disk)
    {
        let modems = state.modems.read().await;
        if let Some(context) = modems.get(&modem_id) {
            let mut modem_config = context.config.write().await;
            *modem_config = config;
            // TODO: Per-modem config persistence will be implemented in config.rs update
            // For now, config is saved to memory only
        }
    }

    // Return updated status
    let status = modem.get_status().await.map_err(ApiError::from)?;
    Ok(Json(status))
}

/// POST /api/modem/:modem_id/disconnect
///
/// Terminate the data connection.
pub async fn disconnect(
    Path(modem_id): Path<String>,
    State(state): State<Arc<AppState>>,
    Extension(session_user): Extension<SessionUser>,
) -> ApiResult<Json<ModemStatus>> {
    require_admin(&session_user)?;
    let handler_arc = get_modem_context(&state, &modem_id).await?;
    let modem = timeout(Duration::from_secs(2), handler_arc.lock())
        .await
        .map_err(|_| ApiError::service_unavailable_with_retry("Modem busy", 1))?;

    timeout(STATE_CHANGE_TIMEOUT, modem.disconnect())
        .await
        .map_err(|_| ApiError::service_unavailable_with_retry("Disconnect timed out", 5))?
        .map_err(ApiError::from)?;

    let status = modem.get_status().await.map_err(ApiError::from)?;
    Ok(Json(status))
}

/// POST /api/modem/:modem_id/reconnect
///
/// Classify a post-reconnect AT probe result for F1 Layer 2 watcher enlistment.
///
/// After a `reconnect` (CFUN=0→CFUN=1) the AT port can re-enumerate
/// (`ttyUSB2↔ttyUSB3`). Layer 1 (handler self-heal in `traits.rs`) attempts to
/// re-open the dead fd on the next command and retry once. If it succeeds the
/// probe returns `Ok` and we leave health alone — a normal reconnect must
/// behave EXACTLY as before (no `Rebooting`, no 90s gate).
///
/// This helper is the backstop: it only fires when Layer 1 could NOT self-heal,
/// i.e. the probe still errors with a **fd-dead / unreachable** class error.
/// In that case we return `Some(Rebooting)` so the caller can mark the modem
/// `available:false` and enlist the existing reconnect watcher's proven
/// hot-swap recovery (`websocket.rs`).
///
/// Classification is grounded against the real `HardwareError` enum
/// (`hardware/traits.rs`):
/// - `Io` / `Timeout` / `DeviceNotFound` ⇒ fd is dead or the device is gone
///   (`Io` flattens `io::ErrorKind` — Broken pipe, NotConnected, etc.) ⇒
///   `Some(Rebooting)`.
/// - `Protocol` / `NotReady` / `CommandRejected` / `SimError` / `PermissionDenied`
///   / `Internal` / `NoModem` ⇒ the modem answered but returned a logical error
///   (fd alive) ⇒ `None`; enlisting the watcher would falsely gate the modem
///   for 90s. `NoModem` is a pre-AT guard, not a dead fd.
///
/// Pure (no HTTP/mutex/await) so it is unit-testable directly under default
/// (mock) features. Generic over the `Ok` payload so callers can pass the real
/// `Result<ModemStatus, HardwareError>` and tests can pass `Ok(())`.
fn post_reconnect_health<T>(
    probe: &Result<T, crate::hardware::HardwareError>,
) -> Option<ModemHealth> {
    use crate::hardware::HardwareError;
    match probe {
        // Layer 1 self-healed (or never needed to) — leave health untouched.
        Ok(_) => None,
        // fd-dead / device-gone class: Layer 1 could not self-heal. Enlist the
        // reconnect watcher by marking Rebooting (its 90s recovery hot-swaps
        // the handler once the port re-enumerates back).
        Err(HardwareError::Io(_))
        | Err(HardwareError::Timeout)
        | Err(HardwareError::DeviceNotFound(_)) => Some(ModemHealth {
            available: false,
            state: ModemHealthState::Rebooting,
            message: Some("Reconnect re-enumeration — watcher recovering".to_string()),
        }),
        // Modem answered with a logical error — the fd is alive. Do NOT enlist
        // the watcher (would falsely report unavailable for 90s).
        Err(_) => None,
    }
}

/// Perform a pure radio cycle (AT+CFUN=0 → AT+CFUN=1) to re-establish the data
/// bearer using the APN **already saved on the modem**.  No `AT+CGDCONT` is
/// written — this is distinct from `connect`, which writes a new PDP context.
///
/// Use when the saved APN is correct but the bearer has dropped and needs
/// to be re-established (e.g. transient network loss, watchdog recovery).
///
/// Broadcasts a `ConnectionState::Connected` event over WebSocket on success
/// so the UI gets immediate feedback without waiting for the 60 s cache cycle.
pub async fn reconnect(
    Path(modem_id): Path<String>,
    State(state): State<Arc<AppState>>,
    Extension(session_user): Extension<SessionUser>,
) -> ApiResult<Json<ModemStatus>> {
    require_admin(&session_user)?;
    let handler_arc = get_modem_context(&state, &modem_id).await?;
    let modem = timeout(Duration::from_secs(2), handler_arc.lock())
        .await
        .map_err(|_| ApiError::service_unavailable_with_retry("Modem busy", 1))?;

    // The CFUN cycle can re-enumerate the AT port (ttyUSB2↔ttyUSB3). Both the
    // reconnect command and the follow-up status probe go through Layer 1's
    // handler self-heal (fail→reopen→retry-once). If Layer 1 recovers, both
    // succeed and we behave exactly as before. If it could not (port not yet
    // back), the result still carries a fd-dead / unreachable error — the
    // primary trigger point is the get_status() probe per the F1 plan, but we
    // also treat a fd-dead reconnect error the same way (DRY: one classifier).
    let reconnect_result = timeout(STATE_CHANGE_TIMEOUT, modem.reconnect())
        .await
        .map_err(|_| ApiError::service_unavailable_with_retry("Reconnect timed out", 5))?;

    // Run the reconnect result through the F1 backstop. On a fd-dead error,
    // enlist the watcher; otherwise propagate any logical error as today.
    if let Err(e) = &reconnect_result {
        if let Some(rebooting) = post_reconnect_health(&reconnect_result) {
            drop(modem);
            return Err(enlist_reconnect_watcher(&state, &modem_id, rebooting).await);
        }
        return Err(ApiError::from(e.clone()));
    }

    // Reconnect succeeded — probe status. This is the primary F1 trigger point.
    let probe = modem.get_status().await;
    drop(modem);

    let status = match probe {
        Ok(status) => status,
        Err(e) => {
            // fd-dead / unreachable after a successful CFUN cycle ⇒ Layer 1
            // couldn't self-heal the re-enumerated port. Enlist the watcher
            // (mark Rebooting, broadcast) and answer 503 (honest vs the old
            // 500). A modem-answered logical error keeps the old behavior.
            return Err(match post_reconnect_health::<ModemStatus>(&Err(e.clone())) {
                Some(rebooting) => enlist_reconnect_watcher(&state, &modem_id, rebooting).await,
                None => ApiError::from(e),
            });
        }
    };

    // Broadcast immediate UI feedback — the 60 s master-cache cycle will follow.
    // A successful reconnect always ends in the Connected state.
    state.broadcast_modem_event(
        &modem_id,
        ModemEvent::ConnectionState {
            state: ConnectionState::Connected,
            network: status.operator.clone(),
            ip: status.ip_address.clone(),
        },
    );

    Ok(Json(status))
}

/// Write a `Rebooting` health into the per-modem context, broadcast a
/// `ModemHealth` event, and return a 503 so the client can retry. Mirrors the
/// reboot handler's health-write + broadcast pattern. Used by F1 Layer 2 when a
/// post-reconnect AT probe shows the port re-enumerated and Layer 1 could not
/// self-heal — this hands recovery to the existing reconnect watcher.
async fn enlist_reconnect_watcher(
    state: &AppState,
    modem_id: &str,
    rebooting: ModemHealth,
) -> ApiError {
    {
        let modems = state.modems.read().await;
        if let Some(context) = modems.get(modem_id) {
            let mut health = context.health.write().await;
            *health = rebooting.clone();
        }
    }
    state.broadcast_event(ModemEvent::ModemHealth(rebooting));
    ApiError::service_unavailable_with_retry(
        "Modem re-enumerating after reconnect — recovering automatically",
        5,
    )
}

/// POST /api/modem/reconnect (backward-compat)
pub async fn reconnect_compat(
    State(state): State<Arc<AppState>>,
    Extension(session_user): Extension<SessionUser>,
) -> ApiResult<Json<ModemStatus>> {
    let modem_id = state
        .get_selected_or_first_modem()
        .await
        .ok_or_else(|| ApiError::not_found("No modems available"))?;
    reconnect(Path(modem_id), State(state), Extension(session_user)).await
}

/// POST /api/modem/:modem_id/command
///
/// Execute an AT command. Commands are validated against the whitelist:
/// - Safe commands execute immediately
/// - Commands requiring confirmation need `confirmed: true`
/// - Blocked commands are rejected
pub async fn command(
    Path(modem_id): Path<String>,
    State(state): State<Arc<AppState>>,
    Extension(session_user): Extension<SessionUser>,
    Json(req): Json<AtCommandRequest>,
) -> ApiResult<Json<AtCommandResponse>> {
    // Raw AT execution is an Admin-only capability (root-capable on hardware).
    require_admin(&session_user)?;

    // Get modem profile for whitelist validation
    let profile_whitelist = {
        let modems = state.modems.read().await;
        let context = modems.get(&modem_id).ok_or_else(|| {
            ApiError::not_found(format!("Modem not found: {modem_id}"))
        })?;
        context.profile.at_whitelist_additions.clone()
    };

    let overrides = state.at_whitelist_overrides.read().await.clone();
    let validation = validate_command_with_context(&req.command, &profile_whitelist, &overrides);

    match validation.safety {
        CommandSafety::Blocked => {
            return Err(ApiError::forbidden(format!(
                "Command blocked: {}",
                validation.reason.unwrap_or_default()
            )));
        }
        CommandSafety::RequiresConfirmation if !req.confirmed => {
            return Err(ApiError::precondition_required(
                validation.reason.unwrap_or_else(|| "Command requires confirmation".to_string()),
            ));
        }
        _ => {}
    }

    let handler_arc = get_modem_context(&state, &modem_id).await?;
    let modem = timeout(Duration::from_secs(2), handler_arc.lock())
        .await
        .map_err(|_| ApiError::service_unavailable_with_retry("Modem busy", 1))?;

    // Execute command with quick timeout
    let response = timeout(QUICK_TIMEOUT, modem.execute_at(&req.command))
        .await
        .map_err(|_| ApiError::service_unavailable_with_retry("Command timed out", 1))?
        .map_err(ApiError::from)?;
    drop(modem);

    // Audit the raw-AT execution — arbitrary AT on a root daemon is the single
    // most sensitive control action and must leave a forensic trail (covers
    // command_compat too, which delegates here).
    state
        .audit
        .log(
            crate::security::audit::AuditEventType::AtCommand,
            None,
            format!(
                "{} executed raw AT on {}: {}",
                session_user.username, modem_id, req.command
            ),
        )
        .await;

    Ok(Json(AtCommandResponse {
        command: req.command,
        response,
        success: true,
    }))
}

/// GET /api/modem/:modem_id/gps
///
/// Get current GPS position. Returns GPS info or error if not supported.
pub async fn gps(
    Path(modem_id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<GpsInfo>> {
    let handler_arc = require_modem_available(&state, &modem_id).await?;
    let modem = timeout(Duration::from_secs(2), handler_arc.lock())
        .await
        .map_err(|_| ApiError::service_unavailable_with_retry("Modem busy", 1))?;

    let gps = timeout(QUICK_TIMEOUT, modem.get_gps_position())
        .await
        .map_err(|_| ApiError::service_unavailable_with_retry("GPS request timed out", 1))?
        .map_err(ApiError::from)?;

    Ok(Json(gps))
}

/// POST /api/modem/:modem_id/gps/stop
///
/// Stop the GPS engine (AT+QGPSEND).
pub async fn gps_stop(
    Path(modem_id): Path<String>,
    State(state): State<Arc<AppState>>,
    Extension(session_user): Extension<SessionUser>,
) -> ApiResult<Json<serde_json::Value>> {
    require_admin(&session_user)?;
    let handler_arc = get_modem_context(&state, &modem_id).await?;
    let modem = timeout(Duration::from_secs(2), handler_arc.lock())
        .await
        .map_err(|_| ApiError::service_unavailable_with_retry("Modem busy", 1))?;

    timeout(QUICK_TIMEOUT, modem.stop_gps())
        .await
        .map_err(|_| ApiError::service_unavailable_with_retry("GPS stop timed out", 1))?
        .map_err(ApiError::from)?;

    Ok(Json(serde_json::json!({ "success": true })))
}

/// GET /api/modem/:modem_id/pdp
///
/// Get PDP context details, MBN carrier profiles, MBN management state,
/// live current APN config (auth/username/has_password via QICSGP on Quectel;
/// CGDCONT fallback on Telit/generic), and per-context active flags (CGACT?).
pub async fn pdp_details(
    Path(modem_id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<serde_json::Value>> {
    // Read profile configs before acquiring the modem lock.
    let (mbn_cfg, apn_live_cfg) = {
        let modems = state.modems.read().await;
        let context = modems.get(&modem_id).ok_or_else(|| {
            ApiError::not_found(format!("Modem not found: {modem_id}"))
        })?;
        (
            context.profile.mbn_config.clone(),
            context.profile.apn_live_config.clone(),
        )
    };

    let handler_arc = require_modem_available(&state, &modem_id).await?;
    let modem = timeout(Duration::from_secs(2), handler_arc.lock())
        .await
        .map_err(|_| ApiError::service_unavailable_with_retry("Modem busy", 1))?;

    // Query PDP contexts
    let cgdcont = timeout(QUICK_TIMEOUT, modem.execute_at("AT+CGDCONT?"))
        .await
        .map_err(|_| ApiError::service_unavailable_with_retry("Request timed out", 1))?
        .unwrap_or_else(|_| String::new());

    // Query per-context active flags (AT+CGACT?)
    let cgact_raw = timeout(QUICK_TIMEOUT, modem.execute_at("AT+CGACT?"))
        .await
        .ok()
        .and_then(|r| r.ok())
        .unwrap_or_default();

    // Query MBN data using profile command templates
    let mut mbn_list_raw = String::new();
    let mut mbn_auto_select: Option<bool> = None;
    let mut mbn_selected: Option<String> = None;

    if mbn_cfg.supported {
        if let Some(ref cmd) = mbn_cfg.commands.list_profiles {
            mbn_list_raw = timeout(QUICK_TIMEOUT, modem.execute_at(cmd))
                .await
                .ok()
                .and_then(|r| r.ok())
                .unwrap_or_default();
        }
        if let Some(ref cmd) = mbn_cfg.commands.query_auto_select {
            if let Ok(Ok(resp)) = timeout(QUICK_TIMEOUT, modem.execute_at(cmd)).await {
                mbn_auto_select = parse_mbn_auto_select(&resp);
            }
        }
        if let Some(ref cmd) = mbn_cfg.commands.query_selected {
            if let Ok(Ok(resp)) = timeout(QUICK_TIMEOUT, modem.execute_at(cmd)).await {
                mbn_selected = parse_mbn_selected(&resp);
            }
        }
    }

    // Parse CGDCONT contexts first (needed to compute default CID before QICSGP query)
    let pdp_contexts_raw: Vec<(u32, String, String)> = cgdcont
        .lines()
        .filter(|l| l.trim().starts_with("+CGDCONT:"))
        .filter_map(|line| {
            let after = line.trim().strip_prefix("+CGDCONT:")?.trim();
            let parts: Vec<&str> = after.splitn(4, ',').collect();
            let cid: u32 = parts.first()?.trim().parse().ok()?;
            let pdp_type = parts.get(1).map(|s| s.trim().trim_matches('"')).unwrap_or("").to_string();
            let apn = parts.get(2).map(|s| s.trim().trim_matches('"')).unwrap_or("").to_string();
            Some((cid, pdp_type, apn))
        })
        .collect();

    // Build the temporary JSON contexts slice (used by default_editing_cid heuristic).
    let ctx_for_heuristic: Vec<serde_json::Value> = pdp_contexts_raw
        .iter()
        .map(|(cid, pdp_type, apn)| serde_json::json!({
            "cid": cid.to_string(),
            "pdp_type": pdp_type,
            "apn": apn,
        }))
        .collect();

    // Determine the default editing CID.
    let editing_cid = default_editing_cid(&ctx_for_heuristic);

    // Query live current config via QICSGP (Quectel only; graceful blank for others).
    let qicsgp_raw: Option<String> = if let (Some(query_tpl), Some(cid)) =
        (apn_live_cfg.query.as_deref(), editing_cid)
    {
        let cmd = query_tpl.replace("{cid}", &cid.to_string());
        timeout(QUICK_TIMEOUT, modem.execute_at(&cmd))
            .await
            .ok()
            .and_then(|r| r.ok())
    } else {
        None
    };

    drop(modem);

    // Parse CGACT active flags
    let active_cids = parse_cgact_response(&cgact_raw);

    // Build structured pdp_contexts with active flag
    let pdp_contexts: Vec<serde_json::Value> = pdp_contexts_raw
        .iter()
        .map(|(cid, pdp_type, apn)| {
            let active = active_cids.contains(cid);
            serde_json::json!({
                "cid": cid.to_string(),
                "pdp_type": pdp_type,
                "apn": apn,
                "active": active,
            })
        })
        .collect();

    // Build current_config.
    //
    // ip_type: prefer CGDCONT pdp_type (available for all modems); QICSGP's
    // context_type is used only when CGDCONT entry for the editing CID is absent.
    let editing_ctx = editing_cid.and_then(|cid| {
        pdp_contexts_raw.iter().find(|(c, _, _)| *c == cid)
    });
    let ip_type_from_cgdcont = editing_ctx
        .map(|(_, pdp_type, _)| ip_type_from_pdp_type(pdp_type))
        .unwrap_or("ipv4");

    let current_config = if let Some(cid) = editing_cid {
        if let Some(ref raw) = qicsgp_raw {
            // Quectel path: QICSGP provides auth/username/has_password
            if let Some(parsed) = parse_qicsgp_response(raw) {
                serde_json::json!({
                    "cid": cid,
                    "apn": parsed.apn,
                    "ip_type": ip_type_from_cgdcont,
                    "auth_type": parsed.auth_type,
                    "username": parsed.username,
                    "has_password": parsed.has_password,
                })
            } else {
                // QICSGP response present but unparseable — fall back to CGDCONT
                let apn = editing_ctx.map(|(_, _, a)| a.as_str()).unwrap_or("");
                serde_json::json!({
                    "cid": cid,
                    "apn": apn,
                    "ip_type": ip_type_from_cgdcont,
                    "auth_type": "none",
                    "username": "",
                    "has_password": false,
                })
            }
        } else {
            // Non-Quectel path (Telit / generic): CGDCONT provides APN + ip_type only
            let apn = editing_ctx.map(|(_, _, a)| a.as_str()).unwrap_or("");
            serde_json::json!({
                "cid": cid,
                "apn": apn,
                "ip_type": ip_type_from_cgdcont,
                "auth_type": "none",
                "username": "",
                "has_password": false,
            })
        }
    } else {
        // No non-reserved context found — return empty sentinel
        serde_json::json!({
            "cid": serde_json::Value::Null,
            "apn": "",
            "ip_type": "ipv4",
            "auth_type": "none",
            "username": "",
            "has_password": false,
        })
    };

    // Raw MBN text (backward compat)
    let mbn_lines: String = mbn_list_raw
        .lines()
        .filter(|l| l.trim().starts_with("+QMBNCFG:"))
        .collect::<Vec<&str>>()
        .join("\n");

    // Structured MBN profiles
    let mbn_profiles = parse_mbn_list(&mbn_list_raw);

    Ok(Json(serde_json::json!({
        "pdp_contexts": pdp_contexts,
        "mbn_config": mbn_lines,
        "mbn_profiles": mbn_profiles,
        "mbn_auto_select": mbn_auto_select,
        "mbn_selected_profile": mbn_selected,
        "mbn_supported": mbn_cfg.supported,
        "current_config": current_config,
    })))
}

/// GET /api/modem/:modem_id/signal/extended
///
/// Get extended signal info: carrier aggregation, network detail, neighbour cells.
pub async fn signal_extended(
    Path(modem_id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<ExtendedSignalInfo>> {
    let handler_arc = require_modem_available(&state, &modem_id).await?;
    let modem = timeout(Duration::from_secs(2), handler_arc.lock())
        .await
        .map_err(|_| ApiError::service_unavailable_with_retry("Modem busy", 1))?;

    let info = timeout(QUICK_TIMEOUT, modem.get_extended_signal())
        .await
        .map_err(|_| ApiError::service_unavailable_with_retry("Request timed out", 1))?
        .map_err(ApiError::from)?;

    Ok(Json(info))
}

/// GET /api/modem/:modem_id/signal/antenna
///
/// Get per-antenna-port signal metrics (RSRP, RSRQ, SINR per RX port).
pub async fn antenna_metrics(
    Path(modem_id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<AntennaMetrics>> {
    let handler_arc = require_modem_available(&state, &modem_id).await?;
    let modem = timeout(Duration::from_secs(2), handler_arc.lock())
        .await
        .map_err(|_| ApiError::service_unavailable_with_retry("Modem busy", 1))?;

    let metrics = timeout(QUICK_TIMEOUT, modem.get_antenna_metrics())
        .await
        .map_err(|_| ApiError::service_unavailable_with_retry("Request timed out", 1))?
        .map_err(ApiError::from)?;

    Ok(Json(metrics))
}

/// GET /api/modem/signal/antenna (backward-compat)
pub async fn antenna_metrics_compat(State(state): State<Arc<AppState>>) -> ApiResult<Json<AntennaMetrics>> {
    let modem_id = state
        .get_selected_or_first_modem()
        .await
        .ok_or_else(|| ApiError::not_found("No modems available"))?;
    antenna_metrics(Path(modem_id), State(state)).await
}

/// GET /api/modem/signal/extended (backward-compat)
pub async fn signal_extended_compat(State(state): State<Arc<AppState>>) -> ApiResult<Json<ExtendedSignalInfo>> {
    let modem_id = state
        .get_selected_or_first_modem()
        .await
        .ok_or_else(|| ApiError::not_found("No modems available"))?;
    signal_extended(Path(modem_id), State(state)).await
}

// ============================================================================
// Modem Power Control Endpoints
// ============================================================================

/// GET /api/modem/health
///
/// Get current modem health/availability state.
/// GET /api/modem/:modem_id/health
///
/// Get the health state of the specified modem.
pub async fn health(
    Path(modem_id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<ModemHealth>> {
    let modems = state.modems.read().await;
    let context = modems.get(&modem_id).ok_or_else(|| {
        ApiError::not_found(format!("Modem not found: {modem_id}"))
    })?;

    let health = context.health.read().await;
    Ok(Json(health.clone()))
}

/// POST /api/modem/:modem_id/power-down
///
/// Gentle reboot via AT+QPOWD=1 (graceful power down). The modem shuts off
/// cleanly then boots back up automatically. USB interfaces will disappear
/// and re-enumerate. The reconnect watcher will auto-detect recovery.
pub async fn power_down(
    Path(modem_id): Path<String>,
    State(state): State<Arc<AppState>>,
    Extension(session_user): Extension<SessionUser>,
) -> ApiResult<Json<serde_json::Value>> {
    require_admin(&session_user)?;
    let handler_arc = get_modem_context(&state, &modem_id).await?;
    let modem = timeout(Duration::from_secs(2), handler_arc.lock())
        .await
        .map_err(|_| ApiError::service_unavailable_with_retry("Modem busy", 1))?;

    // Send power down command — modem may not respond with OK before shutting off
    let result = timeout(STATE_CHANGE_TIMEOUT, modem.execute_at("AT+QPOWD=1")).await;
    drop(modem);

    // Update per-modem health state — use Rebooting since the modem boots back automatically
    let new_health = ModemHealth {
        available: false,
        state: ModemHealthState::Rebooting,
        message: Some("Modem rebooting (AT+QPOWD) — will reconnect automatically".to_string()),
    };

    {
        let modems = state.modems.read().await;
        if let Some(context) = modems.get(&modem_id) {
            let mut health = context.health.write().await;
            *health = new_health.clone();
        }
    }

    // Broadcast health event
    state.broadcast_event(ModemEvent::ModemHealth(new_health));

    match result {
        Ok(Ok(_)) => Ok(Json(serde_json::json!({ "success": true, "message": "Modem rebooting (AT+QPOWD)" }))),
        Ok(Err(_)) | Err(_) => {
            // Command may timeout because modem shuts off mid-response — that's expected
            Ok(Json(serde_json::json!({ "success": true, "message": "Modem rebooting (no response — expected)" })))
        }
    }
}

/// POST /api/modem/:modem_id/reboot
///
/// Reboot the modem (AT+CFUN=1,1). USB interfaces will disappear for ~15-30s
/// then come back. The reconnect watcher will automatically re-detect.
pub async fn reboot(
    Path(modem_id): Path<String>,
    State(state): State<Arc<AppState>>,
    Extension(session_user): Extension<SessionUser>,
) -> ApiResult<Json<serde_json::Value>> {
    require_admin(&session_user)?;
    let handler_arc = get_modem_context(&state, &modem_id).await?;
    let modem = timeout(Duration::from_secs(2), handler_arc.lock())
        .await
        .map_err(|_| ApiError::service_unavailable_with_retry("Modem busy", 1))?;

    // Audit the reboot (admin-gated, privileged control action).
    state
        .audit
        .log(
            crate::security::audit::AuditEventType::ConfigChanged,
            None,
            format!("{} rebooted modem {}", session_user.username, modem_id),
        )
        .await;

    // Send reboot command
    let result = timeout(STATE_CHANGE_TIMEOUT, modem.execute_at("AT+CFUN=1,1")).await;
    drop(modem);

    // Update per-modem health state
    let new_health = ModemHealth {
        available: false,
        state: ModemHealthState::Rebooting,
        message: Some("Modem rebooting — will reconnect automatically".to_string()),
    };

    {
        let modems = state.modems.read().await;
        if let Some(context) = modems.get(&modem_id) {
            let mut health = context.health.write().await;
            *health = new_health.clone();
        }
    }

    // Broadcast health event
    state.broadcast_event(ModemEvent::ModemHealth(new_health));

    match result {
        Ok(Ok(_)) => Ok(Json(serde_json::json!({ "success": true, "message": "Modem rebooting" }))),
        Ok(Err(_)) | Err(_) => {
            Ok(Json(serde_json::json!({ "success": true, "message": "Modem rebooting (no response — expected)" })))
        }
    }
}

/// GET /api/modem/:modem_id/airplane
///
/// Query current airplane mode (CFUN) state without changing it.
pub async fn airplane_status(
    Path(modem_id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<serde_json::Value>> {
    let handler_arc = require_modem_available(&state, &modem_id).await?;
    let modem = timeout(Duration::from_secs(2), handler_arc.lock())
        .await
        .map_err(|_| ApiError::service_unavailable_with_retry("Modem busy", 1))?;

    let cfun_response = timeout(QUICK_TIMEOUT, modem.execute_at("AT+CFUN?"))
        .await
        .map_err(|_| ApiError::service_unavailable_with_retry("Query timed out", 1))?
        .map_err(ApiError::from)?;

    let airplane_active = cfun_response
        .lines()
        .find(|l| l.contains("+CFUN:"))
        .and_then(|l| l.split(':').nth(1))
        .map(|v| v.trim().starts_with('0'))
        .unwrap_or(false);

    Ok(Json(serde_json::json!({
        "airplane_mode": airplane_active,
    })))
}

/// POST /api/modem/:modem_id/airplane
///
/// Toggle airplane mode. `{ "enabled": true }` = radio off (AT+CFUN=0),
/// `{ "enabled": false }` = radio on (AT+CFUN=1).
/// The modem stays responsive to AT commands in airplane mode.
pub async fn airplane(
    Path(modem_id): Path<String>,
    State(state): State<Arc<AppState>>,
    Extension(session_user): Extension<SessionUser>,
    Json(req): Json<AirplaneModeRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    require_admin(&session_user)?;
    let handler_arc = get_modem_context(&state, &modem_id).await?;
    let modem = timeout(Duration::from_secs(2), handler_arc.lock())
        .await
        .map_err(|_| ApiError::service_unavailable_with_retry("Modem busy", 1))?;

    let command = if req.enabled { "AT+CFUN=0" } else { "AT+CFUN=1" };

    timeout(STATE_CHANGE_TIMEOUT, modem.execute_at(command))
        .await
        .map_err(|_| ApiError::service_unavailable_with_retry("Command timed out", 1))?
        .map_err(ApiError::from)?;

    // Brief delay for radio state to settle
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Query current CFUN state to confirm
    let cfun_response = timeout(QUICK_TIMEOUT, modem.execute_at("AT+CFUN?"))
        .await
        .map_err(|_| ApiError::service_unavailable_with_retry("Query timed out", 1))?
        .unwrap_or_default();

    // Parse +CFUN: N from response
    let airplane_active = cfun_response
        .lines()
        .find(|l| l.contains("+CFUN:"))
        .and_then(|l| l.split(':').nth(1))
        .map(|v| v.trim().starts_with('0'))
        .unwrap_or(req.enabled);

    Ok(Json(serde_json::json!({
        "success": true,
        "airplane_mode": airplane_active,
    })))
}

// =============================================================================
// AT Whitelist Management
// =============================================================================

/// Check if user has Admin+ role AND the "at-whitelist" feature permission.
async fn require_whitelist_access(session_user: &SessionUser, state: &AppState) -> Result<(), ApiError> {
    if session_user.role < Role::Admin {
        return Err(ApiError::forbidden("Admin access required"));
    }

    // Check allowed_features (if the user has feature restrictions)
    if session_user.username != "root" {
        if let Some(user) = state.users.get_user(&session_user.username).await {
            if let Some(ref features) = user.allowed_features {
                if !features.contains(&"at-whitelist".to_string()) {
                    return Err(ApiError::forbidden("AT whitelist management not permitted"));
                }
            }
        }
    }

    Ok(())
}

/// GET /api/modem/:modem_id/whitelist
///
/// Returns the full merged whitelist (base + profile + custom overrides).
/// Requires Admin+ role with "at-whitelist" feature permission.
pub async fn get_whitelist(
    Path(modem_id): Path<String>,
    State(state): State<Arc<AppState>>,
    Extension(session_user): Extension<SessionUser>,
) -> ApiResult<Json<MergedWhitelist>> {
    require_whitelist_access(&session_user, &state).await?;

    // Get profile from modem context
    let (profile_name, profile_label, profile_wl) = {
        let modems = state.modems.read().await;
        let context = modems.get(&modem_id).ok_or_else(|| {
            ApiError::not_found(format!("Modem not found: {modem_id}"))
        })?;
        let profile = &context.profile;
        let name = format!("{} {}", profile.identity.manufacturer, profile.identity.model);
        let label = profile.whitelist_label.clone()
            .unwrap_or_else(|| profile.identity.model.clone());
        let wl = profile.at_whitelist_additions.clone();
        (name, label, wl)
    };

    let overrides = state.at_whitelist_overrides.read().await.clone();
    let merged = get_merged_whitelist(&profile_wl, &profile_name, &profile_label, &overrides);

    Ok(Json(merged))
}

/// PUT /api/modem/:modem_id/whitelist
///
/// Update AT whitelist overrides. Requires Admin+ with "at-whitelist" feature.
/// Returns the new merged whitelist view.
pub async fn update_whitelist(
    Path(modem_id): Path<String>,
    State(state): State<Arc<AppState>>,
    Extension(session_user): Extension<SessionUser>,
    Json(new_overrides): Json<WhitelistOverrides>,
) -> ApiResult<Json<MergedWhitelist>> {
    require_whitelist_access(&session_user, &state).await?;

    // Save to disk
    save_overrides(&new_overrides).await.map_err(ApiError::internal)?;

    // Update in-memory state
    {
        let mut wl = state.at_whitelist_overrides.write().await;
        *wl = new_overrides.clone();
    }

    // Audit log
    state
        .audit
        .log(
            AuditEventType::ConfigChanged,
            None,
            format!("{} updated AT whitelist overrides", session_user.username),
        )
        .await;

    // Return the new merged view - get profile from modem context
    let modems = state.modems.read().await;
    let context = modems.get(&modem_id)
        .ok_or_else(|| ApiError::not_found(format!("Modem not found: {modem_id}")))?;
    let profile = Arc::clone(&context.profile);
    drop(modems);

    let profile_name = format!("{} {}", profile.identity.manufacturer, profile.identity.model);
    let profile_label = profile.whitelist_label.clone()
        .unwrap_or_else(|| profile.identity.model.clone());
    let profile_wl = profile.at_whitelist_additions.clone();

    let merged = get_merged_whitelist(&profile_wl, &profile_name, &profile_label, &new_overrides);
    Ok(Json(merged))
}

// =============================================================================
// Band & Mode Configuration
// =============================================================================

/// GET /api/modem/:modem_id/bands
///
/// Returns the current band lock and mode configuration from the modem,
/// plus the profile's supported bands/modes for UI rendering.
pub async fn get_band_config(
    Path(modem_id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<BandConfigResponse>> {
    let handler_arc = require_modem_available(&state, &modem_id).await?;

    // Get profile from modem context
    let modems = state.modems.read().await;
    let context = modems.get(&modem_id)
        .ok_or_else(|| ApiError::not_found(format!("Modem not found: {modem_id}")))?;
    let profile = Arc::clone(&context.profile);
    drop(modems);

    let band_cfg = &profile.band_mode_config;

    if !band_cfg.supported {
        return Err(ApiError::bad_request(
            "Band control not supported by this modem profile",
        ));
    }

    let modem = timeout(Duration::from_secs(2), handler_arc.lock())
        .await
        .map_err(|_| ApiError::service_unavailable_with_retry("Modem busy", 1))?;

    // Query current mode preference
    let active_mode_raw = if let Some(ref cmd) = band_cfg.commands.query_mode {
        let parser = if band_cfg.band_command_variant == "telit_bnd" {
            parse_ws46_value as fn(&str) -> Option<String>
        } else {
            parse_qnwprefcfg_value
        };
        timeout(QUICK_TIMEOUT, modem.execute_at(cmd))
            .await
            .ok()
            .and_then(|r| r.ok())
            .and_then(|r| parser(&r))
    } else {
        None
    };

    // Query nr5g_disable_mode
    let nr5g_disable_mode = if let Some(ref cmd) = band_cfg.commands.query_nr5g_disable {
        timeout(QUICK_TIMEOUT, modem.execute_at(cmd))
            .await
            .ok()
            .and_then(|r| r.ok())
            .and_then(|r| parse_qnwprefcfg_value(&r))
            .and_then(|v| v.parse::<u8>().ok())
    } else {
        None
    };

    // Query bands — variant dispatch
    let (active_lte_bands, active_nsa_bands, active_sa_bands, active_nrdc_bands, nrdc_enabled);

    if band_cfg.band_command_variant == "telit_bnd" {
        // Telit: single AT#BND? query returns all band types
        if let Some(ref cmd) = band_cfg.commands.query_all_bands {
            let (lte, nsa, sa) = timeout(QUICK_TIMEOUT, modem.execute_at(cmd))
                .await
                .ok()
                .and_then(|r| r.ok())
                .map(|r| parse_telit_bnd_response(&r))
                .unwrap_or_default();
            active_lte_bands = lte;
            active_nsa_bands = nsa;
            active_sa_bands = sa;
        } else {
            active_lte_bands = vec![];
            active_nsa_bands = vec![];
            active_sa_bands = vec![];
        }
        active_nrdc_bands = vec![];
        nrdc_enabled = None;
    } else {
        // Quectel (per_type): separate AT commands per band type
        active_lte_bands = if let Some(ref cmd) = band_cfg.commands.query_lte_bands {
            timeout(QUICK_TIMEOUT, modem.execute_at(cmd))
                .await
                .ok()
                .and_then(|r| r.ok())
                .map(|r| parse_band_list(&r, &band_cfg.band_separator))
                .unwrap_or_default()
        } else {
            vec![]
        };

        active_nsa_bands = if let Some(ref cmd) = band_cfg.commands.query_nsa_bands {
            timeout(QUICK_TIMEOUT, modem.execute_at(cmd))
                .await
                .ok()
                .and_then(|r| r.ok())
                .map(|r| parse_band_list(&r, &band_cfg.band_separator))
                .unwrap_or_default()
        } else {
            vec![]
        };

        active_sa_bands = if let Some(ref cmd) = band_cfg.commands.query_sa_bands {
            timeout(QUICK_TIMEOUT, modem.execute_at(cmd))
                .await
                .ok()
                .and_then(|r| r.ok())
                .map(|r| parse_band_list(&r, &band_cfg.band_separator))
                .unwrap_or_default()
        } else {
            vec![]
        };

        active_nrdc_bands = if let Some(ref cmd) = band_cfg.commands.query_nrdc_bands {
            timeout(QUICK_TIMEOUT, modem.execute_at(cmd))
                .await
                .ok()
                .and_then(|r| r.ok())
                .map(|r| parse_band_list(&r, &band_cfg.band_separator))
                .unwrap_or_default()
        } else {
            vec![]
        };

        nrdc_enabled = if let Some(ref cmd) = band_cfg.commands.query_nrdc_mode {
            timeout(QUICK_TIMEOUT, modem.execute_at(cmd))
                .await
                .ok()
                .and_then(|r| r.ok())
                .and_then(|r| parse_qnwprefcfg_value(&r))
                .map(|v| v != "0")
        } else {
            None
        };
    }

    // Resolve which mode definition matches current state
    let active_mode_id = resolve_active_mode(
        &band_cfg.modes,
        active_mode_raw.as_deref(),
        nr5g_disable_mode,
    )
    .map(|m| m.id.clone());

    // Merge active bands into supported lists so the UI always shows every
    // band the modem knows about (handles profile mismatches / firmware updates).
    let supported_lte = merge_band_lists(&band_cfg.lte_bands, &active_lte_bands);
    let supported_nsa = merge_band_lists(&band_cfg.nsa_nr5g_bands, &active_nsa_bands);
    let supported_sa = merge_band_lists(&band_cfg.sa_nr5g_bands, &active_sa_bands);
    let supported_nrdc = merge_band_lists(&band_cfg.nrdc_nr5g_bands, &active_nrdc_bands);

    Ok(Json(BandConfigResponse {
        supported_modes: band_cfg.modes.clone(),
        supported_lte_bands: supported_lte,
        supported_nsa_bands: supported_nsa,
        supported_sa_bands: supported_sa,
        supported_nrdc_bands: supported_nrdc,
        has_nrdc: band_cfg.commands.query_nrdc_bands.is_some(),
        reboot_on_band_change: band_cfg.reboot_on_band_change,
        has_restore: band_cfg.commands.restore_bands.is_some(),
        active_mode_id,
        active_mode_raw,
        nr5g_disable_mode,
        active_lte_bands,
        active_nsa_bands,
        active_sa_bands,
        active_nrdc_bands,
        nrdc_enabled,
    }))
}

/// POST /api/modem/:modem_id/bands
///
/// Apply band lock and mode changes. Sends AT commands to the modem
/// in the correct order: mode → nr5g_disable → LTE bands → NSA → SA → NRDC.
pub async fn set_band_config(
    Path(modem_id): Path<String>,
    State(state): State<Arc<AppState>>,
    Extension(session_user): Extension<SessionUser>,
    Json(req): Json<BandConfigRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    require_admin(&session_user)?;
    let handler_arc = require_modem_available(&state, &modem_id).await?;

    // Get profile from modem context
    let modems = state.modems.read().await;
    let context = modems.get(&modem_id)
        .ok_or_else(|| ApiError::not_found(format!("Modem not found: {modem_id}")))?;
    let profile = Arc::clone(&context.profile);
    drop(modems);

    let band_cfg = &profile.band_mode_config;

    if !band_cfg.supported {
        return Err(ApiError::bad_request("Band control not supported"));
    }

    // Find the requested mode definition
    let mode_def = band_cfg
        .modes
        .iter()
        .find(|m| m.id == req.mode_id)
        .ok_or_else(|| ApiError::bad_request(format!("Unknown mode: {}", req.mode_id)))?
        .clone();

    let sep = band_cfg.band_separator.clone();
    let cmds = band_cfg.commands.clone();
    let reboot_required = band_cfg.reboot_on_band_change;

    let modem = timeout(Duration::from_secs(2), handler_arc.lock())
        .await
        .map_err(|_| ApiError::service_unavailable_with_retry("Modem busy", 1))?;

    // 1. Set mode preference
    if let Some(ref template) = cmds.set_mode {
        let cmd = template.replace("{value}", &mode_def.mode_value);
        let response = timeout(STATE_CHANGE_TIMEOUT, modem.execute_at(&cmd))
            .await
            .map_err(|_| ApiError::service_unavailable_with_retry("Mode set timed out", 5))?
            .map_err(ApiError::from)?;
        if response.contains("ERROR") {
            return Err(ApiError::internal(format!(
                "Failed to set mode: {}",
                response.trim()
            )));
        }
    }

    // 2. Set nr5g_disable_mode (if the mode specifies it)
    if let (Some(ref template), Some(val)) =
        (&cmds.set_nr5g_disable, mode_def.nr5g_disable_value)
    {
        let cmd = template.replace("{value}", &val.to_string());
        let response = timeout(STATE_CHANGE_TIMEOUT, modem.execute_at(&cmd))
            .await
            .map_err(|_| ApiError::service_unavailable_with_retry("NR5G disable timed out", 5))?
            .map_err(ApiError::from)?;
        if response.contains("ERROR") {
            return Err(ApiError::internal(format!(
                "Failed to set nr5g_disable_mode: {}",
                response.trim()
            )));
        }
    }

    // 3. Set bands — variant dispatch
    if band_cfg.band_command_variant == "telit_bnd" {
        // Telit: single AT#BND= command with all band types as hex bitmasks
        if let Some(ref template) = cmds.set_all_bands {
            let lte = if mode_def.active_sections.lte && !req.lte_bands.is_empty() {
                &req.lte_bands[..]
            } else {
                &band_cfg.lte_bands[..]
            };
            let nsa = if mode_def.active_sections.nsa && !req.nsa_bands.is_empty() {
                &req.nsa_bands[..]
            } else {
                &band_cfg.nsa_nr5g_bands[..]
            };
            let sa = if mode_def.active_sections.sa && !req.sa_bands.is_empty() {
                &req.sa_bands[..]
            } else {
                &band_cfg.sa_nr5g_bands[..]
            };
            let cmd = build_telit_bnd_command(template, lte, nsa, sa);
            let response = timeout(STATE_CHANGE_TIMEOUT, modem.execute_at(&cmd))
                .await
                .map_err(|_| {
                    ApiError::service_unavailable_with_retry("Band set timed out", 5)
                })?
                .map_err(ApiError::from)?;
            if response.contains("ERROR") {
                return Err(ApiError::internal(format!(
                    "Failed to set bands: {}",
                    response.trim()
                )));
            }
        }
    } else {
        // Quectel (per_type): separate AT commands per band type

        // 3a. Set LTE bands (if section is active and bands provided)
        if mode_def.active_sections.lte && !req.lte_bands.is_empty() {
            if let Some(ref template) = cmds.set_lte_bands {
                let band_str = format_band_list(&req.lte_bands, &sep);
                let cmd = template.replace("{value}", &band_str);
                let response = timeout(STATE_CHANGE_TIMEOUT, modem.execute_at(&cmd))
                    .await
                    .map_err(|_| {
                        ApiError::service_unavailable_with_retry("LTE band set timed out", 5)
                    })?
                    .map_err(ApiError::from)?;
                if response.contains("ERROR") {
                    return Err(ApiError::internal(format!(
                        "Failed to set LTE bands: {}",
                        response.trim()
                    )));
                }
            }
        }

        // 3b. Set NSA NR5G bands (if section is active and bands provided)
        if mode_def.active_sections.nsa && !req.nsa_bands.is_empty() {
            if let Some(ref template) = cmds.set_nsa_bands {
                let band_str = format_band_list(&req.nsa_bands, &sep);
                let cmd = template.replace("{value}", &band_str);
                let response = timeout(STATE_CHANGE_TIMEOUT, modem.execute_at(&cmd))
                    .await
                    .map_err(|_| {
                        ApiError::service_unavailable_with_retry("NSA band set timed out", 5)
                    })?
                    .map_err(ApiError::from)?;
                if response.contains("ERROR") {
                    return Err(ApiError::internal(format!(
                        "Failed to set NSA bands: {}",
                        response.trim()
                    )));
                }
            }
        }

        // 3c. Set SA NR5G bands (if section is active and bands provided)
        if mode_def.active_sections.sa && !req.sa_bands.is_empty() {
            if let Some(ref template) = cmds.set_sa_bands {
                let band_str = format_band_list(&req.sa_bands, &sep);
                let cmd = template.replace("{value}", &band_str);
                let response = timeout(STATE_CHANGE_TIMEOUT, modem.execute_at(&cmd))
                    .await
                    .map_err(|_| {
                        ApiError::service_unavailable_with_retry("SA band set timed out", 5)
                    })?
                    .map_err(ApiError::from)?;
                if response.contains("ERROR") {
                    return Err(ApiError::internal(format!(
                        "Failed to set SA bands: {}",
                        response.trim()
                    )));
                }
            }
        }

        // 3d. Set NRDC bands + mode (if provided)
        if let Some(ref nrdc_bands) = req.nrdc_bands {
            if !nrdc_bands.is_empty() {
                if let Some(ref template) = cmds.set_nrdc_bands {
                    let band_str = format_band_list(nrdc_bands, &sep);
                    let cmd = template.replace("{value}", &band_str);
                    let _ = timeout(STATE_CHANGE_TIMEOUT, modem.execute_at(&cmd))
                        .await
                        .ok();
                }
            }
        }
        if let Some(nrdc_en) = req.nrdc_enabled {
            if let Some(ref template) = cmds.set_nrdc_mode {
                let cmd = template.replace("{value}", if nrdc_en { "1" } else { "0" });
                let _ = timeout(STATE_CHANGE_TIMEOUT, modem.execute_at(&cmd))
                    .await
                    .ok();
            }
        }
    }

    drop(modem);

    // Audit log
    state
        .audit
        .log(
            AuditEventType::ConfigChanged,
            None,
            format!(
                "{} changed band/mode config: mode={}",
                session_user.username, req.mode_id
            ),
        )
        .await;

    // reboot_required was captured from profile at the beginning

    Ok(Json(serde_json::json!({
        "success": true,
        "reboot_required": reboot_required,
        "message": if reboot_required {
            "Band configuration saved. Modem will reboot to apply changes."
        } else {
            "Band configuration applied successfully."
        }
    })))
}

/// POST /api/modem/:modem_id/bands/restore
///
/// Restore all bands to factory default using the profile's restore command.
pub async fn restore_bands(
    Path(modem_id): Path<String>,
    State(state): State<Arc<AppState>>,
    Extension(session_user): Extension<SessionUser>,
) -> ApiResult<Json<serde_json::Value>> {
    require_admin(&session_user)?;
    let handler_arc = require_modem_available(&state, &modem_id).await?;

    // Get profile from modem context
    let modems = state.modems.read().await;
    let context = modems.get(&modem_id)
        .ok_or_else(|| ApiError::not_found(format!("Modem not found: {modem_id}")))?;
    let profile = Arc::clone(&context.profile);
    drop(modems);

    let band_cfg = &profile.band_mode_config;

    if !band_cfg.supported {
        return Err(ApiError::bad_request("Band control not supported"));
    }

    let restore_cmd = band_cfg
        .commands
        .restore_bands
        .clone()
        .ok_or_else(|| ApiError::bad_request("Restore command not available for this modem"))?;

    let modem = timeout(Duration::from_secs(2), handler_arc.lock())
        .await
        .map_err(|_| ApiError::service_unavailable_with_retry("Modem busy", 1))?;
    let response = timeout(STATE_CHANGE_TIMEOUT, modem.execute_at(&restore_cmd))
        .await
        .map_err(|_| ApiError::service_unavailable_with_retry("Restore timed out", 5))?
        .map_err(ApiError::from)?;

    drop(modem);

    if response.contains("ERROR") {
        return Err(ApiError::internal(format!(
            "Restore failed: {}",
            response.trim()
        )));
    }

    state
        .audit
        .log(
            AuditEventType::ConfigChanged,
            None,
            format!("{} restored all bands to default", session_user.username),
        )
        .await;

    Ok(Json(serde_json::json!({
        "success": true,
        "message": "All bands restored to factory default."
    })))
}

// =============================================================================
// Band Config Parsing Helpers
// =============================================================================

/// Parse the value portion of a +QNWPREFCFG response.
///
/// Input: `+QNWPREFCFG: "mode_pref",AUTO\r\nOK`
/// Output: Some("AUTO")
fn parse_qnwprefcfg_value(response: &str) -> Option<String> {
    response
        .lines()
        .find(|l| l.contains("+QNWPREFCFG:"))
        .and_then(|line| line.split(',').nth(1))
        .map(|v| v.trim().to_string())
}

/// Parse a separator-delimited band list from a modem response.
///
/// Input: `+QNWPREFCFG: "lte_band",1:3:7:20\r\nOK`, separator=":"
/// Output: [1, 3, 7, 20]
fn parse_band_list(response: &str, separator: &str) -> Vec<u32> {
    response
        .lines()
        .find(|l| l.contains("+QNWPREFCFG:") || l.contains("+QCFG:"))
        .and_then(|line| line.split(',').nth(1))
        .map(|bands_str| {
            bands_str
                .trim()
                .split(separator)
                .filter_map(|b| b.trim().parse::<u32>().ok())
                .collect()
        })
        .unwrap_or_default()
}

/// Format a list of band numbers into a sorted, separator-joined string.
///
/// Input: [20, 3, 1, 7], separator=":"
/// Output: "1:3:7:20"
fn format_band_list(bands: &[u32], separator: &str) -> String {
    let mut sorted: Vec<u32> = bands.to_vec();
    sorted.sort_unstable();
    sorted
        .iter()
        .map(|b| b.to_string())
        .collect::<Vec<_>>()
        .join(separator)
}

/// Merge profile band list with active bands from the modem.
/// Returns sorted union so the UI shows every band the modem knows about,
/// even if the profile list is slightly stale or firmware added new bands.
fn merge_band_lists(profile_bands: &[u32], active_bands: &[u32]) -> Vec<u32> {
    let mut merged: Vec<u32> = profile_bands.to_vec();
    for &b in active_bands {
        if !merged.contains(&b) {
            merged.push(b);
        }
    }
    merged.sort_unstable();
    merged
}

/// Match the current modem state (mode + nr5g_disable) to a mode definition.
fn resolve_active_mode<'a>(
    modes: &'a [NetworkModeOption],
    current_mode: Option<&str>,
    nr5g_disable: Option<u8>,
) -> Option<&'a NetworkModeOption> {
    modes.iter().find(|m| {
        let mode_matches = current_mode
            .map(|cm| cm == m.mode_value)
            .unwrap_or(false);
        let nr5g_matches = match (m.nr5g_disable_value, nr5g_disable) {
            (Some(expected), Some(actual)) => expected == actual,
            (None, _) => true, // mode doesn't care about nr5g_disable
            (Some(_), None) => false,
        };
        mode_matches && nr5g_matches
    })
}

// =============================================================================
// Telit AT#BND Hex Bitmask Parsing/Formatting
// =============================================================================

/// Parse the value from an AT+WS46? response.
///
/// Input: `+WS46: 37\r\nOK`
/// Output: Some("37")
fn parse_ws46_value(response: &str) -> Option<String> {
    response
        .lines()
        .find(|l| l.contains("+WS46:"))
        .and_then(|line| line.split(':').nth(1))
        .map(|v| v.trim().to_string())
}

/// Parse Telit AT#BND? response into separate band lists for LTE, NSA, and SA.
///
/// Response format: `#BND: <gsm>,<umts>,<lte_low>,<lte_high>,<nsa_low>,<nsa_high>,<sa_low>,<sa_high>`
/// Each hex field is a bitmask where bit N represents band N+1 in the low field.
/// High field offset differs by technology:
/// - LTE high: bit N = band N+49
/// - NR high: bit N = band N+65 (NR bands above 48 start at n65/n66)
///
/// Returns (lte_bands, nsa_bands, sa_bands) as sorted Vec<u32>.
fn parse_telit_bnd_response(response: &str) -> (Vec<u32>, Vec<u32>, Vec<u32>) {
    let line = match response.lines().find(|l| l.contains("#BND:")) {
        Some(l) => l,
        None => return (vec![], vec![], vec![]),
    };

    let after_colon = match line.split(':').nth(1) {
        Some(v) => v.trim(),
        None => return (vec![], vec![], vec![]),
    };

    let fields: Vec<&str> = after_colon.split(',').map(|s| s.trim()).collect();
    if fields.len() < 8 {
        return (vec![], vec![], vec![]);
    }

    // Fields: [gsm, umts, lte_low, lte_high, nsa_low, nsa_high, sa_low, sa_high]
    // LTE high field starts at band 49; NR high fields start at band 65
    let lte_bands = bitmask_pair_to_bands(fields[2], fields[3], 49);
    let nsa_bands = bitmask_pair_to_bands(fields[4], fields[5], 65);
    let sa_bands = bitmask_pair_to_bands(fields[6], fields[7], 65);

    (lte_bands, nsa_bands, sa_bands)
}

/// Convert a pair of hex bitmask strings (low + high) into a sorted list of band numbers.
///
/// Low field: bit N = band N+1 (bands 1-48 for all technologies).
/// High field: bit N = band N+`high_base` where `high_base` is:
///   - 49 for LTE (bands 49-96)
///   - 65 for NR (bands 65-112, covering n65/n66 through n112)
fn bitmask_pair_to_bands(low_hex: &str, high_hex: &str, high_base: u32) -> Vec<u32> {
    let mut bands = Vec::new();

    if let Ok(low) = u64::from_str_radix(low_hex, 16) {
        for bit in 0..48 {
            if low & (1u64 << bit) != 0 {
                bands.push(bit + 1);
            }
        }
    }

    if let Ok(high) = u64::from_str_radix(high_hex, 16) {
        for bit in 0..48 {
            if high & (1u64 << bit) != 0 {
                bands.push(bit + high_base);
            }
        }
    }

    bands.sort_unstable();
    bands
}

/// Convert a list of band numbers into a hex bitmask pair (low, high) for AT#BND.
///
/// Returns (low_hex, high_hex) strings.
/// Bands 1-48 go into the low field. Bands >= `high_base` go into the high field.
fn bands_to_bitmask_pair(bands: &[u32], high_base: u32) -> (String, String) {
    let mut low: u64 = 0;
    let mut high: u64 = 0;

    for &band in bands {
        if (1..=48).contains(&band) {
            low |= 1u64 << (band - 1);
        } else if band >= high_base && band < high_base + 48 {
            high |= 1u64 << (band - high_base);
        }
    }

    (format!("{low:X}"), format!("{high:X}"))
}

/// Build a complete AT#BND= command from LTE, NSA, and SA band lists.
///
/// Uses the set_all_bands template from the profile, substituting hex bitmask pairs.
/// LTE uses high_base=49, NR (NSA/SA) uses high_base=65.
fn build_telit_bnd_command(
    template: &str,
    lte_bands: &[u32],
    nsa_bands: &[u32],
    sa_bands: &[u32],
) -> String {
    let (lte_low, lte_high) = bands_to_bitmask_pair(lte_bands, 49);
    let (nsa_low, nsa_high) = bands_to_bitmask_pair(nsa_bands, 65);
    let (sa_low, sa_high) = bands_to_bitmask_pair(sa_bands, 65);

    template
        .replace("{lte_low}", &lte_low)
        .replace("{lte_high}", &lte_high)
        .replace("{nsa_low}", &nsa_low)
        .replace("{nsa_high}", &nsa_high)
        .replace("{sa_low}", &sa_low)
        .replace("{sa_high}", &sa_high)
}

// =============================================================================
// MBN Carrier Profile Management
// =============================================================================

/// POST /api/modem/:modem_id/mbn/select
///
/// Select an MBN carrier profile by name.
pub async fn mbn_select(
    Path(modem_id): Path<String>,
    State(state): State<Arc<AppState>>,
    Extension(session_user): Extension<SessionUser>,
    Json(req): Json<MbnSelectRequest>,
) -> ApiResult<Json<MbnActionResult>> {
    require_admin(&session_user)?;
    let handler_arc = require_modem_available(&state, &modem_id).await?;

    // Get profile from modem context
    let modems = state.modems.read().await;
    let context = modems.get(&modem_id)
        .ok_or_else(|| ApiError::not_found(format!("Modem not found: {modem_id}")))?;
    let profile = Arc::clone(&context.profile);
    drop(modems);

    let mbn_cfg = &profile.mbn_config;

    if !mbn_cfg.supported {
        return Err(ApiError::bad_request("MBN management not supported"));
    }

    let select_cmd = mbn_cfg
        .commands
        .select_profile
        .clone()
        .ok_or_else(|| ApiError::bad_request("MBN select not available for this modem"))?;
    let reboot_recommended = mbn_cfg.reboot_recommended;

    let cmd = select_cmd.replace("{value}", &req.profile_name);
    crate::state::debug_trace_with_source(format!("[MBN] Selecting profile \"{}\"", req.profile_name), "system");

    let modem = timeout(Duration::from_secs(2), handler_arc.lock())
        .await
        .map_err(|_| ApiError::service_unavailable_with_retry("Modem busy", 1))?;
    let response = timeout(STATE_CHANGE_TIMEOUT, modem.execute_at(&cmd))
        .await
        .map_err(|_| ApiError::service_unavailable_with_retry("MBN select timed out", 5))?
        .map_err(ApiError::from)?;
    drop(modem);

    if response.contains("ERROR") {
        crate::state::debug_trace_with_source(format!("[MBN] Select failed: {}", response.trim()), "system");
        return Err(ApiError::internal(format!(
            "MBN select failed: {}",
            response.trim()
        )));
    }

    crate::state::debug_trace_with_source("[MBN] Profile selected OK, running APN enforcement...", "system");
    // Check and fix APN if MBN profile change overwrote it
    ensure_saved_apn(&state, &modem_id).await;

    state
        .audit
        .log(
            AuditEventType::ConfigChanged,
            None,
            format!(
                "{} selected MBN profile: {}",
                session_user.username, req.profile_name
            ),
        )
        .await;

    Ok(Json(MbnActionResult {
        success: true,
        reboot_recommended,
        message: if reboot_recommended {
            format!(
                "Profile '{}' selected. A modem reboot is recommended.",
                req.profile_name
            )
        } else {
            format!("Profile '{}' selected successfully.", req.profile_name)
        },
    }))
}

/// POST /api/modem/:modem_id/mbn/deactivate
///
/// Deactivate the currently active MBN carrier profile.
pub async fn mbn_deactivate(
    Path(modem_id): Path<String>,
    State(state): State<Arc<AppState>>,
    Extension(session_user): Extension<SessionUser>,
) -> ApiResult<Json<MbnActionResult>> {
    require_admin(&session_user)?;
    let handler_arc = require_modem_available(&state, &modem_id).await?;

    // Get profile from modem context
    let modems = state.modems.read().await;
    let context = modems.get(&modem_id)
        .ok_or_else(|| ApiError::not_found(format!("Modem not found: {modem_id}")))?;
    let profile = Arc::clone(&context.profile);
    drop(modems);

    let mbn_cfg = &profile.mbn_config;

    if !mbn_cfg.supported {
        return Err(ApiError::bad_request("MBN management not supported"));
    }

    let deactivate_cmd = mbn_cfg
        .commands
        .deactivate
        .clone()
        .ok_or_else(|| ApiError::bad_request("MBN deactivate not available for this modem"))?;
    let reboot_recommended = mbn_cfg.reboot_recommended;

    crate::state::debug_trace_with_source("[MBN] Deactivating current profile", "system");

    let modem = timeout(Duration::from_secs(2), handler_arc.lock())
        .await
        .map_err(|_| ApiError::service_unavailable_with_retry("Modem busy", 1))?;
    let response = timeout(STATE_CHANGE_TIMEOUT, modem.execute_at(&deactivate_cmd))
        .await
        .map_err(|_| ApiError::service_unavailable_with_retry("MBN deactivate timed out", 5))?
        .map_err(ApiError::from)?;
    drop(modem);

    if response.contains("ERROR") {
        crate::state::debug_trace_with_source(format!("[MBN] Deactivate failed: {}", response.trim()), "system");
        return Err(ApiError::internal(format!(
            "MBN deactivate failed: {}",
            response.trim()
        )));
    }

    crate::state::debug_trace_with_source("[MBN] Profile deactivated OK, running APN enforcement...", "system");
    // Check and fix APN if MBN deactivation cleared it
    ensure_saved_apn(&state, &modem_id).await;

    state
        .audit
        .log(
            AuditEventType::ConfigChanged,
            None,
            format!("{} deactivated MBN profile", session_user.username),
        )
        .await;

    Ok(Json(MbnActionResult {
        success: true,
        reboot_recommended,
        message: if reboot_recommended {
            "Profile deactivated. A modem reboot is recommended.".into()
        } else {
            "Profile deactivated successfully.".into()
        },
    }))
}

/// POST /api/modem/:modem_id/mbn/auto-select
///
/// Toggle MBN auto-select. When enabled, the modem automatically selects
/// the best carrier profile based on the inserted SIM card.
pub async fn mbn_auto_select(
    Path(modem_id): Path<String>,
    State(state): State<Arc<AppState>>,
    Extension(session_user): Extension<SessionUser>,
    Json(req): Json<MbnAutoSelectRequest>,
) -> ApiResult<Json<MbnActionResult>> {
    require_admin(&session_user)?;
    let handler_arc = require_modem_available(&state, &modem_id).await?;

    // Get profile from modem context
    let modems = state.modems.read().await;
    let context = modems.get(&modem_id)
        .ok_or_else(|| ApiError::not_found(format!("Modem not found: {modem_id}")))?;
    let profile = Arc::clone(&context.profile);
    drop(modems);

    let mbn_cfg = &profile.mbn_config;

    if !mbn_cfg.supported {
        return Err(ApiError::bad_request("MBN management not supported"));
    }

    let set_cmd = mbn_cfg
        .commands
        .set_auto_select
        .clone()
        .ok_or_else(|| ApiError::bad_request("MBN auto-select not available for this modem"))?;

    let cmd = set_cmd.replace("{value}", if req.enabled { "1" } else { "0" });

    let modem = timeout(Duration::from_secs(2), handler_arc.lock())
        .await
        .map_err(|_| ApiError::service_unavailable_with_retry("Modem busy", 1))?;
    let response = timeout(STATE_CHANGE_TIMEOUT, modem.execute_at(&cmd))
        .await
        .map_err(|_| ApiError::service_unavailable_with_retry("MBN auto-select timed out", 5))?
        .map_err(ApiError::from)?;
    drop(modem);

    if response.contains("ERROR") {
        return Err(ApiError::internal(format!(
            "MBN auto-select failed: {}",
            response.trim()
        )));
    }

    let action = if req.enabled { "enabled" } else { "disabled" };
    state
        .audit
        .log(
            AuditEventType::ConfigChanged,
            None,
            format!("{} {} MBN auto-select", session_user.username, action),
        )
        .await;

    Ok(Json(MbnActionResult {
        success: true,
        reboot_recommended: false,
        message: format!("Auto-select {action}."),
    }))
}

// =============================================================================
// APN Enforcement Helper
// =============================================================================

/// Check the modem's current APN on the configured CID and fix it if it
/// doesn't match the user's saved config.
///
/// Returns `true` if the APN was changed (radio was cycled and WWAN bounced).
///
/// In ECM mode the modem manages the data connection internally — CGACT
/// deactivate/reactivate does NOT force the ECM bearer to reconnect with
/// a new APN. Only cycling the radio (AT+CFUN=0 → AT+CFUN=1) makes the
/// modem tear down and re-establish the ECM data session.
///
/// When a mismatch is found the sequence is:
///   1. `AT+CFUN=0`          — radio off (tears down ECM bearer)
///   2. `AT+CGDCONT=...`     — set correct APN while radio is off
///   3. `AT+CFUN=1`          — radio on (modem re-registers, ECM reconnects with new APN)
///   4. Wait for re-registration (~5-10s)
///   5. Bounce WWAN interface to get a fresh DHCP lease on usb0
pub async fn ensure_saved_apn(state: &Arc<AppState>, modem_id: &str) -> bool {
    use crate::state::debug_trace_with_source;

    debug_trace_with_source("[APN-ENFORCE] Starting APN enforcement check", "apn");

    // Get modem context and config
    let modems = state.modems.read().await;
    let context = match modems.get(modem_id) {
        Some(ctx) => ctx,
        None => {
            debug_trace_with_source(format!("[APN-ENFORCE] Modem not found: {modem_id}"), "apn");
            return false;
        }
    };
    let conn = context.config.read().await.clone();
    let handler_arc = Arc::clone(&context.handler);
    drop(modems);

    if conn.apn.is_empty() {
        debug_trace_with_source("[APN-ENFORCE] No saved APN configured — skipping", "apn");
        return false;
    }

    let cid = conn.cid;
    let pdp_type = match conn.ip_type {
        IpType::Ipv4 => "IP",
        IpType::Ipv6 => "IPV6",
        IpType::Ipv4v6 => "IPV4V6",
    };
    debug_trace_with_source(format!("[APN-ENFORCE] Saved config: CID={cid} APN=\"{}\" type={pdp_type}", conn.apn), "apn");

    // Step 1: Read current PDP contexts from the modem
    debug_trace_with_source("[APN-ENFORCE] Step 1: Querying modem PDP contexts (AT+CGDCONT?)", "apn");

    let modem = match timeout(Duration::from_secs(2), handler_arc.lock()).await {
        Ok(guard) => guard,
        Err(_) => {
            debug_trace_with_source("[APN-ENFORCE] Failed to acquire modem lock — skipping", "apn");
            return false;
        }
    };
    let current_apn = match timeout(
        STATE_CHANGE_TIMEOUT,
        modem.execute_at("AT+CGDCONT?"),
    )
    .await
    {
        Ok(Ok(resp)) => {
            debug_trace_with_source(format!("[APN-ENFORCE] CGDCONT response: {}", resp.trim().replace('\n', " | ")), "apn");
            parse_cid_apn(&resp, cid)
        }
        Ok(Err(e)) => {
            debug_trace_with_source(format!("[APN-ENFORCE] CGDCONT query failed: {e}"), "apn");
            tracing::warn!("Failed to query CGDCONT: {e}");
            None
        }
        Err(_) => {
            debug_trace_with_source("[APN-ENFORCE] CGDCONT query timed out", "apn");
            tracing::warn!("CGDCONT query timed out");
            None
        }
    };

    // Step 2: Compare — if APN already matches, nothing to do
    let expected = conn.apn.to_lowercase();
    if let Some(ref current) = current_apn {
        if current.to_lowercase() == expected {
            debug_trace_with_source(format!("[APN-ENFORCE] CID {cid} APN already correct: \"{current}\" — no action needed"), "apn");
            return false;
        }
        debug_trace_with_source(format!(
            "[APN-ENFORCE] MISMATCH! CID {cid}: modem has \"{current}\", expected \"{expected}\" — will cycle radio"
        ), "apn");
        tracing::info!(
            "CID {cid} APN mismatch: modem has \"{current}\", expected \"{expected}\" — cycling radio to fix"
        );
    } else {
        debug_trace_with_source(format!("[APN-ENFORCE] CID {cid} has no APN — will set \"{expected}\" with radio cycle"), "apn");
        tracing::info!("CID {cid} has no APN set — applying \"{expected}\" with radio cycle");
    }

    // Step 3: CFUN=0 — radio off (tears down ECM bearer)
    debug_trace_with_source("[APN-ENFORCE] Step 3: AT+CFUN=0 — turning radio OFF", "apn");
    match timeout(STATE_CHANGE_TIMEOUT, modem.execute_at("AT+CFUN=0")).await {
        Ok(Ok(resp)) if !resp.contains("ERROR") => {
            debug_trace_with_source("[APN-ENFORCE] Radio off OK", "apn");
        }
        Ok(Ok(resp)) => {
            debug_trace_with_source(format!("[APN-ENFORCE] CFUN=0 ERROR: {} — continuing anyway", resp.trim()), "apn");
            tracing::warn!("CFUN=0 got ERROR: {} — attempting CGDCONT anyway", resp.trim());
        }
        Ok(Err(e)) => {
            debug_trace_with_source(format!("[APN-ENFORCE] CFUN=0 failed: {e} — continuing anyway"), "apn");
            tracing::warn!("CFUN=0 failed: {e} — attempting CGDCONT anyway");
        }
        Err(_) => {
            debug_trace_with_source("[APN-ENFORCE] CFUN=0 timed out — continuing anyway", "apn");
            tracing::warn!("CFUN=0 timed out — attempting CGDCONT anyway");
        }
    }
    debug_trace_with_source("[APN-ENFORCE] Waiting 1s for radio shutdown...", "apn");
    tokio::time::sleep(Duration::from_secs(1)).await;

    // Step 4: Set the correct APN while radio is off
    let cgdcont_cmd = format!("AT+CGDCONT={cid},\"{pdp_type}\",\"{}\"", conn.apn);
    debug_trace_with_source(format!("[APN-ENFORCE] Step 4: Setting APN — {cgdcont_cmd}"), "apn");
    match timeout(STATE_CHANGE_TIMEOUT, modem.execute_at(&cgdcont_cmd)).await {
        Ok(Ok(resp)) if !resp.contains("ERROR") => {
            debug_trace_with_source(format!("[APN-ENFORCE] APN set OK on CID {cid}: \"{}\"", conn.apn), "apn");
        }
        Ok(Ok(resp)) => {
            debug_trace_with_source(format!("[APN-ENFORCE] CGDCONT ERROR: {} — aborting, turning radio back on", resp.trim()), "apn");
            let _ = timeout(STATE_CHANGE_TIMEOUT, modem.execute_at("AT+CFUN=1")).await;
            return false;
        }
        Ok(Err(e)) => {
            debug_trace_with_source(format!("[APN-ENFORCE] CGDCONT failed: {e} — aborting, turning radio back on"), "apn");
            let _ = timeout(STATE_CHANGE_TIMEOUT, modem.execute_at("AT+CFUN=1")).await;
            return false;
        }
        Err(_) => {
            debug_trace_with_source("[APN-ENFORCE] CGDCONT timed out — aborting, turning radio back on", "apn");
            let _ = timeout(STATE_CHANGE_TIMEOUT, modem.execute_at("AT+CFUN=1")).await;
            return false;
        }
    }

    // Step 5: CFUN=1 — radio on (modem re-registers, ECM reconnects with new APN)
    tokio::time::sleep(Duration::from_millis(500)).await;
    debug_trace_with_source("[APN-ENFORCE] Step 5: AT+CFUN=1 — turning radio ON", "apn");
    match timeout(STATE_CHANGE_TIMEOUT, modem.execute_at("AT+CFUN=1")).await {
        Ok(Ok(resp)) if !resp.contains("ERROR") => {
            debug_trace_with_source(format!("[APN-ENFORCE] Radio on OK — modem will re-register with APN \"{}\"", conn.apn), "apn");
        }
        Ok(Ok(resp)) => {
            debug_trace_with_source(format!("[APN-ENFORCE] CFUN=1 ERROR: {}", resp.trim()), "apn");
            tracing::warn!("CFUN=1 got ERROR: {}", resp.trim());
        }
        Ok(Err(e)) => {
            debug_trace_with_source(format!("[APN-ENFORCE] CFUN=1 failed: {e}"), "apn");
            tracing::warn!("CFUN=1 failed: {e}");
        }
        Err(_) => {
            debug_trace_with_source("[APN-ENFORCE] CFUN=1 timed out", "apn");
            tracing::warn!("CFUN=1 timed out");
        }
    }

    // Drop modem lock before waiting for re-registration
    drop(modem);

    // Step 6: Wait for modem to re-register on network
    debug_trace_with_source("[APN-ENFORCE] Step 6: Waiting 10s for network re-registration...", "apn");
    tokio::time::sleep(Duration::from_secs(10)).await;

    // Step 7: Bounce WWAN to get a fresh DHCP lease
    debug_trace_with_source("[APN-ENFORCE] Step 7: Bouncing WWAN interface (ifdown/ifup)...", "apn");
    crate::api::websocket::bounce_wwan_interface().await;

    debug_trace_with_source("[APN-ENFORCE] Complete — radio cycled, WWAN bounced", "apn");
    true
}

/// Extract the APN for a specific CID from an AT+CGDCONT? response.
fn parse_cid_apn(response: &str, target_cid: u8) -> Option<String> {
    for line in response.lines() {
        let line = line.trim();
        if let Some(after) = line.strip_prefix("+CGDCONT:") {
            let parts: Vec<&str> = after.trim().split(',').collect();
            if parts.len() >= 3 {
                if let Ok(cid) = parts[0].trim().parse::<u8>() {
                    if cid == target_cid {
                        let apn = parts[2].trim().trim_matches('"');
                        return if apn.is_empty() { None } else { Some(apn.to_string()) };
                    }
                }
            }
        }
    }
    None
}

// =============================================================================
// APN Profile Management
// =============================================================================

/// GET /api/modem/:modem_id/apn-profiles
///
/// List APN profiles for the specified modem (filtered by modem_profile_id).
pub async fn list_apn_profiles(
    Path(modem_id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<Vec<crate::hardware::ApnProfile>>> {
    let modems = state.modems.read().await;
    let context = modems.get(&modem_id).ok_or_else(|| {
        ApiError::not_found(format!("Modem not found: {modem_id}"))
    })?;
    let active_profile_id = context.profile.profile_id();
    drop(modems);

    let profiles = state.apn_profiles.read().await;
    let filtered: Vec<_> = profiles
        .iter()
        .filter(|p| p.modem_profile_id == active_profile_id)
        .cloned()
        .collect();
    Ok(Json(filtered))
}

/// POST /api/modem/:modem_id/apn-profiles
///
/// Create a new APN profile.
pub async fn create_apn_profile(
    Path(modem_id): Path<String>,
    State(state): State<Arc<AppState>>,
    Extension(session_user): Extension<SessionUser>,
    Json(mut req): Json<crate::hardware::ApnProfileRequest>,
) -> Result<(StatusCode, Json<crate::hardware::ApnProfile>), ApiError> {
    require_admin(&session_user)?;

    // Validate
    let name = req.name.trim().to_string();
    if name.is_empty() || name.len() > 50 {
        return Err(ApiError::bad_request("Profile name must be 1-50 characters"));
    }
    if req.connection.apn.is_empty() {
        return Err(ApiError::bad_request("APN is required"));
    }
    if req.connection.cid == 0 || req.connection.cid > 8 {
        return Err(ApiError::bad_request("CID must be 1-8"));
    }
    // AT-injection front door: reject `"`/control chars in apn/username/password.
    validate_connection_at_args(&req.connection)?;

    // Unedited password (`None`) on Save-as-Custom: capture the modem's live PDP
    // password for this CID so the working password is preserved rather than
    // silently dropped. Best-effort and done WITHOUT holding the profiles write
    // lock (acquired below). On any failure the password stays `None`/empty and
    // the save proceeds. `Some("")` (explicit clear) and `Some(v)` are left as-is.
    // Security: the captured value is never logged/audited/traced.
    if req.connection.password.is_none() {
        req.connection.password = capture_live_password(&state, &modem_id, req.connection.cid).await;
    }

    let mut profiles = state.apn_profiles.write().await;

    // Check name uniqueness within modem
    if profiles.iter().any(|p| {
        p.modem_profile_id == req.modem_profile_id && p.name.eq_ignore_ascii_case(&name)
    }) {
        return Err(ApiError::bad_request(format!(
            "A profile named '{name}' already exists for this modem"
        )));
    }

    let now = chrono::Utc::now().to_rfc3339();
    let profile = crate::hardware::ApnProfile {
        id: uuid::Uuid::new_v4().to_string(),
        name,
        modem_profile_id: req.modem_profile_id,
        connection: req.connection,
        mbn_profile: req.mbn_profile,
        created_at: now.clone(),
        updated_at: now,
    };

    profiles.push(profile.clone());

    // Persist to disk
    if let Err(e) = crate::config::apn_profiles::save_apn_profiles(&profiles).await {
        tracing::warn!("Failed to persist APN profiles: {e}");
    }

    state
        .audit
        .log(
            AuditEventType::ConfigChanged,
            None,
            format!("{} created APN profile: {}", session_user.username, profile.name),
        )
        .await;

    crate::state::debug_trace_with_source(format!("[APN-PROFILE] Created: \"{}\" ({})", profile.name, profile.id), "apn");
    Ok((StatusCode::CREATED, Json(profile)))
}

/// PUT /api/modem/:modem_id/apn-profiles/:id
///
/// Update an existing APN profile.
pub async fn update_apn_profile(
    Path((_modem_id, id)): Path<(String, String)>,
    State(state): State<Arc<AppState>>,
    Extension(session_user): Extension<SessionUser>,
    Json(mut req): Json<crate::hardware::ApnProfileRequest>,
) -> ApiResult<Json<crate::hardware::ApnProfile>> {
    require_admin(&session_user)?;

    let name = req.name.trim().to_string();
    if name.is_empty() || name.len() > 50 {
        return Err(ApiError::bad_request("Profile name must be 1-50 characters"));
    }
    if req.connection.apn.is_empty() {
        return Err(ApiError::bad_request("APN is required"));
    }
    if req.connection.cid == 0 || req.connection.cid > 8 {
        return Err(ApiError::bad_request("CID must be 1-8"));
    }
    // AT-injection front door: reject `"`/control chars in apn/username/password.
    validate_connection_at_args(&req.connection)?;

    let mut profiles = state.apn_profiles.write().await;

    // Check name uniqueness (excluding this profile)
    if profiles.iter().any(|p| {
        p.id != id && p.modem_profile_id == req.modem_profile_id && p.name.eq_ignore_ascii_case(&name)
    }) {
        return Err(ApiError::bad_request(format!(
            "A profile named '{name}' already exists for this modem"
        )));
    }

    let profile = profiles
        .iter_mut()
        .find(|p| p.id == id)
        .ok_or_else(|| ApiError::not_found("APN profile not found"))?;

    // Unedited password (`None`): preserve the existing stored profile's
    // password so an edit that leaves the field untouched does not drop it.
    // Pure in-memory merge — no modem read. `Some("")` (explicit clear) and
    // `Some(v)` are honored as provided.
    // Security: the preserved value is never logged/audited/traced.
    req.connection.password =
        resolve_update_password(req.connection.password, profile.connection.password.as_deref());

    profile.name = name;
    profile.modem_profile_id = req.modem_profile_id;
    profile.connection = req.connection;
    profile.mbn_profile = req.mbn_profile;
    profile.updated_at = chrono::Utc::now().to_rfc3339();

    let updated = profile.clone();

    if let Err(e) = crate::config::apn_profiles::save_apn_profiles(&profiles).await {
        tracing::warn!("Failed to persist APN profiles: {e}");
    }

    state
        .audit
        .log(
            AuditEventType::ConfigChanged,
            None,
            format!("{} updated APN profile: {}", session_user.username, updated.name),
        )
        .await;

    crate::state::debug_trace_with_source(format!("[APN-PROFILE] Updated: \"{}\" ({})", updated.name, updated.id), "apn");
    Ok(Json(updated))
}

/// DELETE /api/modem/:modem_id/apn-profiles/:id
///
/// Delete an APN profile.
pub async fn delete_apn_profile(
    Path((_modem_id, id)): Path<(String, String)>,
    State(state): State<Arc<AppState>>,
    Extension(session_user): Extension<SessionUser>,
) -> ApiResult<Json<serde_json::Value>> {
    require_admin(&session_user)?;

    let mut profiles = state.apn_profiles.write().await;

    let idx = profiles
        .iter()
        .position(|p| p.id == id)
        .ok_or_else(|| ApiError::not_found("APN profile not found"))?;

    let removed = profiles.remove(idx);

    if let Err(e) = crate::config::apn_profiles::save_apn_profiles(&profiles).await {
        tracing::warn!("Failed to persist APN profiles: {e}");
    }

    state
        .audit
        .log(
            AuditEventType::ConfigChanged,
            None,
            format!("{} deleted APN profile: {}", session_user.username, removed.name),
        )
        .await;

    crate::state::debug_trace_with_source(format!("[APN-PROFILE] Deleted: \"{}\" ({})", removed.name, removed.id), "apn");
    Ok(Json(serde_json::json!({ "success": true })))
}

/// POST /api/modem/:modem_id/apn-profiles/apply
///
/// Apply a saved APN profile. Routes through the diff-aware apply core
/// ([`apply_apn_diff`], Task 5) so the modem reboots **only if the saved
/// profile's MBN selection differs from the modem's current MBN state** — an
/// APN-only change (or a profile whose MBN already matches) is a live write
/// with no radio cycle and no reboot.
///
/// A saved profile always expresses a *definite* MBN intent (it can never mean
/// "leave MBN unchanged"), so `profile.mbn_profile: Option<String>` maps to the
/// core's three-state field as `Some(profile.mbn_profile)`:
/// - profile `None` → `Some(None)` (Auto)
/// - profile `Some("X")` → `Some(Some("X"))` (specific)
pub async fn apply_apn_profile(
    Path(modem_id): Path<String>,
    State(state): State<Arc<AppState>>,
    Extension(session_user): Extension<SessionUser>,
    Json(req): Json<crate::hardware::ApnProfileApplyRequest>,
) -> ApiResult<Json<crate::hardware::ApnProfileApplyResult>> {
    require_admin(&session_user)?;

    use crate::state::debug_trace_with_source;

    // Look up the saved APN profile.
    let profiles = state.apn_profiles.read().await;
    let apn_profile = profiles
        .iter()
        .find(|p| p.id == req.profile_id)
        .cloned()
        .ok_or_else(|| ApiError::not_found("APN profile not found"))?;
    drop(profiles);

    debug_trace_with_source(
        format!(
            "[APN-PROFILE] Applying profile \"{}\" (MBN: {:?})",
            apn_profile.name,
            apn_profile.mbn_profile.as_deref().unwrap_or("none")
        ),
        "apn",
    );

    // Build the diff-aware params from the saved profile. The profile's MBN
    // intent is ALWAYS definite — wrap it in the outer `Some` so the core treats
    // it as a target to diff, never as "leave unchanged".
    let params = ApplyParams {
        cid: apn_profile.connection.cid,
        apn: apn_profile.connection.apn.clone(),
        ip_type: apn_profile.connection.ip_type,
        auth_type: apn_profile.connection.auth_type,
        username: apn_profile.connection.username.clone().unwrap_or_default(),
        // Ignored on entry; the core resolves the write password from
        // `provided_password` (the saved profile's own password) below.
        password: String::new(),
        mbn_profile: Some(apn_profile.mbn_profile.clone()),
    };

    // The saved profile carries its own password — pass it as the provided
    // value (NOT the preserve/re-read path). Never logged/returned.
    let result = apply_apn_diff(
        &state,
        &modem_id,
        params,
        apn_profile.connection.password.as_deref(),
    )
    .await?;

    // Non-regression: persist the profile's connection to the GLOBAL config +
    // DISK so the reconnect watcher's APN enforcement keeps working. The
    // diff-aware core only writes the per-modem in-memory `context.config`,
    // which is what the watcher (`ensure_saved_apn`) reads — but that per-modem
    // config is (re-)seeded from `state.config.connection` on every modem
    // re-enumeration (`add_modem`) and from disk at startup. Writing both the
    // global config and disk here is what carries the correct APN into those
    // later in-memory reads across re-detection and daemon restart, so this
    // block must stay here.
    {
        let mut app_config = state.config.write().await;
        app_config.connection = apn_profile.connection.clone();
        if let Err(e) = crate::config::save_config(&app_config).await {
            tracing::warn!("Failed to persist config to disk: {e}");
        }
    }
    debug_trace_with_source("[APN-PROFILE] Saved connection config to disk", "apn");

    // Audit — no password in the message (security rule #2).
    state
        .audit
        .log(
            AuditEventType::ConfigChanged,
            None,
            format!(
                "{} applied APN profile: {} (MBN: {:?}, mbn_changed: {}, reboot: {})",
                session_user.username,
                apn_profile.name,
                apn_profile.mbn_profile.as_deref().unwrap_or("none"),
                result.mbn_changed,
                result.rebooted,
            ),
        )
        .await;

    // Map the diff-aware result back onto the unchanged ApnProfileApplyResult
    // shape the Manage Profiles dialog consumes. `had_errors` is derived via the
    // shared `step_log_has_errors` helper (single source of truth).
    let had_errors = step_log_has_errors(&result.step_log);

    Ok(Json(crate::hardware::ApnProfileApplyResult {
        success: result.success,
        had_errors,
        step_log: result.step_log,
        reboot_triggered: result.rebooted,
    }))
}

/// GET /api/modem/apn-profiles/export
/// GET /api/modem/:modem_id/apn-profiles/export
///
/// Export all APN profiles as a JSON array. Useful for building pre-loaded
/// profiles into the software for distribution.
pub async fn export_apn_profiles(
    Path(_modem_id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<Vec<crate::hardware::ApnProfile>>> {
    let profiles = state.apn_profiles.read().await;
    Ok(Json(profiles.clone()))
}

/// POST /api/modem/:modem_id/apn-profiles/import
///
/// Import APN profiles from a JSON array. Assigns new UUIDs and timestamps.
/// Skips profiles whose name already exists for the same modem_profile_id.
pub async fn import_apn_profiles(
    Path(_modem_id): Path<String>,
    State(state): State<Arc<AppState>>,
    Extension(session_user): Extension<SessionUser>,
    Json(incoming): Json<Vec<crate::hardware::ApnProfileRequest>>,
) -> ApiResult<Json<crate::hardware::ApnProfileImportResult>> {
    require_admin(&session_user)?;

    let mut profiles = state.apn_profiles.write().await;
    let now = chrono::Utc::now().to_rfc3339();
    let mut imported = 0usize;
    let mut skipped = 0usize;

    for req in &incoming {
        let name = req.name.trim().to_string();
        if name.is_empty() || name.len() > 50 {
            skipped += 1;
            continue;
        }
        if req.connection.apn.is_empty() {
            skipped += 1;
            continue;
        }
        if req.connection.cid == 0 || req.connection.cid > 8 {
            skipped += 1;
            continue;
        }
        // AT-injection front door: skip entries whose apn/username/password
        // carry a `"` or control char (would break the quoted AT write).
        if validate_connection_at_args(&req.connection).is_err() {
            skipped += 1;
            continue;
        }

        // Skip duplicates by name within same modem
        if profiles.iter().any(|p| {
            p.modem_profile_id == req.modem_profile_id
                && p.name.eq_ignore_ascii_case(&name)
        }) {
            skipped += 1;
            continue;
        }

        profiles.push(crate::hardware::ApnProfile {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            modem_profile_id: req.modem_profile_id.clone(),
            connection: req.connection.clone(),
            mbn_profile: req.mbn_profile.clone(),
            created_at: now.clone(),
            updated_at: now.clone(),
        });
        imported += 1;
    }

    if imported > 0 {
        if let Err(e) = crate::config::apn_profiles::save_apn_profiles(&profiles).await {
            tracing::warn!("Failed to persist APN profiles: {e}");
        }
    }

    state
        .audit
        .log(
            AuditEventType::ConfigChanged,
            None,
            format!(
                "{} imported APN profiles: {imported} added, {skipped} skipped",
                session_user.username
            ),
        )
        .await;

    crate::state::debug_trace_with_source(format!(
        "[APN-PROFILE] Import: {imported} added, {skipped} skipped"
    ), "apn");

    let message = if skipped > 0 {
        format!("Imported {imported} profile(s), skipped {skipped} (duplicate or invalid)")
    } else {
        format!("Imported {imported} profile(s)")
    };

    Ok(Json(crate::hardware::ApnProfileImportResult {
        imported,
        skipped,
        message,
    }))
}

// =============================================================================
// MBN Parsing Helpers
// =============================================================================

/// Parse AT+QMBNCFG="List" response into structured MBN profiles.
///
/// Format: `+QMBNCFG: "List",<index>,<selected>,<activated>,"<name>",<version_hex>,<revision>`
fn parse_mbn_list(response: &str) -> Vec<MbnProfile> {
    response
        .lines()
        .filter(|l| {
            let trimmed = l.trim();
            trimmed.starts_with("+QMBNCFG:") && trimmed.contains("\"List\"")
        })
        .filter_map(|line| {
            // Strip the prefix: +QMBNCFG: "List",
            let after_list = line
                .trim()
                .strip_prefix("+QMBNCFG:")?
                .trim()
                .strip_prefix("\"List\",")?;
            // Split remaining: <index>,<selected>,<activated>,"<name>",<version>,<revision>
            let parts: Vec<&str> = after_list.splitn(5, ',').collect();
            if parts.len() >= 5 {
                let index = parts[0].trim().parse::<u32>().ok()?;
                let selected = parts[1].trim() == "1";
                let activated = parts[2].trim() == "1";
                // Name is quoted, may contain commas in theory — but in practice doesn't
                let name = parts[3].trim().trim_matches('"').to_string();
                // Remaining part has version,revision
                let rest = parts[4];
                let rest_parts: Vec<&str> = rest.splitn(2, ',').collect();
                let version = rest_parts.first().map(|s| s.trim().to_string()).unwrap_or_default();
                let revision = rest_parts.get(1).map(|s| s.trim().to_string()).unwrap_or_default();
                Some(MbnProfile {
                    index,
                    selected,
                    activated,
                    name,
                    version,
                    revision,
                })
            } else {
                None
            }
        })
        .collect()
}

/// Parse AT+QMBNCFG="AutoSel" response into bool (0=off, 1=on).
///
/// 2-field form: `+QMBNCFG: "AutoSel",0\r\n\r\nOK` → Some(false)
/// 3-field form: `+QMBNCFG: "AutoSel",1,"Commercial-TMO"\r\n\r\nOK` → Some(true)
///
/// The enable flag is the field **immediately after** the `"AutoSel"` token
/// (the second comma-field), NOT the last field — otherwise the documented
/// Quectel 3-field variant mis-parses to `None`, which would feed the same
/// `None → reboot` failure the fail-safe diff is guarding against.
/// Returns `None` only for a genuinely missing/malformed response.
fn parse_mbn_auto_select(response: &str) -> Option<bool> {
    response
        .lines()
        .find(|l| {
            // Structurally strict (R4, 2026-06-18 read-framing spec): the flag may
            // ONLY come from a genuine `+QMBNCFG:` RESPONSE line whose query token
            // is `AutoSel`. Anchoring on the trimmed-line prefix (not a free
            // `contains`) means a URC that merely mentions the AutoSel token in
            // its payload can never be substring-matched into a bogus flag.
            let trimmed = l.trim();
            trimmed.to_uppercase().starts_with("+QMBNCFG:")
                && trimmed.to_uppercase().contains("AUTOSEL")
        })
        .and_then(|line| {
            // Split on the first comma: head = `+QMBNCFG: "AutoSel"`,
            // tail = `<flag>[,"<name>"]`. The flag is the first field of tail.
            let (_, tail) = line.trim().split_once(',')?;
            let val = tail.split(',').next()?.trim();
            match val {
                "0" => Some(false),
                "1" => Some(true),
                _ => None,
            }
        })
}

/// Parse AT+QMBNCFG="Select" query response into the selected profile name.
///
/// Input: `+QMBNCFG: "Select",ROW_Commercial\r\n\r\nOK`
/// Output: Some("ROW_Commercial")
///
/// When no profile is selected (after deactivate), the response may be just `OK`
/// or `+QMBNCFG: "Select",` with an empty value.
fn parse_mbn_selected(response: &str) -> Option<String> {
    response
        .lines()
        .find(|l| {
            // Structurally strict (R4): only a genuine `+QMBNCFG:` response line
            // whose query token is `Select` supplies the selected profile name.
            let trimmed = l.trim();
            trimmed.to_uppercase().starts_with("+QMBNCFG:")
                && trimmed.to_uppercase().contains("SELECT")
        })
        .and_then(|line| {
            let val = line.trim().rsplit(',').next()?.trim().trim_matches('"');
            if val.is_empty() {
                None
            } else {
                Some(val.to_string())
            }
        })
}

// ============================================================================
// APN/PDP Live-Config Parsing Helpers (Item #42 Phase 2, Task 3)
// ============================================================================

/// Parsed fields from an AT+QICSGP=<cid> query response.
///
/// Note: ip_type is NOT stored here — CGDCONT's pdp_type is the preferred
/// ip_type source (works for all modems including Telit/generic). QICSGP
/// supplies auth/username/has_password only.
///
/// Security note: the raw password value is intentionally NOT stored here —
/// only `has_password` (bool) is surfaced to callers and the JSON response.
/// See PENDING-CHANGES.md security constraint.
struct QicsgpParsed {
    /// APN string from the modem
    apn: String,
    /// Auth type: "none" | "pap" | "chap"
    auth_type: String,
    /// Username (empty string when none configured)
    username: String,
    /// True iff the password field in the response was non-empty.
    /// The password VALUE is never stored or returned.
    has_password: bool,
}

/// Parse an AT+QICSGP=<cid> response into `QicsgpParsed`.
///
/// Response shape:
/// `+QICSGP: <context_type>,"<apn>","<username>","<password>",<auth>`
///
/// context_type: 1=IPv4, 2=IPv6, 3=IPv4v6
/// auth: 0=none, 1=PAP, 2=CHAP
///
/// Returns `None` if no `+QICSGP:` line is found or if it has fewer than
/// 5 comma-separated fields (context_type + apn + user + pass + auth).
///
/// Security: the password field is consumed only to determine `has_password`;
/// its value is never propagated.
fn parse_qicsgp_response(response: &str) -> Option<QicsgpParsed> {
    let line = response
        .lines()
        .find(|l| l.trim().starts_with("+QICSGP:"))?;

    let after = line.trim().strip_prefix("+QICSGP:")?.trim();

    // Split all 5 fields: context_type, "apn", "username", "password", auth
    // context_type (parts[0]) encodes IP version (1=IPv4, 2=IPv6, 3=IPv4v6) but we
    // prefer CGDCONT pdp_type as the ip_type source (works for all modems, not just
    // Quectel), so parts[0] is intentionally skipped.
    // The auth field is always a bare integer with no commas, so plain split is safe.
    // We cap at 6 parts to tolerate any extra trailing content.
    let parts: Vec<&str> = after.splitn(6, ',').collect();
    if parts.len() < 5 {
        return None;
    }

    // parts[0] = context_type (intentionally unused — ip_type comes from CGDCONT)
    let apn = parts[1].trim().trim_matches('"');
    let username = parts[2].trim().trim_matches('"');
    // Password: consume only to determine has_password — value is discarded.
    let password_field = parts[3].trim().trim_matches('"');
    let auth_raw = parts[4].trim();

    let has_password = !password_field.is_empty();

    let auth_type = match auth_raw {
        "0" => "none",
        "1" => "pap",
        "2" => "chap",
        _ => "none",
    };

    Some(QicsgpParsed {
        apn: apn.to_string(),
        auth_type: auth_type.to_string(),
        username: username.to_string(),
        has_password,
    })
}

/// Parse AT+CGACT? response into a set of active CIDs.
///
/// Response shape: `+CGACT: <cid>,<state>` per line (state 1=active, 0=inactive).
/// Returns a sorted Vec of CIDs whose state is 1.
fn parse_cgact_response(response: &str) -> Vec<u32> {
    let mut active: Vec<u32> = response
        .lines()
        .filter(|l| l.trim().starts_with("+CGACT:"))
        .filter_map(|line| {
            let after = line.trim().strip_prefix("+CGACT:")?.trim();
            let mut parts = after.splitn(2, ',');
            let cid: u32 = parts.next()?.trim().parse().ok()?;
            let state: u32 = parts.next()?.trim().parse().ok()?;
            if state == 1 { Some(cid) } else { None }
        })
        .collect();
    active.sort_unstable();
    active
}

/// Reserved APN names that are excluded from the default-editing-CID heuristic.
///
/// These are system-managed contexts. The list is checked case-insensitively.
/// Extensible: add more names here as additional carrier-reserved APNs are identified.
const RESERVED_APNS: &[&str] = &["ims", "sos"];

/// Compute the default editing CID: lowest CID whose APN is not in RESERVED_APNS.
///
/// "Default editing context" is the CID that the panel should pre-fill for
/// Apply/write operations. System contexts (ims, sos) are excluded.
///
/// Returns `None` if all contexts are reserved or the list is empty.
fn default_editing_cid(pdp_contexts: &[serde_json::Value]) -> Option<u32> {
    let mut candidates: Vec<u32> = pdp_contexts
        .iter()
        .filter_map(|ctx| {
            let cid: u32 = ctx["cid"].as_str()?.trim().parse().ok()?;
            let apn = ctx["apn"].as_str().unwrap_or("").to_lowercase();
            if RESERVED_APNS.contains(&apn.as_str()) {
                None
            } else {
                Some(cid)
            }
        })
        .collect();
    candidates.sort_unstable();
    candidates.into_iter().next()
}

/// Map a CGDCONT pdp_type string to the API ip_type string.
///
/// CGDCONT uses "IP" for IPv4, "IPV6" for IPv6, "IPV4V6" for dual-stack.
/// Comparison is case-insensitive.
fn ip_type_from_pdp_type(pdp_type: &str) -> &'static str {
    match pdp_type.to_uppercase().as_str() {
        "IPV6" => "ipv6",
        "IPV4V6" => "ipv4v6",
        _ => "ipv4", // "IP" and any unknown → ipv4
    }
}

// ============================================================================
// POST /apn/apply — diff-aware live write vs MBN reboot (Item #42 Phase 2, Task 5)
// ============================================================================

/// Sentinel value the frontend may send for `mbn_profile` to mean "Auto".
const MBN_AUTO_SENTINEL: &str = "__auto__";

/// Request body for `POST /api/modem/:modem_id/apn/apply`.
///
/// The `mbn_profile` field is three-state to distinguish:
/// - **omitted** (`None`) — leave the MBN selection unchanged
/// - **null** (`Some(None)`) — set MBN to Auto (`AT+QMBNCFG="AutoSel",1`)
/// - **`"__auto__"`** (`Some(Some("__auto__"))`) — also Auto
/// - **a profile name** (`Some(Some(name))`) — select that specific profile
#[derive(Debug, Clone, Deserialize)]
pub struct ApnApplyRequest {
    /// PDP context ID (1-8) to write.
    pub cid: u8,
    /// APN string (required, 1-100 chars).
    pub apn: String,
    /// IP protocol version (required).
    pub ip_type: IpType,
    /// Authentication type (required; `none` is valid).
    pub auth_type: AuthType,
    /// Optional username.
    #[serde(default)]
    pub username: Option<String>,
    /// Optional password. Omitted/null = leave the stored password unchanged
    /// (placeholder-untouched rule, spec §11). Provided (incl. "") = use it.
    #[serde(default)]
    pub password: Option<String>,
    /// Three-state MBN target. See struct docs.
    #[serde(default)]
    pub mbn_profile: Option<Option<String>>,
}

/// Response body for `POST /api/modem/:modem_id/apn/apply`.
#[derive(Debug, Clone, Serialize)]
pub struct ApnApplyResult {
    pub success: bool,
    /// Derived: a `step_log` line contains ERROR/Failed/Timeout. Drives the
    /// frontend warning tone when `success` is still true.
    pub had_errors: bool,
    pub mbn_changed: bool,
    pub rebooted: bool,
    /// Human-readable step labels. **Never** contains the password or the
    /// filled QICSGP write command (security rule #2).
    pub step_log: Vec<String>,
    pub message: String,
}

/// Derive whether a step log records any failure. Single source of truth for the
/// warning-tone signal shared by `ApnApplyResult` and `ApnProfileApplyResult`:
/// true iff a line records a failure, matched case-insensitively against
/// `error`, `failed`, `timeout`, or `timed out`. The case-insensitive match plus
/// the `timed out` token also catch the reboot-failure labels
/// ("Reboot failed: ...", "Reboot command timed out"). Reads only the
/// already-sanitized step labels (the password is never present — security rule #2).
fn step_log_has_errors(step_log: &[String]) -> bool {
    step_log.iter().any(|s| {
        let lower = s.to_lowercase();
        lower.contains("error")
            || lower.contains("failed")
            || lower.contains("timeout")
            || lower.contains("timed out")
    })
}

/// The resolved MBN write intent after diffing the request against the modem.
#[derive(Debug, Clone, PartialEq, Eq)]
enum MbnTarget {
    /// Enable AutoSel (`AT+QMBNCFG="AutoSel",1`).
    Auto,
    /// Select a specific profile (AutoSel 0 + Select "<name>").
    Profile(String),
}

/// Compute whether the MBN selection changed and, if so, the write target.
///
/// Inputs:
/// - `requested`: the three-state request field (omitted / null / sentinel / name).
/// - `current_auto`: current `AT+QMBNCFG="AutoSel"` value (`Some(true)`=auto).
/// - `current_selected`: current `AT+QMBNCFG="Select"` profile name, if any.
///
/// Returns `(changed, target)`. When `changed` is false, `target` is a
/// placeholder (`MbnTarget::Auto`) that callers must ignore.
///
/// Diff rules (fail-safe, 2026-06-17 spec — reboot only on a
/// positively-confirmed difference; an unread/unknown current state is treated
/// as "no change" so a transient AT-read miss never reboots a working link):
/// - omitted → never changed.
/// - Auto (null or sentinel) → changed iff AutoSel is **confirmed off**
///   (`current_auto == Some(false)`). `None` (unread) → unchanged.
/// - specific name → changed iff AutoSel is **confirmed on**
///   (`Some(true)`), OR AutoSel is **confirmed off** AND a **confirmed
///   different** profile is selected. `current_auto == None`, or auto off with
///   `current_selected == None` (unread) → unchanged.
fn compute_mbn_diff(
    requested: &Option<Option<String>>,
    current_auto: Option<bool>,
    current_selected: Option<&str>,
) -> (bool, MbnTarget) {
    match requested {
        // Omitted — leave unchanged.
        None => (false, MbnTarget::Auto),
        // null or "__auto__" → Auto.
        //
        // Fail-safe (2026-06-17 spec): a reboot must only fire on a
        // positively-confirmed difference. The modem is "not already auto"
        // ONLY when AutoSel is confirmed off (`Some(false)`). An unread state
        // (`None`) is treated as "no change" — no reboot — so a transient
        // AT-read hiccup never destroys a working link.
        Some(None) => {
            let changed = current_auto == Some(false);
            (changed, MbnTarget::Auto)
        }
        Some(Some(name)) if name == MBN_AUTO_SENTINEL => {
            let changed = current_auto == Some(false);
            (changed, MbnTarget::Auto)
        }
        // Specific profile.
        //
        // Fail-safe: changed ONLY when
        //   - AutoSel is confirmed ON (`Some(true)`) — selecting a profile is a
        //     genuine switch out of auto, OR
        //   - AutoSel is confirmed OFF (`Some(false)`) AND a different profile is
        //     confirmed selected.
        // If `current_auto` is `None` (unread), or auto is off but
        // `current_selected` is `None` (unread), → no change / no reboot.
        Some(Some(name)) => {
            let changed = match current_auto {
                Some(true) => true,
                Some(false) => match current_selected {
                    Some(selected) => selected != name.as_str(),
                    None => false,
                },
                None => false,
            };
            (changed, MbnTarget::Profile(name.clone()))
        }
    }
}

/// Extract the **password** field from an `AT+QICSGP=<cid>` response.
///
/// Response shape: `+QICSGP: <type>,"<apn>","<user>","<pass>",<auth>`.
/// Returns `Some("")` when the password field is present but empty (open APN),
/// `None` when no `+QICSGP:` line is found or it is malformed.
///
/// Security: this value is consumed ONLY to re-supply the existing password to
/// the write command when the request omits it. It is never logged or returned.
fn parse_qicsgp_password(response: &str) -> Option<String> {
    let line = response
        .lines()
        .find(|l| l.trim().starts_with("+QICSGP:"))?;
    let after = line.trim().strip_prefix("+QICSGP:")?.trim();
    let parts: Vec<&str> = after.splitn(6, ',').collect();
    if parts.len() < 5 {
        return None;
    }
    Some(parts[3].trim().trim_matches('"').to_string())
}

/// Resolve the password to write per the placeholder-untouched rule (§11).
///
/// - `provided = Some(p)` → use `p` verbatim (incl. empty string = clear).
/// - `provided = None` (omitted/null in request) → preserve the existing
///   password parsed from the current `QICSGP=<cid>` response, or `""` if none.
///
/// Security: the returned value is used ONLY to build the (never-logged) write
/// command. Callers must not log or return it.
fn resolve_apply_password(provided: Option<&str>, current_qicsgp: Option<&str>) -> String {
    match provided {
        Some(p) => p.to_string(),
        None => current_qicsgp
            .and_then(parse_qicsgp_password)
            .unwrap_or_default(),
    }
}

/// Resolve the password to store on an APN-profile **update**, per the
/// unedited-field rule.
///
/// - `incoming = Some(p)` → use `p` verbatim (incl. `""` = explicit clear).
/// - `incoming = None` (field omitted = unedited) → preserve the `existing`
///   stored password.
///
/// Pure / no modem read. Security: the returned value is load-bearing-secret;
/// callers must never log, audit, or trace it.
fn resolve_update_password(incoming: Option<String>, existing: Option<&str>) -> Option<String> {
    match incoming {
        Some(p) => Some(p),
        None => existing.map(str::to_string),
    }
}

/// Best-effort capture of the modem's live PDP password for a CID.
///
/// Used by `create_apn_profile` when the incoming request omits the password
/// (`None` = "unedited") so a Save-as-Custom preserves the working password
/// instead of silently clearing it.
///
/// Mirrors the live re-read `apply_apn_diff` performs at §11: build the QICSGP
/// query from the modem profile's `apn_live_config.query` template (`{cid}`
/// substitution), `execute_at`, and parse with [`parse_qicsgp_password`].
///
/// **Best-effort:** returns `None` (never errors) if the modem is missing /
/// unavailable / busy, the profile has no `apn_live_config.query` (Telit /
/// generic), or the AT query times out or errors. The caller must proceed with
/// the save regardless.
///
/// **Lock hygiene:** acquires and releases the modem lock entirely within this
/// function; the caller must NOT hold the `apn_profiles` write lock across the
/// call.
///
/// Security: the returned value is load-bearing-secret — the caller stores it
/// into the profile but must never log, audit, trace, or otherwise surface it.
async fn capture_live_password(state: &Arc<AppState>, modem_id: &str, cid: u8) -> Option<String> {
    // Read the profile's QICSGP query template without holding any modem lock.
    let query_tpl = {
        let modems = state.modems.read().await;
        let context = modems.get(modem_id)?;
        context.profile.apn_live_config.query.clone()?
    };

    // Modem must be healthy; bail (best-effort) if not.
    let handler_arc = require_modem_available(state, modem_id).await.ok()?;
    let modem = timeout(Duration::from_secs(2), handler_arc.lock()).await.ok()?;

    let cmd = query_tpl.replace("{cid}", &cid.to_string());
    let resp = timeout(QUICK_TIMEOUT, modem.execute_at(&cmd))
        .await
        .ok()?
        .ok()?;
    drop(modem);

    parse_qicsgp_password(&resp)
}

/// Numeric QICSGP context_type for an IpType (1=IPv4, 2=IPv6, 3=IPv4v6).
fn qicsgp_context_type(ip_type: IpType) -> u8 {
    match ip_type {
        IpType::Ipv4 => 1,
        IpType::Ipv6 => 2,
        IpType::Ipv4v6 => 3,
    }
}

/// Numeric QICSGP auth code (0=none, 1=PAP, 2=CHAP).
fn qicsgp_auth_code(auth: AuthType) -> u8 {
    match auth {
        AuthType::None => 0,
        AuthType::Pap => 1,
        AuthType::Chap => 2,
    }
}

/// CGDCONT pdp_type string for the fallback write path.
fn cgdcont_pdp_type(ip_type: IpType) -> &'static str {
    match ip_type {
        IpType::Ipv4 => "IP",
        IpType::Ipv6 => "IPV6",
        IpType::Ipv4v6 => "IPV4V6",
    }
}

/// Resolved, validated parameters for the diff-aware apply core.
///
/// `password` is the already-resolved value (provided-or-preserved). It is
/// load-bearing-secret: the core uses it only to fill the (never-logged)
/// QICSGP write command.
struct ApplyParams {
    cid: u8,
    apn: String,
    ip_type: IpType,
    auth_type: AuthType,
    username: String,
    /// Resolved password (provided or preserved). NEVER logged/returned.
    password: String,
    /// MBN request field (three-state) — diffed inside the core.
    mbn_profile: Option<Option<String>>,
}

/// POST /api/modem/:modem_id/apn/apply
///
/// Diff-aware APN apply. Reads the modem's current MBN state, diffs against the
/// request, and either:
/// - **live-writes** the APN/auth/IP via `AT+QICSGP` (Quectel) or `AT+CGDCONT`
///   (fallback) with NO radio cycle and NO reboot (`rebooted=false`), when the
///   MBN selection is unchanged (or MBN is unsupported); or
/// - runs the profile's `apn_apply_config` MBN steps + live write + reboot
///   (`rebooted=true`, `mbn_changed=true`), when the MBN selection changed.
///
/// The server is **idempotent** for the no-change case: a same-value live write
/// is harmless (P0.1) and returns `success=true`. The frontend disables Apply
/// when nothing is dirty; this handler does not reject a no-op.
///
/// Security: the filled QICSGP write command (which carries the password) is
/// NEVER passed to `debug_trace`, placed in `step_log`, returned, or
/// audit-logged. Only a redacted label is recorded.
pub async fn apn_apply(
    Path(modem_id): Path<String>,
    State(state): State<Arc<AppState>>,
    Extension(session_user): Extension<SessionUser>,
    Json(req): Json<ApnApplyRequest>,
) -> ApiResult<Json<ApnApplyResult>> {
    require_admin(&session_user)?;

    // --- Validation (spec §11) ---------------------------------------------
    if req.apn.is_empty() {
        return Err(ApiError::bad_request("APN is required"));
    }
    if req.apn.len() > 100 {
        return Err(ApiError::bad_request("APN too long"));
    }
    if req.cid == 0 || req.cid > 8 {
        return Err(ApiError::bad_request("CID must be 1-8"));
    }
    // ip_type and auth_type are required by the type system (deserialize fails
    // if absent/invalid). `none` auth is valid.

    // AT-injection front door: apn/username/password are interpolated into
    // quoted AT arguments (QICSGP/CGDCONT). Reject `"` and control chars (CR/LF)
    // here so the user gets a 400 instead of a silent hardware-layer error.
    validate_at_arg(&req.apn, "APN")?;
    if let Some(ref u) = req.username {
        validate_at_arg(u, "Username")?;
    }
    if let Some(ref p) = req.password {
        validate_at_arg(p, "Password")?;
    }

    // Keep audit-safe fields before moving into params (NO password — that is
    // resolved inside the core and never surfaces here).
    let audit_cid = req.cid;
    let audit_apn = req.apn.clone();

    let params = ApplyParams {
        cid: req.cid,
        apn: req.apn,
        ip_type: req.ip_type,
        auth_type: req.auth_type,
        username: req.username.unwrap_or_default(),
        // Resolved inside the core (preserve-rule re-read happens there). The
        // request's provided/omitted intent is threaded via `provided_password`.
        password: String::new(),
        mbn_profile: req.mbn_profile,
    };

    let result =
        apply_apn_diff(&state, &modem_id, params, req.password.as_deref()).await?;

    // Audit — message must NOT contain the password (security rule #2).
    state
        .audit
        .log(
            AuditEventType::ConfigChanged,
            None,
            format!(
                "{} applied APN '{}' (cid {}, mbn_changed: {}, rebooted: {})",
                session_user.username, audit_apn, audit_cid, result.mbn_changed, result.rebooted,
            ),
        )
        .await;

    Ok(Json(result))
}

/// Build the live-APN write command (QICSGP when the profile supports it,
/// otherwise the CGDCONT fallback). Returns `(command, redacted_label)`.
///
/// **Security:** the returned `command` may contain the password (QICSGP). The
/// caller must execute it but must log only the `redacted_label`.
fn build_live_write(
    apn_live_write_tpl: Option<&str>,
    params: &ApplyParams,
) -> (String, String) {
    match apn_live_write_tpl {
        Some(tpl) => {
            // QICSGP write — carries the password. Redacted label only.
            let cmd = tpl
                .replace("{cid}", &params.cid.to_string())
                .replace(
                    "{context_type}",
                    &qicsgp_context_type(params.ip_type).to_string(),
                )
                .replace("{apn}", &params.apn)
                .replace("{username}", &params.username)
                .replace("{password}", &params.password)
                .replace("{auth}", &qicsgp_auth_code(params.auth_type).to_string());
            let label = format!("OK: live APN write (QICSGP, cid {})", params.cid);
            (cmd, label)
        }
        None => {
            // CGDCONT fallback — APN + IP only, no auth/password. Safe to log.
            let cmd = format!(
                "AT+CGDCONT={},\"{}\",\"{}\"",
                params.cid,
                cgdcont_pdp_type(params.ip_type),
                params.apn
            );
            let label = format!("OK: live APN write (CGDCONT, cid {})", params.cid);
            (cmd, label)
        }
    }
}

/// Core of the diff-aware apply. Reusable by Task 6 (saved-profile apply).
///
/// `provided_password` carries the request's three-state password intent:
/// `Some(p)` = use `p`; `None` = preserve the stored password (re-read QICSGP).
/// The resolved value lives only inside this function and the (never-logged)
/// write command; it is never placed in `step_log`, the result, or the audit.
///
/// `params.password` is ignored on entry (the handler passes a placeholder);
/// the resolved value is computed here from `provided_password` + the live read.
async fn apply_apn_diff(
    state: &Arc<AppState>,
    modem_id: &str,
    mut params: ApplyParams,
    provided_password: Option<&str>,
) -> Result<ApnApplyResult, ApiError> {
    use crate::state::debug_trace_with_source;

    // Read profile configs before locking the modem.
    let (mbn_cfg, apn_live_cfg, apply_cfg) = {
        let modems = state.modems.read().await;
        let context = modems
            .get(modem_id)
            .ok_or_else(|| ApiError::not_found(format!("Modem not found: {modem_id}")))?;
        (
            context.profile.mbn_config.clone(),
            context.profile.apn_live_config.clone(),
            context.profile.apn_apply_config.clone(),
        )
    };

    let handler_arc = require_modem_available(state, modem_id).await?;
    let modem = timeout(Duration::from_secs(2), handler_arc.lock())
        .await
        .map_err(|_| ApiError::service_unavailable_with_retry("Modem busy", 1))?;

    let mut step_log: Vec<String> = Vec::new();

    // --- Read current MBN state (for the diff) ------------------------------
    let mut current_auto: Option<bool> = None;
    let mut current_selected: Option<String> = None;
    if mbn_cfg.supported {
        if let Some(ref cmd) = mbn_cfg.commands.query_auto_select {
            if let Ok(Ok(resp)) = timeout(QUICK_TIMEOUT, modem.execute_at(cmd)).await {
                current_auto = parse_mbn_auto_select(&resp);
            }
        }
        if let Some(ref cmd) = mbn_cfg.commands.query_selected {
            if let Ok(Ok(resp)) = timeout(QUICK_TIMEOUT, modem.execute_at(cmd)).await {
                current_selected = parse_mbn_selected(&resp);
            }
        }
    }

    // --- Resolve the password (preserve-rule, §11) --------------------------
    // When the request omits the password we re-read the current QICSGP for the
    // CID and re-supply the existing value. Never logged.
    if provided_password.is_none() {
        let current_qicsgp: Option<String> = if let Some(query_tpl) = apn_live_cfg.query.as_deref()
        {
            let cmd = query_tpl.replace("{cid}", &params.cid.to_string());
            timeout(QUICK_TIMEOUT, modem.execute_at(&cmd))
                .await
                .ok()
                .and_then(|r| r.ok())
        } else {
            None
        };
        params.password = resolve_apply_password(None, current_qicsgp.as_deref());
    } else {
        params.password = resolve_apply_password(provided_password, None);
    }

    // --- Compute the MBN diff ----------------------------------------------
    // MBN unsupported ⇒ always treat as unchanged (live-write-only branch).
    let (mbn_changed, mbn_target) = if mbn_cfg.supported {
        compute_mbn_diff(&params.mbn_profile, current_auto, current_selected.as_deref())
    } else {
        (false, MbnTarget::Auto)
    };

    let (live_write_cmd, live_write_label) = build_live_write(apn_live_cfg.write.as_deref(), &params);

    let mut rebooted = false;

    if !mbn_changed {
        // ---------- Branch A: live write only, no reboot --------------------
        // QUICK timeout per the live-write contract (5s).
        match timeout(QUICK_TIMEOUT, modem.execute_at(&live_write_cmd)).await {
            Ok(Ok(resp)) if !resp.contains("ERROR") => {
                debug_trace_with_source(format!("[APN-APPLY] {live_write_label}"), "apn");
                step_log.push(live_write_label);
            }
            Ok(Ok(resp)) => {
                let msg = format!("ERROR on live APN write (cid {}): {}", params.cid, resp.trim());
                debug_trace_with_source(format!("[APN-APPLY] {msg}"), "apn");
                step_log.push(msg);
            }
            Ok(Err(e)) => {
                let msg = format!("Failed live APN write (cid {}) — {e}", params.cid);
                debug_trace_with_source(format!("[APN-APPLY] {msg}"), "apn");
                step_log.push(msg);
            }
            Err(_) => {
                let msg = format!("Timeout on live APN write (cid {})", params.cid);
                debug_trace_with_source(format!("[APN-APPLY] {msg}"), "apn");
                step_log.push(msg);
            }
        }
    } else {
        // ---------- Branch B: MBN change → steps + live write + reboot ------
        // Reuse the profile's apn_apply_config MBN templates. The CGDCONT/APN
        // step in those templates is the modem's own way of writing the APN
        // during a reboot apply; we ALSO issue the live write below for auth.
        let ip_type_str = match params.ip_type {
            IpType::Ipv4 => "IP",
            IpType::Ipv6 => "IPV6",
            IpType::Ipv4v6 => "IPV4V6",
        };
        let mbn_name = match &mbn_target {
            MbnTarget::Profile(n) => n.clone(),
            MbnTarget::Auto => String::new(),
        };
        let to_auto = matches!(mbn_target, MbnTarget::Auto);

        for step in &apply_cfg.steps {
            // MBN-requiring steps (Deactivate / Select / AutoSel,0) only apply
            // when selecting a SPECIFIC profile. For the Auto target we skip
            // them and enable AutoSel below instead.
            if step.requires_mbn && to_auto {
                let msg = format!("Skipped: {} (target is Auto)", step.label);
                debug_trace_with_source(format!("[APN-APPLY] {msg}"), "apn");
                step_log.push(msg);
                continue;
            }

            // Substitute placeholders. NOTE: this is the CGDCONT/MBN template
            // path which has NO password placeholder — safe to log.
            let cmd = step
                .command
                .replace("{mbn_profile}", &mbn_name)
                .replace("{cid}", &params.cid.to_string())
                .replace("{ip_type}", ip_type_str)
                .replace("{apn}", &params.apn);

            debug_trace_with_source(format!("[APN-APPLY] Step: {} → {}", step.label, cmd), "apn");

            let step_timeout = Duration::from_secs(step.timeout_secs);
            match timeout(step_timeout, modem.execute_at(&cmd)).await {
                Ok(Ok(resp)) if !resp.contains("ERROR") => {
                    step_log.push(format!("OK: {}", step.label));
                }
                Ok(Ok(resp)) => {
                    step_log.push(format!(
                        "ERROR in {} (will retry after reboot): {}",
                        step.label,
                        resp.trim()
                    ));
                }
                Ok(Err(e)) => {
                    step_log.push(format!("Failed: {} (will retry after reboot) — {e}", step.label));
                }
                Err(_) => {
                    step_log.push(format!("Timeout: {} (will retry after reboot)", step.label));
                }
            }
        }

        // Auto target: enable AutoSel (AT+QMBNCFG="AutoSel",1).
        if to_auto {
            let autosel_cmd = mbn_cfg
                .commands
                .set_auto_select
                .as_deref()
                .map(|tpl| tpl.replace("{value}", "1"))
                .unwrap_or_else(|| "AT+QMBNCFG=\"AutoSel\",1".to_string());
            debug_trace_with_source(
                format!("[APN-APPLY] Enabling MBN AutoSel: {autosel_cmd}"),
                "apn",
            );
            match timeout(STATE_CHANGE_TIMEOUT, modem.execute_at(&autosel_cmd)).await {
                Ok(Ok(resp)) if !resp.contains("ERROR") => {
                    step_log.push("OK: Enable MBN AutoSel".into());
                }
                Ok(Ok(resp)) => {
                    step_log.push(format!("ERROR enabling MBN AutoSel: {}", resp.trim()));
                }
                Ok(Err(e)) => step_log.push(format!("Failed enabling MBN AutoSel: {e}")),
                Err(_) => step_log.push("Timeout enabling MBN AutoSel".into()),
            }
        }

        // Live write (QICSGP carries auth/user/pass — redacted label only).
        match timeout(QUICK_TIMEOUT, modem.execute_at(&live_write_cmd)).await {
            Ok(Ok(resp)) if !resp.contains("ERROR") => {
                debug_trace_with_source(format!("[APN-APPLY] {live_write_label}"), "apn");
                step_log.push(live_write_label);
            }
            Ok(Ok(resp)) => {
                step_log.push(format!(
                    "ERROR on live APN write (cid {}, will retry after reboot): {}",
                    params.cid,
                    resp.trim()
                ));
            }
            Ok(Err(e)) => {
                step_log.push(format!("Failed live APN write (cid {}) — {e}", params.cid));
            }
            Err(_) => {
                step_log.push(format!("Timeout on live APN write (cid {})", params.cid));
            }
        }

        // Reboot (AT+CFUN=1,1) — reuse apply_apn_profile's reboot path.
        if apply_cfg.pre_reboot_delay_ms > 0 {
            tokio::time::sleep(Duration::from_millis(apply_cfg.pre_reboot_delay_ms)).await;
        }
        debug_trace_with_source("[APN-APPLY] Rebooting modem (AT+CFUN=1,1)...", "apn");
        match timeout(STATE_CHANGE_TIMEOUT, modem.execute_at("AT+CFUN=1,1")).await {
            Ok(Ok(_)) => {
                rebooted = true;
                step_log.push("Modem reboot initiated".into());
                {
                    let modems = state.modems.read().await;
                    if let Some(context) = modems.get(modem_id) {
                        let mut health = context.health.write().await;
                        health.available = false;
                        health.state = ModemHealthState::Rebooting;
                        health.message = Some("Rebooting after APN/MBN apply".into());
                    }
                }
                state.broadcast_event(ModemEvent::ModemHealth(ModemHealth {
                    available: false,
                    state: ModemHealthState::Rebooting,
                    message: Some("Rebooting after APN/MBN apply".into()),
                }));
            }
            Ok(Err(e)) => step_log.push(format!("Reboot failed: {e}")),
            Err(_) => step_log.push("Reboot command timed out".into()),
        }
    }

    drop(modem);

    // Save connection config (memory + disk) so reconnect/watchdog enforce it.
    {
        let new_conn = ConnectionConfig {
            cid: params.cid,
            apn: params.apn.clone(),
            username: if params.username.is_empty() {
                None
            } else {
                Some(params.username.clone())
            },
            // Persist the resolved password so reconnect can re-supply it.
            password: if params.password.is_empty() {
                None
            } else {
                Some(params.password.clone())
            },
            auth_type: params.auth_type,
            ip_type: params.ip_type,
        };
        let modems = state.modems.read().await;
        if let Some(context) = modems.get(modem_id) {
            let mut modem_config = context.config.write().await;
            *modem_config = new_conn;
        }
    }

    let message = if rebooted {
        "Carrier profile changed — modem is rebooting (~30-60s offline).".to_string()
    } else {
        "Saved — click Reconnect to apply to the live link.".to_string()
    };

    let had_errors = step_log_has_errors(&step_log);

    Ok(ApnApplyResult {
        success: true,
        had_errors,
        mbn_changed,
        rebooted,
        step_log,
        message,
    })
}

// ============================================================================
// Signal History
// ============================================================================

/// Query parameters for signal history endpoint.
#[derive(Debug, Deserialize)]
pub struct SignalHistoryQuery {
    /// Time window: "1h", "6h", "24h". Default: "1h".
    #[serde(default = "default_signal_history_window")]
    pub window: String,
}

fn default_signal_history_window() -> String {
    "1h".to_string()
}

/// GET /api/modem/:modem_id/signal/history
///
/// Returns signal quality history samples within the requested time window.
/// Reads from the in-memory ring buffer — does not hit hardware.
pub async fn signal_history(
    Path(modem_id): Path<String>,
    Query(params): Query<SignalHistoryQuery>,
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<SignalHistory>> {
    let window_secs: i64 = match params.window.as_str() {
        "6h" => 6 * 3600,
        "24h" => 24 * 3600,
        _ => 3600, // default "1h"
    };

    let cutoff = chrono::Utc::now().timestamp() - window_secs;

    let modems = state.modems.read().await;
    let context = modems.get(&modem_id).ok_or_else(|| {
        ApiError::not_found(format!("Modem not found: {modem_id}"))
    })?;

    let history = context.signal_history.read().await;
    let samples: Vec<_> = history
        .iter()
        .filter(|s| s.ts >= cutoff)
        .cloned()
        .collect();

    Ok(Json(SignalHistory {
        modem_id,
        samples,
    }))
}

/// GET /api/modem/signal/history (backward-compat)
pub async fn signal_history_compat(
    query: Query<SignalHistoryQuery>,
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<SignalHistory>> {
    let modem_id = state
        .get_selected_or_first_modem()
        .await
        .ok_or_else(|| ApiError::not_found("No modems available"))?;
    signal_history(Path(modem_id), query, State(state)).await
}

// ============================================================================
// Backward-Compatibility Routes (Phase 1)
// ============================================================================
//
// These routes maintain compatibility with the old single-modem frontend API
// by operating on a "selected modem" instead of requiring explicit modem_id.
//
// The selected modem defaults to the first modem in the HashMap, and can be
// changed via POST /api/modem/select.

/// POST /api/modem/select
///
/// Select which modem the backward-compat routes should operate on.
/// Request: { "modem_id": "2c7c:0122:e3183572" } (stable modem ID string)
pub async fn select_modem_compat(
    State(state): State<Arc<AppState>>,
    Extension(session_user): Extension<SessionUser>,
    Json(req): Json<serde_json::Value>,
) -> ApiResult<Json<serde_json::Value>> {
    require_admin(&session_user)?;

    let modem_id = req
        .get("modem_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::bad_request("Missing modem_id field"))?;

    let modems = state.modems.read().await;
    if !modems.contains_key(modem_id) {
        return Err(ApiError::bad_request(format!("Unknown modem_id: {modem_id}")));
    }
    drop(modems);

    *state.selected_modem_id.write().await = Some(modem_id.to_string());

    Ok(Json(serde_json::json!({
        "success": true,
        "modem_id": modem_id
    })))
}

/// GET /api/modem/status (backward-compat)
pub async fn status_compat(State(state): State<Arc<AppState>>) -> ApiResult<Json<ModemStatus>> {
    let modem_id = state
        .get_selected_or_first_modem()
        .await
        .ok_or_else(|| ApiError::not_found("No modems available"))?;
    status(Path(modem_id), State(state)).await
}

/// GET /api/modem/signal (backward-compat)
pub async fn signal_compat(State(state): State<Arc<AppState>>) -> ApiResult<Json<SignalInfo>> {
    let modem_id = state
        .get_selected_or_first_modem()
        .await
        .ok_or_else(|| ApiError::not_found("No modems available"))?;
    signal(Path(modem_id), State(state)).await
}

/// GET /api/modem/info (backward-compat)
pub async fn info_compat(State(state): State<Arc<AppState>>) -> ApiResult<Json<DeviceInfo>> {
    let modem_id = state
        .get_selected_or_first_modem()
        .await
        .ok_or_else(|| ApiError::not_found("No modems available"))?;
    info(Path(modem_id), State(state)).await
}

/// GET /api/modem/gps (backward-compat)
pub async fn gps_compat(State(state): State<Arc<AppState>>) -> ApiResult<Json<GpsInfo>> {
    let modem_id = state
        .get_selected_or_first_modem()
        .await
        .ok_or_else(|| ApiError::not_found("No modems available"))?;
    gps(Path(modem_id), State(state)).await
}

/// GET /api/modem/pdp (backward-compat)
pub async fn pdp_details_compat(
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<serde_json::Value>> {
    let modem_id = state
        .get_selected_or_first_modem()
        .await
        .ok_or_else(|| ApiError::not_found("No modems available"))?;
    pdp_details(Path(modem_id), State(state)).await
}

/// POST /api/modem/connect (backward-compat)
pub async fn connect_compat(
    State(state): State<Arc<AppState>>,
    Extension(session_user): Extension<SessionUser>,
    Json(config): Json<ConnectionConfig>,
) -> ApiResult<Json<ModemStatus>> {
    let modem_id = state
        .get_selected_or_first_modem()
        .await
        .ok_or_else(|| ApiError::not_found("No modems available"))?;
    connect(Path(modem_id), State(state), Extension(session_user), Json(config)).await
}

/// POST /api/modem/disconnect (backward-compat)
pub async fn disconnect_compat(
    State(state): State<Arc<AppState>>,
    Extension(session_user): Extension<SessionUser>,
) -> ApiResult<Json<ModemStatus>> {
    let modem_id = state
        .get_selected_or_first_modem()
        .await
        .ok_or_else(|| ApiError::not_found("No modems available"))?;
    disconnect(Path(modem_id), State(state), Extension(session_user)).await
}

/// GET /api/sim/status (backward-compat)
pub async fn sim_status_compat(
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<SimStatus>> {
    let modem_id = state
        .get_selected_or_first_modem()
        .await
        .ok_or_else(|| ApiError::not_found("No modems available"))?;
    super::sim::status(Path(modem_id), State(state)).await
}

/// GET /api/config (backward-compat)
pub async fn get_config_compat(
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<ConnectionConfig>> {
    let modem_id = state
        .get_selected_or_first_modem()
        .await
        .ok_or_else(|| ApiError::not_found("No modems available"))?;
    super::config::get_config(Path(modem_id), State(state)).await
}

/// POST /api/modem/command (backward-compat)
///
/// AT command execution via selected/first modem. Fixes the HTML bug where
/// POST /api/modem/command fell through to the SPA fallback.
pub async fn command_compat(
    State(state): State<Arc<AppState>>,
    Extension(session_user): Extension<SessionUser>,
    Json(req): Json<AtCommandRequest>,
) -> ApiResult<Json<AtCommandResponse>> {
    let modem_id = state
        .get_selected_or_first_modem()
        .await
        .ok_or_else(|| ApiError::not_found("No modems available"))?;
    command(Path(modem_id), State(state), Extension(session_user), Json(req)).await
}

// ============================================================================
// On-demand refresh endpoints (bypass cache, hit hardware directly)
// ============================================================================

/// POST /api/modem/:modem_id/signal/refresh
///
/// Force-refresh signal metrics from hardware, update cache, return fresh data.
pub async fn signal_refresh(
    Path(modem_id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<SignalInfo>> {
    let handler_arc = require_modem_available(&state, &modem_id).await?;
    let modem = timeout(Duration::from_secs(2), handler_arc.lock())
        .await
        .map_err(|_| ApiError::service_unavailable_with_retry("Modem busy", 1))?;

    let signal = timeout(QUICK_TIMEOUT, modem.get_signal())
        .await
        .map_err(|_| ApiError::service_unavailable_with_retry("Request timed out", 1))?
        .map_err(ApiError::from)?;

    drop(modem);

    // Side-effect: update cache
    {
        let modems = state.modems.read().await;
        if let Some(context) = modems.get(&modem_id) {
            let mut cache = context.state_cache.write().await;
            if let Some(ref mut c) = *cache {
                c.signal = signal.clone();
                c.signal_strength =
                    ((signal.rssi + 113.0) * 100.0 / 62.0).clamp(0.0, 100.0) as i32;
                c.timestamp = chrono::Utc::now().to_rfc3339();
            }
            let mut ls = context.last_signal.write().await;
            *ls = Some(signal.clone());
        }
    }

    Ok(Json(signal))
}

/// POST /api/modem/:modem_id/status/refresh
///
/// Force-refresh modem status from hardware, update cache, return fresh data.
pub async fn status_refresh(
    Path(modem_id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<ModemStatus>> {
    let handler_arc = require_modem_available(&state, &modem_id).await?;
    let modem = timeout(Duration::from_secs(2), handler_arc.lock())
        .await
        .map_err(|_| ApiError::service_unavailable_with_retry("Modem busy", 1))?;

    let fresh = timeout(QUICK_TIMEOUT, modem.get_status())
        .await
        .map_err(|_| ApiError::service_unavailable_with_retry("Request timed out", 1))?
        .map_err(ApiError::from)?;

    drop(modem);

    // Side-effect: update cache connection fields
    {
        let modems = state.modems.read().await;
        if let Some(context) = modems.get(&modem_id) {
            let mut cache = context.state_cache.write().await;
            if let Some(ref mut c) = *cache {
                c.connection.connected = fresh.connected;
                c.connection.technology = fresh.technology;
                c.connection.operator = fresh.operator.clone();
                c.connection.ip_address = fresh.ip_address.clone();
                c.signal_strength = fresh.signal_strength;
                c.timestamp = chrono::Utc::now().to_rfc3339();
            }
        }
    }

    Ok(Json(fresh))
}

/// POST /api/modem/:modem_id/device/refresh
///
/// Force-refresh device info from hardware, update discovery cache, return fresh data.
pub async fn device_refresh(
    Path(modem_id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<DeviceInfo>> {
    let handler_arc = require_modem_available(&state, &modem_id).await?;
    let modem = timeout(Duration::from_secs(2), handler_arc.lock())
        .await
        .map_err(|_| ApiError::service_unavailable_with_retry("Modem busy", 1))?;

    let fresh = timeout(QUICK_TIMEOUT, modem.get_device_info())
        .await
        .map_err(|_| ApiError::service_unavailable_with_retry("Request timed out", 1))?
        .map_err(ApiError::from)?;

    drop(modem);

    // Update discovery cache
    {
        let modems = state.modems.read().await;
        if let Some(context) = modems.get(&modem_id) {
            let mut discovery = context.discovery.write().await;
            discovery.device_info = fresh.clone();
        }
    }

    Ok(Json(fresh))
}

/// POST /api/modem/:modem_id/sim/refresh
///
/// Force-refresh SIM status from hardware, update discovery cache, return fresh data.
pub async fn sim_refresh(
    Path(modem_id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<SimStatus>> {
    let handler_arc = require_modem_available(&state, &modem_id).await?;
    let modem = timeout(Duration::from_secs(2), handler_arc.lock())
        .await
        .map_err(|_| ApiError::service_unavailable_with_retry("Modem busy", 1))?;

    let fresh = timeout(QUICK_TIMEOUT, modem.get_sim_status())
        .await
        .map_err(|_| ApiError::service_unavailable_with_retry("Request timed out", 1))?
        .map_err(ApiError::from)?;

    drop(modem);

    // Update discovery cache
    {
        let modems = state.modems.read().await;
        if let Some(context) = modems.get(&modem_id) {
            let mut discovery = context.discovery.write().await;
            discovery.sim_status = fresh.clone();
        }
    }

    Ok(Json(fresh))
}

/// POST /api/modem/:modem_id/gps/refresh
///
/// Force-refresh GPS position from hardware, update cache, return fresh data.
pub async fn gps_refresh(
    Path(modem_id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<GpsInfo>> {
    let handler_arc = require_modem_available(&state, &modem_id).await?;
    let modem = timeout(Duration::from_secs(2), handler_arc.lock())
        .await
        .map_err(|_| ApiError::service_unavailable_with_retry("Modem busy", 1))?;

    let fresh = timeout(QUICK_TIMEOUT, modem.get_gps_position())
        .await
        .map_err(|_| ApiError::service_unavailable_with_retry("Request timed out", 1))?
        .map_err(ApiError::from)?;

    drop(modem);

    // Side-effect: update cache GPS field
    {
        let modems = state.modems.read().await;
        if let Some(context) = modems.get(&modem_id) {
            let mut cache = context.state_cache.write().await;
            if let Some(ref mut c) = *cache {
                c.gps = Some(fresh.clone());
                c.timestamp = chrono::Utc::now().to_rfc3339();
            }
        }
    }

    Ok(Json(fresh))
}

/// POST /api/modem/:modem_id/registration/refresh
///
/// Force-refresh network registration from hardware, update cache, return fresh data.
pub async fn registration_refresh(
    Path(modem_id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<RegistrationState>> {
    let handler_arc = require_modem_available(&state, &modem_id).await?;
    let modem = timeout(Duration::from_secs(2), handler_arc.lock())
        .await
        .map_err(|_| ApiError::service_unavailable_with_retry("Modem busy", 1))?;

    let fresh = timeout(QUICK_TIMEOUT, modem.get_registration())
        .await
        .map_err(|_| ApiError::service_unavailable_with_retry("Request timed out", 1))?
        .map_err(ApiError::from)?;

    drop(modem);

    // Side-effect: update cache registration field
    {
        let modems = state.modems.read().await;
        if let Some(context) = modems.get(&modem_id) {
            let mut cache = context.state_cache.write().await;
            if let Some(ref mut c) = *cache {
                c.registration = fresh.clone();
                c.timestamp = chrono::Utc::now().to_rfc3339();
            }
        }
    }

    Ok(Json(fresh))
}

// ============================================================================
// GPS panel gate
// ============================================================================

/// Request body for GPS panel gate.
#[derive(Debug, Deserialize)]
pub struct GpsPanelRequest {
    pub active: bool,
}

/// POST /api/gps/panel
///
/// Toggle GPS polling in the cache refresh task.
/// When active=false (default), GPS is not polled, saving AT bandwidth.
pub async fn gps_panel(
    State(state): State<Arc<AppState>>,
    Json(req): Json<GpsPanelRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    state
        .gps_panel_active
        .store(req.active, std::sync::atomic::Ordering::Relaxed);
    tracing::debug!("GPS panel active: {}", req.active);
    Ok(Json(serde_json::json!({
        "gps_panel_active": req.active
    })))
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // AT+WS46 mode parsing
    // =========================================================================

    #[test]
    fn parse_ws46_auto_mode() {
        let response = "+WS46: 37\r\nOK";
        assert_eq!(parse_ws46_value(response), Some("37".to_string()));
    }

    #[test]
    fn parse_ws46_lte_only() {
        let response = "+WS46: 28\r\nOK";
        assert_eq!(parse_ws46_value(response), Some("28".to_string()));
    }

    #[test]
    fn parse_ws46_lte_nr_no_sa() {
        let response = "+WS46: 36\r\nOK";
        assert_eq!(parse_ws46_value(response), Some("36".to_string()));
    }

    #[test]
    fn parse_ws46_no_match() {
        let response = "OK";
        assert_eq!(parse_ws46_value(response), None);
    }

    // =========================================================================
    // Telit AT#BND hex bitmask parsing
    // =========================================================================

    #[test]
    fn bitmask_pair_to_bands_single_band() {
        // Bit 0 = band 1 (LTE, high_base=49)
        assert_eq!(bitmask_pair_to_bands("1", "0", 49), vec![1]);
        // Bit 1 = band 2
        assert_eq!(bitmask_pair_to_bands("2", "0", 49), vec![2]);
        // Bit 6 = band 7
        assert_eq!(bitmask_pair_to_bands("40", "0", 49), vec![7]);
    }

    #[test]
    fn bitmask_pair_to_bands_high_field_lte() {
        // LTE high field bit 0 = band 49
        assert_eq!(bitmask_pair_to_bands("0", "1", 49), vec![49]);
        // LTE high field bit 1 = band 50
        assert_eq!(bitmask_pair_to_bands("0", "2", 49), vec![50]);
    }

    #[test]
    fn bitmask_pair_to_bands_high_field_nr() {
        // NR high field bit 0 = band 65
        assert_eq!(bitmask_pair_to_bands("0", "1", 65), vec![65]);
        // NR high field bit 1 = band 66
        assert_eq!(bitmask_pair_to_bands("0", "2", 65), vec![66]);
    }

    #[test]
    fn bitmask_pair_to_bands_combined() {
        // Bands 1 and 49 (LTE)
        assert_eq!(bitmask_pair_to_bands("1", "1", 49), vec![1, 49]);
        // Bands 1 and 65 (NR)
        assert_eq!(bitmask_pair_to_bands("1", "1", 65), vec![1, 65]);
    }

    #[test]
    fn bitmask_pair_to_bands_real_lte() {
        // Real Telit FN990 LTE bitmask from AT#BND=? (high_base=49)
        let bands = bitmask_pair_to_bands("A7E2BB0F38DF", "42", 49);
        assert_eq!(
            bands,
            vec![1, 2, 3, 4, 5, 7, 8, 12, 13, 14, 17, 18, 19, 20, 25, 26, 28,
                 29, 30, 32, 34, 38, 39, 40, 41, 42, 43, 46, 48, 50, 55]
        );
    }

    #[test]
    fn bitmask_pair_to_bands_real_nsa() {
        // Real Telit FN990 NSA bitmask from AT#BND=? (high_base=65)
        let bands = bitmask_pair_to_bands("1A0290828D7", "7042", 65);
        assert_eq!(
            bands,
            vec![1, 2, 3, 5, 7, 8, 12, 14, 20, 25, 28, 30, 38, 40, 41, 66, 71,
                 77, 78, 79]
        );
    }

    #[test]
    fn bitmask_pair_to_bands_real_sa() {
        // Real Telit FN990 SA bitmask from AT#BND=? (high_base=65)
        let bands = bitmask_pair_to_bands("81A03B0A38D7", "7C42", 65);
        assert_eq!(
            bands,
            vec![1, 2, 3, 5, 7, 8, 12, 13, 14, 18, 20, 25, 26, 28, 29, 30, 38,
                 40, 41, 48, 66, 71, 75, 76, 77, 78, 79]
        );
    }

    #[test]
    fn parse_telit_bnd_response_real() {
        let response = "#BND: 0,0,A7E2BB0E38DF,42,1A0290828D7,7042,81A03B0A38D7,7C42\r\nOK";
        let (lte, nsa, sa) = parse_telit_bnd_response(response);

        // LTE: note bit 16 (band 17) is 0 in current config (0E vs 0F)
        assert!(lte.contains(&1));
        assert!(lte.contains(&14));
        assert!(!lte.contains(&17)); // band 17 disabled in this response
        assert!(lte.contains(&41));

        // NSA
        assert!(nsa.contains(&1));
        assert!(nsa.contains(&77));
        assert!(nsa.contains(&79));
        assert_eq!(nsa.len(), 20);

        // SA
        assert!(sa.contains(&1));
        assert!(sa.contains(&48));
        assert!(sa.contains(&79));
        assert_eq!(sa.len(), 27);
    }

    #[test]
    fn parse_telit_bnd_response_no_match() {
        let (lte, nsa, sa) = parse_telit_bnd_response("OK");
        assert!(lte.is_empty());
        assert!(nsa.is_empty());
        assert!(sa.is_empty());
    }

    #[test]
    fn parse_telit_bnd_response_too_few_fields() {
        let (lte, nsa, sa) = parse_telit_bnd_response("#BND: 0,0,FF");
        assert!(lte.is_empty());
        assert!(nsa.is_empty());
        assert!(sa.is_empty());
    }

    // =========================================================================
    // Telit AT#BND hex bitmask formatting
    // =========================================================================

    #[test]
    fn bands_to_bitmask_pair_single_band_lte() {
        let (low, high) = bands_to_bitmask_pair(&[1], 49);
        assert_eq!(low, "1");
        assert_eq!(high, "0");
    }

    #[test]
    fn bands_to_bitmask_pair_band_49_lte() {
        let (low, high) = bands_to_bitmask_pair(&[49], 49);
        assert_eq!(low, "0");
        assert_eq!(high, "1");
    }

    #[test]
    fn bands_to_bitmask_pair_band_66_nr() {
        // NR band 66 -> high field bit 1 (66 - 65 = 1)
        let (low, high) = bands_to_bitmask_pair(&[66], 65);
        assert_eq!(low, "0");
        assert_eq!(high, "2");
    }

    #[test]
    fn bands_to_bitmask_pair_roundtrip_lte() {
        // Roundtrip: parse real bitmask, then format back
        let original_bands = bitmask_pair_to_bands("A7E2BB0F38DF", "42", 49);
        let (low, high) = bands_to_bitmask_pair(&original_bands, 49);
        assert_eq!(low, "A7E2BB0F38DF");
        assert_eq!(high, "42");
    }

    #[test]
    fn bands_to_bitmask_pair_roundtrip_nsa() {
        let original_bands = bitmask_pair_to_bands("1A0290828D7", "7042", 65);
        let (low, high) = bands_to_bitmask_pair(&original_bands, 65);
        assert_eq!(low, "1A0290828D7");
        assert_eq!(high, "7042");
    }

    #[test]
    fn bands_to_bitmask_pair_roundtrip_sa() {
        let original_bands = bitmask_pair_to_bands("81A03B0A38D7", "7C42", 65);
        let (low, high) = bands_to_bitmask_pair(&original_bands, 65);
        assert_eq!(low, "81A03B0A38D7");
        assert_eq!(high, "7C42");
    }

    // =========================================================================
    // Build AT#BND= command
    // =========================================================================

    #[test]
    fn build_telit_bnd_command_basic() {
        let template = "AT#BND=0,0,{lte_low},{lte_high},{nsa_low},{nsa_high},{sa_low},{sa_high}";
        // LTE bands 1,3 → low=5, high=0
        // NSA band 77 → low=0, high=bit(77-65)=bit12=0x1000
        // SA bands 1,77 → low=1, high=bit12=0x1000
        let cmd = build_telit_bnd_command(template, &[1, 3], &[77], &[1, 77]);
        assert_eq!(cmd, "AT#BND=0,0,5,0,0,1000,1,1000");
    }

    #[test]
    fn build_telit_bnd_command_single_band_per_type() {
        let template = "AT#BND=0,0,{lte_low},{lte_high},{nsa_low},{nsa_high},{sa_low},{sa_high}";
        let cmd = build_telit_bnd_command(template, &[1], &[1], &[1]);
        assert_eq!(cmd, "AT#BND=0,0,1,0,1,0,1,0");
    }

    #[test]
    fn build_telit_bnd_command_nr_band_66() {
        let template = "AT#BND=0,0,{lte_low},{lte_high},{nsa_low},{nsa_high},{sa_low},{sa_high}";
        // NR band 66 → high bit 1 (66-65=1) = 0x2
        let cmd = build_telit_bnd_command(template, &[1], &[66], &[66]);
        assert_eq!(cmd, "AT#BND=0,0,1,0,0,2,0,2");
    }

    // =========================================================================
    // Existing Quectel parsers (regression tests)
    // =========================================================================

    #[test]
    fn parse_qnwprefcfg_value_mode() {
        let resp = "+QNWPREFCFG: \"mode_pref\",AUTO\r\nOK";
        assert_eq!(parse_qnwprefcfg_value(resp), Some("AUTO".to_string()));
    }

    #[test]
    fn parse_band_list_quectel() {
        let resp = "+QNWPREFCFG: \"lte_band\",1:3:7:20\r\nOK";
        assert_eq!(parse_band_list(resp, ":"), vec![1, 3, 7, 20]);
    }

    #[test]
    fn format_band_list_sorted() {
        assert_eq!(format_band_list(&[20, 3, 1, 7], ":"), "1:3:7:20");
    }

    #[test]
    fn merge_band_lists_dedup_sorted() {
        let profile = vec![1, 3, 7];
        let active = vec![3, 5, 7, 20];
        assert_eq!(merge_band_lists(&profile, &active), vec![1, 3, 5, 7, 20]);
    }

    // =========================================================================
    // Signal history ring buffer
    // =========================================================================

    use crate::hardware::SignalSample;
    use std::collections::VecDeque;

    fn make_sample(ts: i64, rsrp: f32) -> SignalSample {
        SignalSample {
            ts,
            rsrp,
            rsrq: -10.0,
            sinr: 15.0,
        }
    }

    #[test]
    fn signal_history_ring_buffer_cap() {
        let mut buf: VecDeque<SignalSample> = VecDeque::with_capacity(1440);
        // Push 1441 samples
        for i in 0..1441 {
            if buf.len() >= 1440 {
                buf.pop_front();
            }
            buf.push_back(make_sample(i as i64, -85.0));
        }
        assert_eq!(buf.len(), 1440);
        // Oldest should be ts=1 (ts=0 was evicted)
        assert_eq!(buf.front().unwrap().ts, 1);
        // Newest should be ts=1440
        assert_eq!(buf.back().unwrap().ts, 1440);
    }

    #[test]
    fn signal_history_window_filtering() {
        let now = chrono::Utc::now().timestamp();
        let mut buf: VecDeque<SignalSample> = VecDeque::with_capacity(1440);

        // Add samples spanning 25 hours (one per minute = 1500 samples, capped at 1440)
        for i in 0..1500 {
            let ts = now - (25 * 3600) + (i * 60);
            if buf.len() >= 1440 {
                buf.pop_front();
            }
            buf.push_back(make_sample(ts, -80.0 - (i % 20) as f32));
        }

        // 1h window
        let cutoff_1h = now - 3600;
        let count_1h = buf.iter().filter(|s| s.ts >= cutoff_1h).count();
        // ~60 samples in 1 hour (one per minute)
        assert!(count_1h >= 59 && count_1h <= 61, "1h count: {count_1h}");

        // 6h window
        let cutoff_6h = now - 6 * 3600;
        let count_6h = buf.iter().filter(|s| s.ts >= cutoff_6h).count();
        assert!(count_6h >= 359 && count_6h <= 361, "6h count: {count_6h}");

        // 24h window
        let cutoff_24h = now - 24 * 3600;
        let count_24h = buf.iter().filter(|s| s.ts >= cutoff_24h).count();
        // All 1440 samples fit within 24h (oldest is ~25h - 1h = 24h ago, but
        // first 60 were evicted, so the oldest remaining is ~24h ago)
        assert!(count_24h >= 1430 && count_24h <= 1440, "24h count: {count_24h}");
    }

    // =========================================================================
    // AT+QICSGP parser — Task 3 (APN/PDP panel redesign, Item #42 Phase 2)
    // =========================================================================

    #[test]
    fn parse_qicsgp_ipv4v6_with_credentials() {
        // Bench sample from Phase 0 RM551E — context_type=3 (IPV4V6), non-empty user/pass.
        // ip_type is NOT in QicsgpParsed — it comes from ip_type_from_pdp_type(CGDCONT).
        let resp = "+QICSGP: 3,\"b2b.tmobile.com\",\"testuser\",\"secret\",1\r\n\r\nOK";
        let got = parse_qicsgp_response(resp).unwrap();
        assert_eq!(got.apn, "b2b.tmobile.com");
        assert_eq!(got.auth_type, "pap");
        assert_eq!(got.username, "testuser");
        assert!(got.has_password, "non-empty password field must set has_password=true");
    }

    #[test]
    fn parse_qicsgp_no_credentials() {
        // Phase 0 bench sample — context_type=3 (IPV4V6), empty user/pass, auth=0
        let resp = "+QICSGP: 3,\"b2b.tmobile.com\",\"\",\"\",0\r\n\r\nOK";
        let got = parse_qicsgp_response(resp).unwrap();
        assert_eq!(got.apn, "b2b.tmobile.com");
        assert_eq!(got.auth_type, "none");
        assert_eq!(got.username, "");
        assert!(!got.has_password, "empty password field must set has_password=false");
    }

    #[test]
    fn parse_qicsgp_ipv4_chap() {
        let resp = "+QICSGP: 1,\"fast.t-mobile.com\",\"user1\",\"pass1\",2\r\n\r\nOK";
        let got = parse_qicsgp_response(resp).unwrap();
        assert_eq!(got.auth_type, "chap");
        assert_eq!(got.username, "user1");
        assert!(got.has_password);
    }

    #[test]
    fn parse_qicsgp_pap_auth() {
        // PAP auth (1), non-empty password
        let resp = "+QICSGP: 2,\"test.apn\",\"myuser\",\"mypass\",1\r\n\r\nOK";
        let got = parse_qicsgp_response(resp).unwrap();
        assert_eq!(got.auth_type, "pap");
        assert_eq!(got.apn, "test.apn");
        assert!(got.has_password);
    }

    #[test]
    fn parse_qicsgp_no_prefix_line_returns_none() {
        // No +QICSGP: line → None (modem returned just OK or error)
        assert!(parse_qicsgp_response("OK").is_none());
        assert!(parse_qicsgp_response("ERROR").is_none());
    }

    #[test]
    fn parse_qicsgp_missing_fields_returns_none() {
        // Truncated — only context_type, missing apn/user/pass/auth
        let resp = "+QICSGP: 1,\"apn.only\"";
        assert!(parse_qicsgp_response(resp).is_none());
    }

    // =========================================================================
    // AT+CGACT? parser
    // =========================================================================

    #[test]
    fn parse_cgact_active_and_inactive() {
        let resp = "+CGACT: 1,1\r\n+CGACT: 2,0\r\n+CGACT: 3,1\r\n\r\nOK";
        let active = parse_cgact_response(resp);
        assert!(active.contains(&1));
        assert!(!active.contains(&2));
        assert!(active.contains(&3));
    }

    #[test]
    fn parse_cgact_all_inactive() {
        let resp = "+CGACT: 1,0\r\n+CGACT: 2,0\r\n\r\nOK";
        let active = parse_cgact_response(resp);
        assert!(active.is_empty());
    }

    #[test]
    fn parse_cgact_no_lines_returns_empty() {
        let active = parse_cgact_response("OK");
        assert!(active.is_empty());
    }

    #[test]
    fn parse_cgact_single_active() {
        let resp = "+CGACT: 1,1\r\nOK";
        let active = parse_cgact_response(resp);
        assert_eq!(active, vec![1u32]);
    }

    // =========================================================================
    // Default-editing-CID heuristic
    // =========================================================================

    #[test]
    fn default_cid_picks_lowest_non_reserved() {
        // CIDs 1=ims, 2=sos, 3=internet — heuristic should pick CID 3
        let contexts = vec![
            serde_json::json!({"cid": "1", "apn": "ims", "pdp_type": "IP"}),
            serde_json::json!({"cid": "2", "apn": "SOS", "pdp_type": "IP"}),
            serde_json::json!({"cid": "3", "apn": "b2b.tmobile.com", "pdp_type": "IPV4V6"}),
        ];
        assert_eq!(default_editing_cid(&contexts), Some(3));
    }

    #[test]
    fn default_cid_skips_ims_case_insensitive() {
        // IMS in upper, lower, mixed case — all reserved
        let contexts = vec![
            serde_json::json!({"cid": "1", "apn": "IMS", "pdp_type": "IP"}),
            serde_json::json!({"cid": "2", "apn": "Ims", "pdp_type": "IP"}),
            serde_json::json!({"cid": "3", "apn": "internet", "pdp_type": "IP"}),
        ];
        assert_eq!(default_editing_cid(&contexts), Some(3));
    }

    #[test]
    fn default_cid_all_reserved_returns_none() {
        let contexts = vec![
            serde_json::json!({"cid": "1", "apn": "ims", "pdp_type": "IP"}),
            serde_json::json!({"cid": "2", "apn": "sos", "pdp_type": "IP"}),
        ];
        assert_eq!(default_editing_cid(&contexts), None);
    }

    #[test]
    fn default_cid_empty_contexts_returns_none() {
        let contexts: Vec<serde_json::Value> = vec![];
        assert_eq!(default_editing_cid(&contexts), None);
    }

    #[test]
    fn default_cid_picks_lowest_of_multiple_non_reserved() {
        // CIDs 5 and 3 both non-reserved — should pick 3 (lowest)
        let contexts = vec![
            serde_json::json!({"cid": "1", "apn": "ims", "pdp_type": "IP"}),
            serde_json::json!({"cid": "5", "apn": "apn2", "pdp_type": "IP"}),
            serde_json::json!({"cid": "3", "apn": "apn1", "pdp_type": "IP"}),
        ];
        assert_eq!(default_editing_cid(&contexts), Some(3));
    }

    // =========================================================================
    // ip_type_from_pdp_type helper
    // =========================================================================

    #[test]
    fn ip_type_from_pdp_type_variants() {
        assert_eq!(ip_type_from_pdp_type("IP"), "ipv4");
        assert_eq!(ip_type_from_pdp_type("ip"), "ipv4");
        assert_eq!(ip_type_from_pdp_type("IPV6"), "ipv6");
        assert_eq!(ip_type_from_pdp_type("IPv6"), "ipv6");
        assert_eq!(ip_type_from_pdp_type("IPV4V6"), "ipv4v6");
        assert_eq!(ip_type_from_pdp_type("ipv4v6"), "ipv4v6");
        // Unknown → default to ipv4 (safe fallback)
        assert_eq!(ip_type_from_pdp_type("PPP"), "ipv4");
    }

    // =========================================================================
    // POST /reconnect handler — Task 4 (APN/PDP panel redesign, Item #42 Phase 2)
    // =========================================================================

    use crate::hardware::{
        AppConfig, AuthType, ConnectionConfig, DetectedModem, DiscoveryInfo, IpType, MockHardware,
        ModemProtocol,
    };
    use crate::hardware::profiles::ProfileRegistry;
    use crate::security::license::LicenseState;
    use crate::security::users::UserStore;

    async fn make_test_state_with_mock(modem_id: &str) -> Arc<AppState> {
        let config = AppConfig::default();
        let users = UserStore::load("/nonexistent/users.json").await;
        let registry = ProfileRegistry::load();
        let device_token = "test-device-token".to_string();
        let license_state = LicenseState::Unlicensed;

        let state = Arc::new(AppState::new(
            config,
            users,
            registry,
            device_token,
            Arc::new(crate::security::device_auth::DeviceAuth::ephemeral()),
            license_state,
        ));

        let mock = MockHardware::new();
        let profile = state.profile_registry.generic().clone();
        let detected = DetectedModem {
            device_path: "/dev/ttyUSB0".to_string(),
            protocol: ModemProtocol::At,
            description: "Test Mock Modem".to_string(),
            vendor_id: Some("0000".to_string()),
            product_id: Some("0000".to_string()),
            profile_id: None,
            has_profile: false,
            bus_port: None,
            all_ports: vec![],
        };
        let conn_config = ConnectionConfig {
            cid: 1,
            apn: "test.apn".to_string(),
            username: None,
            password: None,
            auth_type: AuthType::None,
            ip_type: IpType::Ipv4,
        };
        let discovery = DiscoveryInfo::default();

        state
            .add_modem(
                modem_id.to_string(),
                Box::new(mock),
                profile,
                detected,
                conn_config,
                discovery,
            )
            .await;

        state
    }

    #[tokio::test]
    async fn reconnect_returns_200_with_modem_status() {
        // TDD Step 1: This test must FAIL before the handler is implemented.
        let modem_id = "test:mock:reconnect01";
        let state = make_test_state_with_mock(modem_id).await;

        let result = reconnect(
            Path(modem_id.to_string()),
            State(state),
            Extension(test_operator()),
        )
        .await;

        let Json(status) = result.expect("reconnect handler must return Ok");
        // After reconnect the mock sets connected=true and ip_address=Some(...)
        assert!(status.connected, "status.connected must be true after reconnect");
        assert!(
            status.ip_address.is_some(),
            "status.ip_address must be Some after reconnect"
        );
    }

    #[tokio::test]
    async fn reconnect_returns_not_found_for_missing_modem() {
        // TDD Step 1: This test must FAIL before the handler is implemented.
        let state = make_test_state_with_mock("some:other:modem").await;

        let result = reconnect(
            Path("no:such:modem".to_string()),
            State(state),
            Extension(test_operator()),
        )
        .await;

        assert!(
            result.is_err(),
            "reconnect must return Err for unknown modem_id"
        );
        let err = result.unwrap_err();
        // get_modem_context returns ApiError::not_found when modem doesn't exist
        assert_eq!(
            err.status,
            axum::http::StatusCode::NOT_FOUND,
            "missing modem must yield 404"
        );
    }

    // =========================================================================
    // POST /apn/apply handler — Task 5 (APN/PDP panel redesign, Item #42 Phase 2)
    // =========================================================================

    /// Build an AppState whose single modem carries the real Quectel RM551E-GL
    /// profile (QICSGP live-write template + apn_apply_config MBN steps +
    /// mbn_config). The mock's scripted QMBNCFG responses give a current MBN
    /// state of AutoSel=0 (manual) / Selected="ROW_Commercial".
    async fn make_test_state_with_quectel(modem_id: &str) -> Arc<AppState> {
        let config = AppConfig::default();
        let users = UserStore::load("/nonexistent/users.json").await;
        let registry = ProfileRegistry::load();
        let device_token = "test-device-token".to_string();
        let license_state = LicenseState::Unlicensed;

        let state = Arc::new(AppState::new(
            config,
            users,
            registry,
            device_token,
            Arc::new(crate::security::device_auth::DeviceAuth::ephemeral()),
            license_state,
        ));

        let mock = MockHardware::new();
        // Quectel profile carries QICSGP write + apn_apply_config + mbn_config.
        let profile = state
            .profile_registry
            .find_by_id("quectel_rm551e_gl")
            .expect("quectel_rm551e_gl profile must exist")
            .clone();
        let detected = DetectedModem {
            device_path: "/dev/ttyUSB0".to_string(),
            protocol: ModemProtocol::At,
            description: "Test Quectel Mock".to_string(),
            vendor_id: Some("2c7c".to_string()),
            product_id: Some("0122".to_string()),
            profile_id: Some("quectel_rm551e_gl".to_string()),
            has_profile: true,
            bus_port: None,
            all_ports: vec![],
        };
        let conn_config = ConnectionConfig {
            cid: 1,
            apn: "test.apn".to_string(),
            username: None,
            password: None,
            auth_type: AuthType::None,
            ip_type: IpType::Ipv4,
        };
        let discovery = DiscoveryInfo::default();

        state
            .add_modem(
                modem_id.to_string(),
                Box::new(mock),
                profile,
                detected,
                conn_config,
                discovery,
            )
            .await;

        state
    }

    fn test_operator() -> SessionUser {
        SessionUser {
            username: "tester".to_string(),
            role: Role::Admin,
        }
    }

    // --- (a) APN-only change → live write, no reboot ------------------------

    #[tokio::test]
    async fn apn_apply_apn_only_change_writes_no_reboot() {
        let modem_id = "test:quectel:apply_a";
        let state = make_test_state_with_quectel(modem_id).await;

        // mbn_profile omitted (None) → MBN unchanged → live-write branch only.
        let req = ApnApplyRequest {
            cid: 1,
            apn: "newapn.example".to_string(),
            ip_type: IpType::Ipv4v6,
            auth_type: AuthType::Pap,
            username: Some("user1".to_string()),
            password: Some("secret".to_string()),
            mbn_profile: None,
        };

        let Json(result) = apn_apply(
            Path(modem_id.to_string()),
            State(state),
            Extension(test_operator()),
            Json(req),
        )
        .await
        .expect("apply (apn-only) must return Ok");

        assert!(result.success, "apn-only apply must succeed");
        assert!(!result.mbn_changed, "MBN must be unchanged for omitted mbn_profile");
        assert!(!result.rebooted, "apn-only apply must NOT reboot");
    }

    // --- (b) MBN change → apply steps + reboot ------------------------------

    #[tokio::test]
    async fn apn_apply_mbn_change_runs_steps_and_reboots() {
        let modem_id = "test:quectel:apply_b";
        let state = make_test_state_with_quectel(modem_id).await;

        // Current selected = "ROW_Commercial"; request a different profile.
        let req = ApnApplyRequest {
            cid: 1,
            apn: "test.apn".to_string(),
            ip_type: IpType::Ipv4,
            auth_type: AuthType::None,
            username: None,
            password: None,
            mbn_profile: Some(Some("Commercial-TMO".to_string())),
        };

        let Json(result) = apn_apply(
            Path(modem_id.to_string()),
            State(state),
            Extension(test_operator()),
            Json(req),
        )
        .await
        .expect("apply (mbn-change) must return Ok");

        assert!(result.mbn_changed, "switching to a different profile must set mbn_changed");
        assert!(result.rebooted, "MBN change must reboot the modem");
        // Step log must contain the MBN select step label.
        assert!(
            result.step_log.iter().any(|s| s.contains("Select MBN profile")),
            "MBN-change step_log must include the Select step: {:?}",
            result.step_log
        );
    }

    // --- (b2) MBN change to Auto (null) -------------------------------------

    #[tokio::test]
    async fn apn_apply_mbn_change_to_auto_reboots() {
        let modem_id = "test:quectel:apply_b2";
        let state = make_test_state_with_quectel(modem_id).await;

        // Current is manual (AutoSel=0); requesting Auto (null) is a change.
        let req = ApnApplyRequest {
            cid: 1,
            apn: "test.apn".to_string(),
            ip_type: IpType::Ipv4,
            auth_type: AuthType::None,
            username: None,
            password: None,
            mbn_profile: Some(None), // JSON null == Auto
        };

        let Json(result) = apn_apply(
            Path(modem_id.to_string()),
            State(state),
            Extension(test_operator()),
            Json(req),
        )
        .await
        .expect("apply (mbn->auto) must return Ok");

        assert!(result.mbn_changed, "manual->Auto must set mbn_changed");
        assert!(result.rebooted, "manual->Auto must reboot");
        assert!(
            result.step_log.iter().any(|s| s.contains("AutoSel")),
            "Auto path step_log must mention AutoSel: {:?}",
            result.step_log
        );
    }

    // --- (c) no-change request → idempotent success, no reboot --------------

    #[tokio::test]
    async fn apn_apply_no_change_is_idempotent_no_reboot() {
        let modem_id = "test:quectel:apply_c";
        let state = make_test_state_with_quectel(modem_id).await;

        // mbn_profile omitted == unchanged; APN write is same-value (harmless).
        let req = ApnApplyRequest {
            cid: 1,
            apn: "test.apn".to_string(),
            ip_type: IpType::Ipv4,
            auth_type: AuthType::None,
            username: None,
            password: None,
            mbn_profile: None,
        };

        let Json(result) = apn_apply(
            Path(modem_id.to_string()),
            State(state),
            Extension(test_operator()),
            Json(req),
        )
        .await
        .expect("idempotent no-op apply must return Ok");

        assert!(result.success, "idempotent apply must succeed");
        assert!(!result.mbn_changed);
        assert!(!result.rebooted, "no-op apply must NOT reboot");
    }

    // --- (c2) MBN profile==current selected → unchanged, no reboot ----------

    #[tokio::test]
    async fn apn_apply_same_mbn_profile_no_reboot() {
        let modem_id = "test:quectel:apply_c2";
        let state = make_test_state_with_quectel(modem_id).await;

        // Request the SAME profile currently selected ("ROW_Commercial").
        let req = ApnApplyRequest {
            cid: 1,
            apn: "test.apn".to_string(),
            ip_type: IpType::Ipv4,
            auth_type: AuthType::None,
            username: None,
            password: None,
            mbn_profile: Some(Some("ROW_Commercial".to_string())),
        };

        let Json(result) = apn_apply(
            Path(modem_id.to_string()),
            State(state),
            Extension(test_operator()),
            Json(req),
        )
        .await
        .expect("same-mbn apply must return Ok");

        assert!(!result.mbn_changed, "same selected profile must be unchanged");
        assert!(!result.rebooted, "same MBN must NOT reboot");
    }

    // --- (d) validation errors ----------------------------------------------

    #[tokio::test]
    async fn apn_apply_empty_apn_is_bad_request() {
        let modem_id = "test:quectel:apply_d";
        let state = make_test_state_with_quectel(modem_id).await;

        let req = ApnApplyRequest {
            cid: 1,
            apn: "".to_string(),
            ip_type: IpType::Ipv4,
            auth_type: AuthType::None,
            username: None,
            password: None,
            mbn_profile: None,
        };

        let result = apn_apply(
            Path(modem_id.to_string()),
            State(state),
            Extension(test_operator()),
            Json(req),
        )
        .await;

        let err = result.expect_err("empty APN must be rejected");
        assert_eq!(
            err.status,
            axum::http::StatusCode::BAD_REQUEST,
            "empty APN must yield 400"
        );
    }

    #[tokio::test]
    async fn apn_apply_apn_too_long_is_bad_request() {
        let modem_id = "test:quectel:apply_d2";
        let state = make_test_state_with_quectel(modem_id).await;

        let req = ApnApplyRequest {
            cid: 1,
            apn: "a".repeat(101),
            ip_type: IpType::Ipv4,
            auth_type: AuthType::None,
            username: None,
            password: None,
            mbn_profile: None,
        };

        let err = apn_apply(
            Path(modem_id.to_string()),
            State(state),
            Extension(test_operator()),
            Json(req),
        )
        .await
        .expect_err("APN >100 chars must be rejected");
        assert_eq!(err.status, axum::http::StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn apn_apply_bad_cid_is_bad_request() {
        let modem_id = "test:quectel:apply_d3";
        let state = make_test_state_with_quectel(modem_id).await;

        let req = ApnApplyRequest {
            cid: 9, // out of 1-8 range
            apn: "test.apn".to_string(),
            ip_type: IpType::Ipv4,
            auth_type: AuthType::None,
            username: None,
            password: None,
            mbn_profile: None,
        };

        let err = apn_apply(
            Path(modem_id.to_string()),
            State(state),
            Extension(test_operator()),
            Json(req),
        )
        .await
        .expect_err("cid out of range must be rejected");
        assert_eq!(err.status, axum::http::StatusCode::BAD_REQUEST);
    }

    // --- FIX 3: AT-injection front door (apn/username/password) ---------------

    #[test]
    fn validate_at_arg_rejects_quote_and_control_chars() {
        // Double-quote closes the quoted AT field.
        assert!(super::validate_at_arg("inj\"ect", "APN").is_err());
        // CR / LF terminate the AT line (multi-line injection).
        assert!(super::validate_at_arg("a\rb", "APN").is_err());
        assert!(super::validate_at_arg("a\nb", "APN").is_err());
        // Other ASCII control chars (e.g. NUL, ESC) also rejected.
        assert!(super::validate_at_arg("a\0b", "APN").is_err());
        assert!(super::validate_at_arg("a\x1bb", "APN").is_err());
        // Clean values pass.
        assert!(super::validate_at_arg("internet", "APN").is_ok());
        assert!(super::validate_at_arg("user.name+tag", "Username").is_ok());
        assert!(super::validate_at_arg("", "Password").is_ok());
    }

    #[tokio::test]
    async fn apn_apply_rejects_injection_in_apn() {
        let modem_id = "test:quectel:apply_inj_apn";
        let state = make_test_state_with_quectel(modem_id).await;

        for bad in ["bad\"apn", "apn\r\nAT+CFUN=0", "apn\nAT+QPOWD"] {
            let req = ApnApplyRequest {
                cid: 1,
                apn: bad.to_string(),
                ip_type: IpType::Ipv4,
                auth_type: AuthType::None,
                username: None,
                password: None,
                mbn_profile: None,
            };
            let err = apn_apply(
                Path(modem_id.to_string()),
                State(state.clone()),
                Extension(test_operator()),
                Json(req),
            )
            .await
            .expect_err(&format!("APN {bad:?} must be rejected"));
            assert_eq!(
                err.status,
                axum::http::StatusCode::BAD_REQUEST,
                "APN {bad:?} must yield 400",
            );
        }
    }

    #[tokio::test]
    async fn apn_apply_rejects_injection_in_username() {
        let modem_id = "test:quectel:apply_inj_user";
        let state = make_test_state_with_quectel(modem_id).await;

        for bad in ["us\"er", "user\rX", "user\nAT+CFUN=0"] {
            let req = ApnApplyRequest {
                cid: 1,
                apn: "internet".to_string(),
                ip_type: IpType::Ipv4,
                auth_type: AuthType::Pap,
                username: Some(bad.to_string()),
                password: Some("pw".to_string()),
                mbn_profile: None,
            };
            let err = apn_apply(
                Path(modem_id.to_string()),
                State(state.clone()),
                Extension(test_operator()),
                Json(req),
            )
            .await
            .expect_err(&format!("username {bad:?} must be rejected"));
            assert_eq!(err.status, axum::http::StatusCode::BAD_REQUEST);
        }
    }

    #[tokio::test]
    async fn apn_apply_rejects_injection_in_password() {
        let modem_id = "test:quectel:apply_inj_pw";
        let state = make_test_state_with_quectel(modem_id).await;

        for bad in ["p\"w", "pw\rX", "pw\nAT+QPOWD"] {
            let req = ApnApplyRequest {
                cid: 1,
                apn: "internet".to_string(),
                ip_type: IpType::Ipv4,
                auth_type: AuthType::Chap,
                username: Some("alice".to_string()),
                password: Some(bad.to_string()),
                mbn_profile: None,
            };
            let err = apn_apply(
                Path(modem_id.to_string()),
                State(state.clone()),
                Extension(test_operator()),
                Json(req),
            )
            .await
            .expect_err(&format!("password {bad:?} must be rejected"));
            assert_eq!(err.status, axum::http::StatusCode::BAD_REQUEST);
        }
    }

    // --- SECURITY: password must never appear in step_log nor message -------

    #[tokio::test]
    async fn apn_apply_password_never_in_log_or_message() {
        let modem_id = "test:quectel:apply_sec";
        let state = make_test_state_with_quectel(modem_id).await;

        const SECRET: &str = "SuperSecretPdpPassword123!";
        let req = ApnApplyRequest {
            cid: 1,
            apn: "secure.apn".to_string(),
            ip_type: IpType::Ipv4v6,
            auth_type: AuthType::Chap,
            username: Some("alice".to_string()),
            password: Some(SECRET.to_string()),
            mbn_profile: None, // live-write branch (where QICSGP write carries the password)
        };

        let Json(result) = apn_apply(
            Path(modem_id.to_string()),
            State(state),
            Extension(test_operator()),
            Json(req),
        )
        .await
        .expect("secure apply must return Ok");

        for line in &result.step_log {
            assert!(
                !line.contains(SECRET),
                "password leaked into step_log line: {line}"
            );
        }
        assert!(
            !result.message.contains(SECRET),
            "password leaked into result.message: {}",
            result.message
        );
    }

    // --- had_errors wiring test (handler-level) ------------------------------

    #[tokio::test]
    async fn apn_apply_clean_apply_reports_no_errors() {
        let modem_id = "test:quectel:apply_no_err";
        let state = make_test_state_with_quectel(modem_id).await;

        let req = ApnApplyRequest {
            cid: 1,
            apn: "clean.apn".to_string(),
            ip_type: IpType::Ipv4,
            auth_type: AuthType::None,
            username: None,
            password: None,
            mbn_profile: None,
        };

        let Json(result) = apn_apply(
            Path(modem_id.to_string()),
            State(state),
            Extension(test_operator()),
            Json(req),
        )
        .await
        .expect("clean apply must return Ok");

        assert!(result.success, "clean apply must succeed");
        assert!(
            !result.had_errors,
            "a clean apply must not flag had_errors: {:?}",
            result.step_log
        );
    }

    // --- step_log_has_errors helper (pure unit tests) -----------------------

    #[test]
    fn step_log_has_errors_detects_failure_markers() {
        // Real-world partial-failure line hit during the Phase 4 bench acceptance.
        assert!(step_log_has_errors(&[
            "OK: live APN write (QICSGP, cid 1)".to_string(),
            "ERROR in Set APN on PDP context (will retry after reboot)".to_string(),
        ]));
        assert!(step_log_has_errors(&[
            "Failed: Select MBN profile (will retry after reboot) — timeout".to_string()
        ]));
        assert!(step_log_has_errors(&["Timeout enabling MBN AutoSel".to_string()]));
    }

    #[test]
    fn step_log_has_errors_false_for_clean_log() {
        assert!(!step_log_has_errors(&[
            "OK: live APN write (QICSGP, cid 1)".to_string(),
            "OK: Select MBN profile".to_string(),
        ]));
        assert!(!step_log_has_errors(&[]));
    }

    #[test]
    fn step_log_has_errors_detects_reboot_failure_labels() {
        // Reboot-path labels use lowercase "failed" and "timed out" — these slipped
        // through the old case-sensitive ERROR/Failed/Timeout set.
        assert!(step_log_has_errors(&["Reboot failed: serial port closed".to_string()]));
        assert!(step_log_has_errors(&["Reboot command timed out".to_string()]));
    }

    // --- password preserve resolver (pure unit test) ------------------------

    #[test]
    fn resolve_password_uses_provided_value() {
        // When a password is provided, it is used verbatim.
        let resp = "+QICSGP: 3,\"apn\",\"u\",\"oldpass\",1\r\n\r\nOK";
        let resolved = resolve_apply_password(Some("newpass"), Some(resp));
        assert_eq!(resolved, "newpass");
    }

    #[test]
    fn resolve_password_preserves_existing_when_omitted() {
        // When omitted, re-read QICSGP and preserve the stored password.
        let resp = "+QICSGP: 3,\"apn\",\"u\",\"oldpass\",1\r\n\r\nOK";
        let resolved = resolve_apply_password(None, Some(resp));
        assert_eq!(resolved, "oldpass");
    }

    #[test]
    fn resolve_password_empty_when_omitted_and_no_current() {
        // Omitted + no parseable current config → empty (open APN).
        let resolved = resolve_apply_password(None, Some("OK"));
        assert_eq!(resolved, "");
        let resolved2 = resolve_apply_password(None, None);
        assert_eq!(resolved2, "");
    }

    #[test]
    fn resolve_password_empty_provided_clears_password() {
        // Explicitly provided empty string clears the password (not "omitted").
        let resp = "+QICSGP: 3,\"apn\",\"u\",\"oldpass\",1\r\n\r\nOK";
        let resolved = resolve_apply_password(Some(""), Some(resp));
        assert_eq!(resolved, "");
    }

    // --- update-profile password preserve resolver (pure unit test) ---------

    #[test]
    fn update_password_preserves_existing_when_omitted() {
        // Unedited (None) → keep the stored profile's existing password.
        let resolved = resolve_update_password(None, Some("Secret123"));
        assert_eq!(resolved, Some("Secret123".to_string()));
    }

    #[test]
    fn update_password_uses_provided_value() {
        // Provided value → use it verbatim, ignoring the existing one.
        let resolved = resolve_update_password(Some("newpass".to_string()), Some("Secret123"));
        assert_eq!(resolved, Some("newpass".to_string()));
    }

    #[test]
    fn update_password_explicit_empty_clears() {
        // Explicit "" (edited to clear) → store empty, do NOT preserve.
        let resolved = resolve_update_password(Some(String::new()), Some("Secret123"));
        assert_eq!(resolved, Some(String::new()));
    }

    #[test]
    fn update_password_omitted_with_no_existing_stays_none() {
        // Unedited and nothing stored → stays None (open APN).
        let resolved = resolve_update_password(None, None);
        assert_eq!(resolved, None);
    }

    #[test]
    fn parse_qicsgp_password_extracts_field() {
        let resp = "+QICSGP: 3,\"b2b.tmobile.com\",\"alice\",\"hunter2\",2\r\n\r\nOK";
        assert_eq!(parse_qicsgp_password(resp), Some("hunter2".to_string()));
    }

    #[test]
    fn parse_qicsgp_password_empty_field() {
        let resp = "+QICSGP: 3,\"open.apn\",\"\",\"\",0\r\n\r\nOK";
        assert_eq!(parse_qicsgp_password(resp), Some(String::new()));
    }

    #[test]
    fn parse_qicsgp_password_no_line_returns_none() {
        assert_eq!(parse_qicsgp_password("OK"), None);
    }

    // --- mbn diff helper (pure unit test) -----------------------------------

    #[test]
    fn mbn_diff_omitted_is_unchanged() {
        // Omitted (None) → never changed, target irrelevant.
        let (changed, _target) = compute_mbn_diff(&None, Some(false), Some("ROW_Commercial"));
        assert!(!changed);
    }

    #[test]
    fn mbn_diff_auto_unread_state_is_unchanged() {
        // Fail-safe (2026-06-17 spec): Auto requested but the current MBN state
        // could not be read (current_auto = None) → must NOT reboot.
        // This is the regression test for the intermittent-reboot bug.
        let (changed, _t) = compute_mbn_diff(&Some(None), None, None);
        assert!(!changed);
    }

    #[test]
    fn mbn_diff_auto_changed_when_currently_manual() {
        // Auto requested, currently manual (auto_select=false) → changed.
        let (changed, target) = compute_mbn_diff(&Some(None), Some(false), Some("ROW_Commercial"));
        assert!(changed);
        assert_eq!(target, MbnTarget::Auto);
    }

    #[test]
    fn mbn_diff_auto_unchanged_when_already_auto() {
        let (changed, _t) = compute_mbn_diff(&Some(None), Some(true), None);
        assert!(!changed);
    }

    #[test]
    fn mbn_diff_auto_sentinel_string() {
        // "__auto__" string is treated as Auto.
        let (changed, target) =
            compute_mbn_diff(&Some(Some("__auto__".to_string())), Some(false), Some("X"));
        assert!(changed);
        assert_eq!(target, MbnTarget::Auto);
    }

    #[test]
    fn mbn_diff_specific_changed_when_different() {
        let (changed, target) = compute_mbn_diff(
            &Some(Some("Commercial-TMO".to_string())),
            Some(false),
            Some("ROW_Commercial"),
        );
        assert!(changed);
        assert_eq!(target, MbnTarget::Profile("Commercial-TMO".to_string()));
    }

    #[test]
    fn mbn_diff_specific_unchanged_when_same_and_manual() {
        let (changed, _t) = compute_mbn_diff(
            &Some(Some("ROW_Commercial".to_string())),
            Some(false),
            Some("ROW_Commercial"),
        );
        assert!(!changed);
    }

    #[test]
    fn mbn_diff_specific_changed_when_currently_auto() {
        // Same name string but modem is in AutoSel — selecting a profile IS a change.
        let (changed, target) = compute_mbn_diff(
            &Some(Some("ROW_Commercial".to_string())),
            Some(true),
            Some("ROW_Commercial"),
        );
        assert!(changed);
        assert_eq!(target, MbnTarget::Profile("ROW_Commercial".to_string()));
    }

    // --- mbn diff fail-safe matrix (2026-06-17 spec) ------------------------

    #[test]
    fn mbn_diff_specific_unread_auto_is_unchanged() {
        // Specific profile requested but AutoSel could not be read → no reboot.
        let (changed, target) =
            compute_mbn_diff(&Some(Some("Commercial-TMO".to_string())), None, None);
        assert!(!changed);
        // Target is still the requested profile (placeholder when unchanged).
        assert_eq!(target, MbnTarget::Profile("Commercial-TMO".to_string()));
    }

    #[test]
    fn mbn_diff_specific_unread_selected_is_unchanged() {
        // AutoSel confirmed off, but the selected profile could not be read
        // (current_selected = None) → cannot confirm a difference → no reboot.
        let (changed, _t) =
            compute_mbn_diff(&Some(Some("Commercial-TMO".to_string())), Some(false), None);
        assert!(!changed);
    }

    #[test]
    fn mbn_diff_sentinel_unread_is_unchanged() {
        // "__auto__" with an unread current state → no reboot.
        let (changed, _t) =
            compute_mbn_diff(&Some(Some("__auto__".to_string())), None, None);
        assert!(!changed);
    }

    #[test]
    fn mbn_diff_sentinel_unchanged_when_already_auto() {
        let (changed, _t) =
            compute_mbn_diff(&Some(Some("__auto__".to_string())), Some(true), None);
        assert!(!changed);
    }

    // --- parse_mbn_auto_select hardening (2026-06-17 spec) ------------------

    #[test]
    fn parse_mbn_auto_select_two_field_on() {
        assert_eq!(
            parse_mbn_auto_select("+QMBNCFG: \"AutoSel\",1\r\n\r\nOK"),
            Some(true)
        );
    }

    #[test]
    fn parse_mbn_auto_select_two_field_off() {
        assert_eq!(
            parse_mbn_auto_select("+QMBNCFG: \"AutoSel\",0\r\n\r\nOK"),
            Some(false)
        );
    }

    #[test]
    fn parse_mbn_auto_select_three_field_on() {
        // Quectel 3-field variant must parse the flag (field 2), not the name.
        assert_eq!(
            parse_mbn_auto_select("+QMBNCFG: \"AutoSel\",1,\"Commercial-TMO\"\r\n\r\nOK"),
            Some(true)
        );
    }

    #[test]
    fn parse_mbn_auto_select_three_field_off() {
        assert_eq!(
            parse_mbn_auto_select("+QMBNCFG: \"AutoSel\",0,\"Commercial-TMO\"\r\n\r\nOK"),
            Some(false)
        );
    }

    #[test]
    fn parse_mbn_auto_select_missing_returns_none() {
        assert_eq!(parse_mbn_auto_select("OK"), None);
    }

    #[test]
    fn parse_mbn_auto_select_junk_flag_returns_none() {
        // Malformed flag value (not 0/1) → None.
        assert_eq!(
            parse_mbn_auto_select("+QMBNCFG: \"AutoSel\",x\r\n\r\nOK"),
            None
        );
    }

    // --- R4 parser hardening (2026-06-18 read-framing spec) -----------------

    #[test]
    fn parse_mbn_auto_select_with_interleaved_urcs_returns_some() {
        // R5(d): a +QMBNCFG AutoSel read arriving interleaved with a URC flood
        // (+CREG / +QIND / +CGEV) must still parse the AutoSel flag — the URC
        // lines must not collapse the parse to None (the dev.56 root regression).
        // The hardened parser locates the +QMBNCFG line structurally regardless
        // of surrounding URC noise.
        let resp = "AT+QMBNCFG=\"AutoSel\"\r\r\n\
                    +CREG: 1,\"1A2B\",\"00C1D2\",7\r\n\
                    +QIND: SMS DONE\r\n\
                    +QMBNCFG: \"AutoSel\",1\r\n\
                    +CGEV: ME PDN ACT 1\r\n\
                    OK\r\n";
        assert_eq!(parse_mbn_auto_select(resp), Some(true));
    }

    #[test]
    fn parse_mbn_auto_select_requires_autosel_token_not_substring_noise() {
        // Structural strictness: a +QMBNCFG line that is NOT the AutoSel query
        // (e.g. a List/Select line) must not be mis-read as an AutoSel flag.
        let resp = "+QMBNCFG: \"List\",0,1,1,\"ROW_Commercial\",\"...\"\r\nOK\r\n";
        assert_eq!(parse_mbn_auto_select(resp), None);
    }

    #[test]
    fn parse_mbn_auto_select_ignores_non_response_line_mentioning_autosel() {
        // Structural strictness (R4): only a line whose RESPONSE prefix is
        // +QMBNCFG: may supply the flag. A stray/URC line that merely mentions
        // the AutoSel token but is not a +QMBNCFG: response must be ignored, not
        // substring-matched into a (wrong) flag.
        let resp = "+QIND: cfg changed AutoSel,9\r\n+QMBNCFG: \"AutoSel\",1\r\nOK\r\n";
        // The real +QMBNCFG line says enabled → Some(true); the +QIND noise must
        // not pre-empt it with the bogus ",9" field.
        assert_eq!(parse_mbn_auto_select(resp), Some(true));
    }

    #[test]
    fn parse_mbn_selected_with_interleaved_urcs_returns_name() {
        // The Select query parsed cleanly despite interleaved URC noise.
        let resp = "AT+QMBNCFG=\"Select\"\r\r\n\
                    +QIND: SMS DONE\r\n\
                    +QMBNCFG: \"Select\",\"ROW_Commercial\"\r\n\
                    +CREG: 1\r\n\
                    OK\r\n";
        assert_eq!(
            parse_mbn_selected(resp),
            Some("ROW_Commercial".to_string())
        );
    }

    // =========================================================================
    // Custom-profile apply routes through diff-aware apply — Task 6
    // (APN/PDP panel redesign, Item #42 Phase 2)
    // =========================================================================

    /// Insert a saved ApnProfile directly into state and return its id.
    /// Uses the real create handler so validation + persistence match production.
    async fn seed_apn_profile(
        state: &Arc<AppState>,
        modem_id: &str,
        connection: ConnectionConfig,
        mbn_profile: Option<String>,
    ) -> String {
        let req = crate::hardware::ApnProfileRequest {
            name: "Test Saved Profile".to_string(),
            modem_profile_id: "quectel_rm551e_gl".to_string(),
            connection,
            mbn_profile,
        };
        let (_status, Json(profile)) = create_apn_profile(
            Path(modem_id.to_string()),
            State(Arc::clone(state)),
            Extension(test_operator()),
            Json(req),
        )
        .await
        .expect("create_apn_profile must succeed");
        profile.id
    }

    fn quectel_connection() -> ConnectionConfig {
        ConnectionConfig {
            cid: 1,
            apn: "saved.apn".to_string(),
            username: None,
            password: None,
            auth_type: AuthType::None,
            ip_type: IpType::Ipv4,
        }
    }

    // --- (i) saved MBN == current selected → no reboot ----------------------

    #[tokio::test]
    async fn apply_saved_profile_same_mbn_does_not_reboot() {
        let modem_id = "test:quectel:saved_i";
        let state = make_test_state_with_quectel(modem_id).await;

        // Mock current MBN: AutoSel=0 / Selected="ROW_Commercial".
        // A saved profile whose MBN matches the current selection must NOT reboot.
        let profile_id = seed_apn_profile(
            &state,
            modem_id,
            quectel_connection(),
            Some("ROW_Commercial".to_string()),
        )
        .await;

        let Json(result) = apply_apn_profile(
            Path(modem_id.to_string()),
            State(Arc::clone(&state)),
            Extension(test_operator()),
            Json(crate::hardware::ApnProfileApplyRequest { profile_id }),
        )
        .await
        .expect("apply (same mbn) must return Ok");

        assert!(result.success, "same-mbn apply must succeed");
        assert!(
            !result.reboot_triggered,
            "saved profile matching current MBN must NOT reboot: {:?}",
            result.step_log
        );
    }

    // --- (ii) saved MBN != current selected → MBN+reboot branch -------------

    #[tokio::test]
    async fn apply_saved_profile_different_mbn_reboots() {
        let modem_id = "test:quectel:saved_ii";
        let state = make_test_state_with_quectel(modem_id).await;

        // Current selected = "ROW_Commercial"; saved profile picks a different one.
        let profile_id = seed_apn_profile(
            &state,
            modem_id,
            quectel_connection(),
            Some("Commercial-TMO".to_string()),
        )
        .await;

        let Json(result) = apply_apn_profile(
            Path(modem_id.to_string()),
            State(Arc::clone(&state)),
            Extension(test_operator()),
            Json(crate::hardware::ApnProfileApplyRequest { profile_id }),
        )
        .await
        .expect("apply (different mbn) must return Ok");

        assert!(
            result.reboot_triggered,
            "saved profile with a different MBN must reboot: {:?}",
            result.step_log
        );
    }

    // --- (iii) saved MBN None (Auto) vs current manual → reboot -------------

    #[tokio::test]
    async fn apply_saved_profile_auto_mbn_reboots_when_currently_manual() {
        let modem_id = "test:quectel:saved_iii";
        let state = make_test_state_with_quectel(modem_id).await;

        // Saved profile mbn_profile=None means "Auto". Current modem is manual
        // (AutoSel=0), so applying Auto IS a change → reboot.
        let profile_id =
            seed_apn_profile(&state, modem_id, quectel_connection(), None).await;

        let Json(result) = apply_apn_profile(
            Path(modem_id.to_string()),
            State(Arc::clone(&state)),
            Extension(test_operator()),
            Json(crate::hardware::ApnProfileApplyRequest { profile_id }),
        )
        .await
        .expect("apply (auto mbn) must return Ok");

        assert!(
            result.reboot_triggered,
            "saved Auto profile against a manual modem must reboot: {:?}",
            result.step_log
        );
        assert!(
            result.step_log.iter().any(|s| s.contains("AutoSel")),
            "Auto path must enable AutoSel: {:?}",
            result.step_log
        );
    }

    // --- (iv) mbn_profile round-trips through create + update ---------------

    #[tokio::test]
    async fn apn_profile_mbn_round_trips_through_create_and_update() {
        let modem_id = "test:quectel:saved_iv";
        let state = make_test_state_with_quectel(modem_id).await;

        // Create with a specific MBN.
        let (status, Json(created)) = create_apn_profile(
            Path(modem_id.to_string()),
            State(Arc::clone(&state)),
            Extension(test_operator()),
            Json(crate::hardware::ApnProfileRequest {
                name: "RoundTrip".to_string(),
                modem_profile_id: "quectel_rm551e_gl".to_string(),
                connection: quectel_connection(),
                mbn_profile: Some("Commercial-TMO".to_string()),
            }),
        )
        .await
        .expect("create must succeed");
        assert_eq!(status, axum::http::StatusCode::CREATED);
        assert_eq!(
            created.mbn_profile.as_deref(),
            Some("Commercial-TMO"),
            "create must persist mbn_profile"
        );

        // Update the same profile to a different MBN.
        let Json(updated) = update_apn_profile(
            Path((modem_id.to_string(), created.id.clone())),
            State(Arc::clone(&state)),
            Extension(test_operator()),
            Json(crate::hardware::ApnProfileRequest {
                name: "RoundTrip".to_string(),
                modem_profile_id: "quectel_rm551e_gl".to_string(),
                connection: quectel_connection(),
                mbn_profile: Some("VoLTE-ATT".to_string()),
            }),
        )
        .await
        .expect("update must succeed");
        assert_eq!(
            updated.mbn_profile.as_deref(),
            Some("VoLTE-ATT"),
            "update must persist the new mbn_profile"
        );

        // Confirm it survives in the stored list.
        let stored = state.apn_profiles.read().await;
        let found = stored
            .iter()
            .find(|p| p.id == created.id)
            .expect("profile must remain in store");
        assert_eq!(found.mbn_profile.as_deref(), Some("VoLTE-ATT"));
    }

    // --- (v) unsupported-MBN modem still applies (live-write only) ----------

    #[tokio::test]
    async fn apply_saved_profile_unsupported_mbn_modem_live_writes_no_reboot() {
        // The generic profile reports mbn_config.supported = false and has no
        // QICSGP write template (CGDCONT fallback). The diff-aware path must
        // still apply (live write only) instead of returning 400.
        let modem_id = "test:mock:saved_v";
        let state = make_test_state_with_mock(modem_id).await;

        // Seed a profile tagged to this modem's profile id (generic).
        let req = crate::hardware::ApnProfileRequest {
            name: "GenericSaved".to_string(),
            modem_profile_id: "generic".to_string(),
            connection: ConnectionConfig {
                cid: 1,
                apn: "generic.apn".to_string(),
                username: None,
                password: None,
                auth_type: AuthType::None,
                ip_type: IpType::Ipv4,
            },
            mbn_profile: Some("ShouldBeIgnored".to_string()),
        };
        let (_status, Json(profile)) = create_apn_profile(
            Path(modem_id.to_string()),
            State(Arc::clone(&state)),
            Extension(test_operator()),
            Json(req),
        )
        .await
        .expect("create must succeed even for generic profile");

        let Json(result) = apply_apn_profile(
            Path(modem_id.to_string()),
            State(Arc::clone(&state)),
            Extension(test_operator()),
            Json(crate::hardware::ApnProfileApplyRequest {
                profile_id: profile.id,
            }),
        )
        .await
        .expect("apply on unsupported-MBN modem must return Ok (not 400)");

        assert!(result.success, "unsupported-MBN apply must succeed");
        assert!(
            !result.reboot_triggered,
            "unsupported-MBN modem must live-write only, no reboot: {:?}",
            result.step_log
        );
    }

    // =========================================================================
    // post_reconnect_health — F1 Layer 2 conditional watcher enlistment
    //
    // After a CFUN cycle re-enumerates the AT port, Layer 1 (handler self-heal)
    // tries to re-open the fd. If that fails, the post-reconnect AT probe still
    // errors with a fd-dead / unreachable class HardwareError. These unit tests
    // pin the pure classifier that decides whether to enlist the reconnect
    // watcher (mark Rebooting) or leave health untouched. Pure function — no
    // HTTP/mutex plumbing, runs in CI under default (mock) features.
    // =========================================================================

    #[test]
    fn post_reconnect_health_success_returns_none() {
        // No-regression guard: a normal reconnect keeps the modem AT-responsive,
        // so the probe succeeds → leave health untouched (do NOT mark Rebooting).
        let probe: Result<(), crate::hardware::HardwareError> = Ok(());
        assert!(
            post_reconnect_health(&probe).is_none(),
            "successful probe must not touch health"
        );
    }

    #[test]
    fn post_reconnect_health_io_error_marks_rebooting() {
        // Layer 1 could not self-heal the dead fd: the probe fails with an
        // I/O / timeout / device-gone class error → enlist the watcher by
        // marking Rebooting (available:false).
        for err in [
            crate::hardware::HardwareError::Io("Broken pipe (os error 32)".into()),
            crate::hardware::HardwareError::Timeout,
            crate::hardware::HardwareError::DeviceNotFound("ttyUSB2".into()),
        ] {
            let probe: Result<(), _> = Err(err.clone());
            let health = post_reconnect_health(&probe)
                .unwrap_or_else(|| panic!("{err:?} must enlist the watcher"));
            assert!(!health.available, "{err:?} → available must be false");
            assert_eq!(
                health.state,
                ModemHealthState::Rebooting,
                "{err:?} → state must be Rebooting"
            );
        }
    }

    #[test]
    fn post_reconnect_health_modem_answered_error_returns_none() {
        // The fd is alive — the modem answered but returned a logical error.
        // Do NOT enlist the watcher (that would falsely report unavailable 90s).
        for err in [
            crate::hardware::HardwareError::Protocol("unexpected response".into()),
            crate::hardware::HardwareError::NotReady("SIM busy".into()),
            crate::hardware::HardwareError::CommandRejected("AT+...: ERROR".into()),
        ] {
            let probe: Result<(), _> = Err(err.clone());
            assert!(
                post_reconnect_health(&probe).is_none(),
                "{err:?} (modem alive) must not touch health"
            );
        }
    }

    // =========================================================================
    // replace_handler — Arc-swap for wedged-mutex recovery (Task 2)
    // =========================================================================

    #[tokio::test]
    async fn replace_handler_swaps_arc_and_returns_false_for_unknown() {
        let modem_id = "test:quectel:replace_h";
        let state = make_test_state_with_quectel(modem_id).await;

        let old_arc = {
            state.modems.read().await.get(modem_id).unwrap().handler.clone()
        };
        let replaced = state
            .replace_handler(modem_id, Box::new(crate::hardware::mock::MockHardware::new()))
            .await;
        assert!(replaced, "replace_handler must return true for an existing modem");

        let new_arc = {
            state.modems.read().await.get(modem_id).unwrap().handler.clone()
        };
        assert!(
            !std::sync::Arc::ptr_eq(&old_arc, &new_arc),
            "the ENTIRE handler Arc must be replaced (not the Box inside it)"
        );

        // Old Arc clone is still independently lockable — proves the swap did not
        // touch it (the leak is isolated; pre-held clones are unaffected).
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(200), old_arc.lock())
                .await
                .is_ok(),
            "old Arc clone must remain lockable independently"
        );

        assert!(
            !state
                .replace_handler("nonexistent:0:0", Box::new(crate::hardware::mock::MockHardware::new()))
                .await,
            "unknown modem id must return false"
        );
    }

    // =========================================================================
    // live_device_path extraction + reconcile tests (Task 2, device-path reconcile)
    // =========================================================================

    #[tokio::test]
    async fn add_modem_with_mock_yields_no_live_device_path() {
        // The mock returns None from live_device_path_handle, so a mock-backed
        // context carries live_device_path: None and the reconcile is a no-op.
        let modem_id = "test:mock:ldp_none";
        let state = make_test_state_with_mock(modem_id).await;
        let modems = state.modems.read().await;
        let ctx = modems.get(modem_id).expect("modem must exist");
        assert!(
            ctx.live_device_path.is_none(),
            "mock-backed context must have live_device_path == None"
        );
    }

    #[tokio::test]
    async fn replace_handler_re_extracts_live_device_path() {
        // replace_handler must pull the handle from the NEW handler. With a mock
        // (None) before and after, the post-replace handle is still None (not a
        // stale Some from a prior handler). This pins the re-extraction call site.
        let modem_id = "test:mock:ldp_replace";
        let state = make_test_state_with_mock(modem_id).await;

        let replaced = state
            .replace_handler(modem_id, Box::new(crate::hardware::mock::MockHardware::new()))
            .await;
        assert!(replaced, "replace must succeed for an existing modem");

        let modems = state.modems.read().await;
        let ctx = modems.get(modem_id).expect("modem must exist");
        assert!(
            ctx.live_device_path.is_none(),
            "after replacing with a mock, live_device_path must be re-extracted to None"
        );
    }

    #[tokio::test]
    async fn cache_reconcile_applies_changed_device_path() {
        // This test exercises the REAL reconcile_modem_device_path method on
        // AppState (not a re-implementation). It constructs a mock-backed context,
        // overwrites its live_device_path cell with a different path (simulating
        // what reopen_port does on a real handler), seeds a matching detected_modems
        // entry, runs the method, and asserts both records updated.

        let modem_id = "test:mock:reconcile_apply";
        let state = make_test_state_with_mock(modem_id).await;
        // make_test_state_with_mock sets detected.device_path = "/dev/ttyUSB0"
        // and bus_port: None.

        // Pre-set a live cell to a DIFFERENT path (as if reopen_port healed onto it).
        let cell = std::sync::Arc::new(std::sync::Mutex::new("/dev/ttyUSB3".to_string()));
        {
            let mut modems = state.modems.write().await;
            let ctx = modems.get_mut(modem_id).expect("modem must exist");
            ctx.live_device_path = Some(std::sync::Arc::clone(&cell));
        }

        // Populate the global detected_modems Vec with a matching entry
        // (matched by bus_port; the helper uses bus_port: None, so seed None).
        {
            let mut detected = state.detected_modems.write().await;
            detected.push(DetectedModem {
                device_path: "/dev/ttyUSB0".to_string(),
                protocol: ModemProtocol::At,
                description: "reconcile test".to_string(),
                vendor_id: Some("0000".to_string()),
                product_id: Some("0000".to_string()),
                profile_id: None,
                has_profile: false,
                bus_port: None,
                all_ports: vec![],
            });
        }

        // Run the REAL reconcile method on AppState (not an inline re-implementation).
        let result = state.reconcile_modem_device_path(modem_id).await;
        assert_eq!(
            result.as_deref(),
            Some("/dev/ttyUSB3"),
            "reconcile must return Some(new_path) when cell differs"
        );

        // Assert ctx.detected.device_path updated.
        {
            let modems = state.modems.read().await;
            assert_eq!(
                modems.get(modem_id).unwrap().detected.device_path,
                "/dev/ttyUSB3",
                "ctx.detected.device_path must reflect the live cell"
            );
        }

        // Assert the matching detected_modems entry updated.
        {
            let detected = state.detected_modems.read().await;
            assert_eq!(
                detected[0].device_path, "/dev/ttyUSB3",
                "matching detected_modems entry must reflect the live cell"
            );
        }

        // Idempotent: running again must return None (no longer changed).
        let result2 = state.reconcile_modem_device_path(modem_id).await;
        assert!(
            result2.is_none(),
            "second reconcile with same cell must return None (idempotent)"
        );
    }

    // =========================================================================
    // Role gate (Fix 2) + raw AT gate / fail-closed (Fix 3) handler tests
    // =========================================================================

    fn readonly_user() -> SessionUser {
        SessionUser {
            username: "viewer".to_string(),
            role: Role::ReadOnly,
        }
    }

    #[tokio::test]
    async fn raw_at_command_forbidden_for_readonly() {
        let modem_id = "test:mock:atgate";
        let state = make_test_state_with_mock(modem_id).await;

        let req = AtCommandRequest {
            command: "AT+CSQ".to_string(),
            confirmed: true,
        };
        let res = command(
            Path(modem_id.to_string()),
            State(state),
            Extension(readonly_user()),
            Json(req),
        )
        .await;

        let err = res.expect_err("ReadOnly must be forbidden from raw AT");
        assert_eq!(err.status, axum::http::StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn raw_at_unknown_command_blocked_even_when_confirmed() {
        // Fail-closed: an unknown command must be rejected even with
        // confirmed:true, for an Admin user (gate passes, whitelist blocks).
        let modem_id = "test:mock:atunknown";
        let state = make_test_state_with_mock(modem_id).await;

        let req = AtCommandRequest {
            command: "AT+TOTALLYUNKNOWNCMD".to_string(),
            confirmed: true,
        };
        let res = command(
            Path(modem_id.to_string()),
            State(state),
            Extension(test_operator()), // Admin — passes the role gate
            Json(req),
        )
        .await;

        let err = res.expect_err("unknown command must be blocked");
        assert_eq!(
            err.status,
            axum::http::StatusCode::FORBIDDEN,
            "unknown AT command must be blocked (403), not allowed through with confirmed:true"
        );
    }

    // FIX 3: a successful raw-AT execution must leave an audit entry.
    #[tokio::test]
    async fn raw_at_success_writes_audit_entry() {
        let modem_id = "test:mock:ataudit";
        let state = make_test_state_with_mock(modem_id).await;

        // AT+CSQ is a standard safe query (passes the whitelist).
        let req = AtCommandRequest {
            command: "AT+CSQ".to_string(),
            confirmed: false,
        };
        let _ = command(
            Path(modem_id.to_string()),
            State(state.clone()),
            Extension(test_operator()),
            Json(req),
        )
        .await
        .expect("Admin raw AT (safe query) must succeed");

        let events = state.audit.recent(10).await;
        let found = events.iter().any(|e| {
            matches!(e.event_type, crate::security::audit::AuditEventType::AtCommand)
                && e.details.contains("AT+CSQ")
                && e.details.contains("tester")
        });
        assert!(
            found,
            "successful raw-AT must produce an AtCommand audit entry naming the user + command, got: {:?}",
            events
        );
    }

    // FIX 4: a modem reboot must leave an audit entry.
    #[tokio::test]
    async fn reboot_writes_audit_entry() {
        let modem_id = "test:mock:rebootaudit";
        let state = make_test_state_with_mock(modem_id).await;

        let _ = reboot(
            Path(modem_id.to_string()),
            State(state.clone()),
            Extension(test_operator()),
        )
        .await
        .expect("Admin reboot must return Ok");

        let events = state.audit.recent(10).await;
        let found = events.iter().any(|e| {
            e.details.contains("rebooted modem")
                && e.details.contains(modem_id)
                && e.details.contains("tester")
        });
        assert!(
            found,
            "reboot must produce an audit entry naming the user + modem, got: {:?}",
            events
        );
    }

    #[tokio::test]
    async fn modem_control_forbidden_for_readonly() {
        let modem_id = "test:mock:ctrlgate";

        // disconnect
        let state = make_test_state_with_mock(modem_id).await;
        let res = disconnect(
            Path(modem_id.to_string()),
            State(state),
            Extension(readonly_user()),
        )
        .await;
        assert_eq!(
            res.expect_err("ReadOnly disconnect must be forbidden").status,
            axum::http::StatusCode::FORBIDDEN,
        );

        // reboot
        let state = make_test_state_with_mock(modem_id).await;
        let res = reboot(
            Path(modem_id.to_string()),
            State(state),
            Extension(readonly_user()),
        )
        .await;
        assert_eq!(
            res.expect_err("ReadOnly reboot must be forbidden").status,
            axum::http::StatusCode::FORBIDDEN,
        );

        // airplane toggle
        let state = make_test_state_with_mock(modem_id).await;
        let res = airplane(
            Path(modem_id.to_string()),
            State(state),
            Extension(readonly_user()),
            Json(AirplaneModeRequest { enabled: true }),
        )
        .await;
        assert_eq!(
            res.expect_err("ReadOnly airplane must be forbidden").status,
            axum::http::StatusCode::FORBIDDEN,
        );
    }

    #[tokio::test]
    async fn select_compat_forbidden_for_readonly() {
        // POST /modem/select mutates the global selected_modem_id (default modem
        // for every compat route, for all clients) — a ReadOnly user must not.
        let modem_id = "test:mock:selectgate";
        let state = make_test_state_with_mock(modem_id).await;

        let res = select_modem_compat(
            State(state),
            Extension(readonly_user()),
            Json(serde_json::json!({ "modem_id": modem_id })),
        )
        .await;

        let err = res.expect_err("ReadOnly select must be forbidden");
        assert_eq!(err.status, axum::http::StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn select_compat_succeeds_for_admin() {
        let modem_id = "test:mock:selectok";
        let state = make_test_state_with_mock(modem_id).await;

        let Json(result) = select_modem_compat(
            State(state.clone()),
            Extension(test_operator()), // Admin — passes the role gate
            Json(serde_json::json!({ "modem_id": modem_id })),
        )
        .await
        .expect("Admin select must succeed");

        assert_eq!(result["success"], serde_json::json!(true));
        assert_eq!(result["modem_id"], serde_json::json!(modem_id));
        assert_eq!(
            state.selected_modem_id.read().await.as_deref(),
            Some(modem_id),
            "Admin select must update the global selected_modem_id"
        );
    }

    // =========================================================================
    // Public (pre-auth) signal shape — cell_id privacy split (2026-06-19)
    //
    // The public/unauthenticated login-screen route returns the reduced
    // `PublicSignalInfo` shape, which must NOT expose `cell_id` (a coarsely-
    // geolocatable serving-cell identifier). The authenticated routes
    // (`/modem/signal` compat, `/modem/:id/signal/extended`, WS `signal`)
    // still return the full `SignalInfo` INCLUDING `cell_id`. These tests lock
    // the split in both directions.
    // =========================================================================

    fn sample_signal_info() -> SignalInfo {
        SignalInfo {
            rssi: -65.0,
            rsrp: -85.0,
            rsrq: -10.0,
            sinr: 15.0,
            band: "B14".to_string(),
            cell_id: "SECRET_CELL_42".to_string(),
            technology: Some(crate::hardware::Technology::Gen4),
        }
    }

    #[test]
    fn public_signal_json_omits_cell_id() {
        // Reduced public shape must drop cell_id entirely.
        let public: PublicSignalInfo = PublicSignalInfo::from(&sample_signal_info());
        let json = serde_json::to_value(&public).expect("PublicSignalInfo must serialize");
        let obj = json.as_object().expect("must serialize to a JSON object");

        assert!(
            !obj.contains_key("cell_id"),
            "public signal JSON must NOT contain cell_id; got keys: {:?}",
            obj.keys().collect::<Vec<_>>()
        );
        // The other metrics must still be present.
        assert_eq!(obj.get("rssi"), Some(&serde_json::json!(-65.0)));
        assert_eq!(obj.get("rsrp"), Some(&serde_json::json!(-85.0)));
        assert_eq!(obj.get("rsrq"), Some(&serde_json::json!(-10.0)));
        assert_eq!(obj.get("sinr"), Some(&serde_json::json!(15.0)));
        assert_eq!(obj.get("band"), Some(&serde_json::json!("B14")));
        assert!(obj.contains_key("technology"), "technology must be present");
        // The secret value must not leak under any key.
        assert!(
            !json.to_string().contains("SECRET_CELL_42"),
            "public signal JSON must not leak the cell_id value anywhere"
        );
    }

    #[test]
    fn authenticated_signal_json_includes_cell_id() {
        // The full SignalInfo (authenticated compat / extended / WS) keeps cell_id.
        let full = sample_signal_info();
        let json = serde_json::to_value(&full).expect("SignalInfo must serialize");
        let obj = json.as_object().expect("must serialize to a JSON object");

        assert_eq!(
            obj.get("cell_id"),
            Some(&serde_json::json!("SECRET_CELL_42")),
            "authenticated SignalInfo JSON MUST contain cell_id"
        );
    }
}
