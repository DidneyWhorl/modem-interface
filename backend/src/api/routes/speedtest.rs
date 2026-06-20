//! Speedtest API route handlers.
//!
//! Provides endpoints to run embedded speed tests bound to a specific WAN
//! interface, check running status, and retrieve historical results.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    extract::{connect_info::ConnectInfo, Query, State},
    Json,
};
use axum_extra::extract::CookieJar;
use serde::{Deserialize, Serialize};

use crate::api::error::{ApiError, ApiResult};
use crate::api::routes::auth::SESSION_COOKIE;
use crate::hardware::types::{SpeedtestMode, SpeedtestResult};
use crate::state::AppState;

// ============================================================================
// Request / response types
// ============================================================================

#[derive(Debug, Deserialize)]
#[cfg_attr(not(feature = "tunnel"), allow(dead_code))]
pub struct RunSpeedtestRequest {
    pub mode: SpeedtestMode,
    pub wan_id: String,
}

#[derive(Debug, Serialize)]
pub struct RunSpeedtestResponse {
    pub test_id: String,
}

#[derive(Debug, Serialize)]
pub struct SpeedtestStatusResponse {
    pub running: bool,
}

#[derive(Debug, Serialize)]
pub struct HistoryResponse {
    pub results: Vec<SpeedtestResult>,
}

#[derive(Debug, Deserialize)]
pub struct HistoryQuery {
    pub wan_id: Option<String>,
    pub limit: Option<usize>,
}

// ============================================================================
// Helpers
// ============================================================================

/// Authorization decision for `POST /api/speedtest/run-sync`.
///
/// The route stays in the public group (so the portal-through-tunnel relay,
/// which carries NO router session, is not 401'd by `auth_middleware`), but the
/// handler self-gates: a speedtest burns cellular data, so it must not be
/// triggerable by an arbitrary unauthenticated LAN client.
///
/// Allow iff EITHER:
/// - the request source IP is loopback (`peer_is_loopback`) — the
///   portal-through-tunnel relay forwards to `127.0.0.1`, and on-device callers
///   are loopback too; a LAN client cannot forge a loopback source; OR
/// - the request carries a valid router session (`has_valid_session`).
///
/// Pure function — unit-tested below.
fn speedtest_access_allowed(peer_is_loopback: bool, has_valid_session: bool) -> bool {
    peer_is_loopback || has_valid_session
}

#[cfg(feature = "tunnel")]
/// Resolve a wan_id to the modem_id used in WAN config.
/// Portal uses IMEI as wan_id, but WAN config uses VID:PID:SERIAL.
/// Returns the original wan_id if no IMEI match is found.
async fn resolve_wan_id(state: &AppState, wan_id: &str) -> String {
    // First check if wan_id directly matches a WAN config entry
    {
        let wc = state.wan_config.read().await;
        if wc.modem_priority.iter().any(|e| e.modem_id == wan_id) {
            return wan_id.to_string();
        }
    }

    // If not, check if wan_id is an IMEI by searching modem contexts
    let modems = state.modems.read().await;
    for (modem_id, ctx) in modems.iter() {
        let discovery = ctx.discovery.read().await;
        if discovery.device_info.imei == wan_id {
            return modem_id.clone();
        }
    }

    // Return as-is (will fail the WAN lookup with a clear error)
    wan_id.to_string()
}

// ============================================================================
// Handlers
// ============================================================================

/// POST /api/speedtest/run
///
/// Start a speedtest on the given WAN interface. Returns immediately with
/// a test_id. Progress and completion are broadcast via WebSocket events
/// (SpeedtestProgress, SpeedtestComplete, SpeedtestError).
pub async fn run_speedtest(
    State(state): State<Arc<AppState>>,
    Json(req): Json<RunSpeedtestRequest>,
) -> ApiResult<Json<RunSpeedtestResponse>> {
    // Gate: speedtest engine requires the tunnel feature (which pulls in reqwest)
    #[cfg(not(feature = "tunnel"))]
    {
        let _ = (&state, &req);
        Err(ApiError::bad_request(
            "Speedtest requires real hardware (tunnel feature not enabled)",
        ))
    }

    #[cfg(feature = "tunnel")]
    {
        use crate::hardware::ModemEvent;
        use crate::state::BroadcastEvent;

        // Try to acquire the concurrency lock (non-blocking)
        let lock_guard = state.speedtest_lock.clone().try_lock_owned();
        let lock_guard = match lock_guard {
            Ok(guard) => guard,
            Err(_) => {
                return Err(ApiError::conflict("A speedtest is already running"));
            }
        };

        // Resolve wan_id to modem_id (portal sends IMEI, config uses VID:PID:SERIAL)
        let resolved_wan_id = resolve_wan_id(&state, &req.wan_id).await;

        // Look up interface from WAN config
        let (interface, wan_name) = {
            let wc = state.wan_config.read().await;
            let entry = wc
                .modem_priority
                .iter()
                .find(|e| e.modem_id == resolved_wan_id);
            match entry {
                Some(e) => (e.network_device.clone(), e.label.clone()),
                None => {
                    return Err(ApiError::not_found(format!(
                        "WAN entry not found: {}",
                        req.wan_id
                    )));
                }
            }
        };

        // Generate test_id up front so we can return it immediately
        let test_id = uuid::Uuid::new_v4().to_string();
        let response_test_id = test_id.clone();

        // Progress broadcast channel (engine -> WS clients)
        let (progress_tx, mut progress_rx) =
            tokio::sync::broadcast::channel::<crate::hardware::types::SpeedtestProgress>(64);

        // Clone what the spawned task needs
        let event_tx = state.event_tx.clone();
        let history = state.speedtest_history.clone();
        let buffer = state.speedtest_buffer.clone();
        let mode = req.mode;
        let wan_id = req.wan_id.clone();
        let wan_name_clone = wan_name;
        let interface_clone = interface;

        // Spawn a task that relays progress events to the WebSocket broadcast
        let relay_event_tx = event_tx.clone();
        let relay_handle = tokio::spawn(async move {
            while let Ok(progress) = progress_rx.recv().await {
                let _ = relay_event_tx.send(BroadcastEvent {
                    modem_id: None,
                    event: ModemEvent::SpeedtestProgress(progress),
                });
            }
        });

        // Spawn the actual speedtest task
        tokio::spawn(async move {
            // Hold the lock guard for the duration of the test
            let _guard = lock_guard;

            let result = crate::hardware::speedtest::run_speedtest(
                &interface_clone,
                mode,
                &wan_id,
                &wan_name_clone,
                progress_tx,
            )
            .await;

            // Stop the relay task
            relay_handle.abort();

            match result {
                Ok(result) => {
                    // Broadcast completion
                    let _ = event_tx.send(BroadcastEvent {
                        modem_id: None,
                        event: ModemEvent::SpeedtestComplete(Box::new(result.clone())),
                    });

                    // Save to history
                    {
                        let mut hist = history.write().await;
                        hist.push(result.clone());
                        if let Err(e) = crate::hardware::speedtest::save_history(&hist) {
                            tracing::warn!("Failed to save speedtest history: {e}");
                        }
                    }

                    // Buffer for heartbeat
                    {
                        let mut buf = buffer.write().await;
                        buf.push(result);
                    }
                }
                Err(error) => {
                    tracing::warn!("Speedtest failed: {error}");
                    let _ = event_tx.send(BroadcastEvent {
                        modem_id: None,
                        event: ModemEvent::SpeedtestError {
                            test_id,
                            error,
                        },
                    });
                }
            }
        });

        Ok(Json(RunSpeedtestResponse {
            test_id: response_test_id,
        }))
    }
}

/// POST /api/speedtest/run-sync
///
/// Start a speedtest and block until completion. Returns the full result.
/// Used by the portal tunnel proxy — no need for WebSocket streaming.
/// Timeout budget: Quick ~15s, Medium ~45s, Full ~60-90s. Caller should allow up to 90s.
pub async fn run_speedtest_sync(
    State(state): State<Arc<AppState>>,
    peer: Option<ConnectInfo<SocketAddr>>,
    jar: CookieJar,
    Json(req): Json<RunSpeedtestRequest>,
) -> ApiResult<Json<SpeedtestResult>> {
    // Authorization gate (gate (a)). This route is intentionally public so the
    // portal-through-tunnel relay (no router session) still works, but we must
    // not let an arbitrary LAN client burn cellular data: allow iff the source
    // IP is loopback (relay / on-device) OR a valid session is presented.
    let peer_is_loopback = peer.map(|ci| ci.0.ip().is_loopback()).unwrap_or(false);
    let has_valid_session = if peer_is_loopback {
        // Skip the session lookup entirely on the loopback fast path.
        false
    } else if let Some(cookie) = jar.get(SESSION_COOKIE) {
        state.sessions.validate(cookie.value()).await.is_some()
    } else {
        false
    };

    if !speedtest_access_allowed(peer_is_loopback, has_valid_session) {
        return Err(ApiError::unauthorized(
            "Speedtest requires a valid session or an on-device/tunnel origin",
        ));
    }

    #[cfg(not(feature = "tunnel"))]
    {
        let _ = (&state, &req);
        Err(ApiError::bad_request(
            "Speedtest requires real hardware (tunnel feature not enabled)",
        ))
    }

    #[cfg(feature = "tunnel")]
    {
        use std::time::Duration;
        use crate::hardware::ModemEvent;
        use crate::state::BroadcastEvent;

        // Generate a unique test_id for WS correlation (matches async handler pattern)
        let test_id = uuid::Uuid::new_v4().to_string();

        // Try to acquire the concurrency lock (non-blocking)
        let lock_guard = state.speedtest_lock.clone().try_lock_owned();
        let lock_guard = match lock_guard {
            Ok(guard) => guard,
            Err(_) => {
                return Err(ApiError::conflict("A speedtest is already running"));
            }
        };

        // Resolve wan_id to modem_id (portal sends IMEI, config uses VID:PID:SERIAL)
        let resolved_wan_id = resolve_wan_id(&state, &req.wan_id).await;

        // Look up interface from WAN config
        let (interface, wan_name) = {
            let wc = state.wan_config.read().await;
            let entry = wc
                .modem_priority
                .iter()
                .find(|e| e.modem_id == resolved_wan_id);
            match entry {
                Some(e) => (e.network_device.clone(), e.label.clone()),
                None => {
                    return Err(ApiError::not_found(format!(
                        "WAN entry not found: {}",
                        req.wan_id
                    )));
                }
            }
        };

        // Progress channel — still needed by the engine API, but we don't relay it.
        // Local WS clients will still see progress via the broadcast relay below.
        let (progress_tx, mut progress_rx) =
            tokio::sync::broadcast::channel::<crate::hardware::types::SpeedtestProgress>(64);

        let event_tx = state.event_tx.clone();

        // Relay progress to local WS clients (so the local UI still works if open)
        let relay_event_tx = event_tx.clone();
        let relay_handle = tokio::spawn(async move {
            while let Ok(progress) = progress_rx.recv().await {
                let _ = relay_event_tx.send(BroadcastEvent {
                    modem_id: None,
                    event: ModemEvent::SpeedtestProgress(progress),
                });
            }
        });

        // Run the test synchronously (await completion) with a hard timeout.
        // Quick mode is ~15s, Full mode is ~45s; 90s gives ample headroom.
        let run_future = crate::hardware::speedtest::run_speedtest(
            &interface,
            req.mode,
            &req.wan_id,
            &wan_name,
            progress_tx,
        );
        let timed = tokio::time::timeout(Duration::from_secs(90), run_future).await;

        // Stop the relay
        relay_handle.abort();

        // Release the lock
        drop(lock_guard);

        // Unwrap the timeout layer, then handle the inner result
        let result = match timed {
            Ok(inner) => inner,
            Err(_elapsed) => {
                tracing::warn!("Speedtest timed out after 90s (test_id={test_id})");
                let error = "speedtest timed out after 90 seconds".to_string();
                let _ = event_tx.send(BroadcastEvent {
                    modem_id: None,
                    event: ModemEvent::SpeedtestError {
                        test_id: test_id.clone(),
                        error: error.clone(),
                    },
                });
                return Err(ApiError::internal(format!("Speedtest failed: {error}")));
            }
        };

        match result {
            Ok(result) => {
                // Broadcast completion to local WS clients
                let _ = event_tx.send(BroadcastEvent {
                    modem_id: None,
                    event: ModemEvent::SpeedtestComplete(Box::new(result.clone())),
                });

                // Save to history
                {
                    let mut hist = state.speedtest_history.write().await;
                    hist.push(result.clone());
                    if let Err(e) = crate::hardware::speedtest::save_history(&hist) {
                        tracing::warn!("Failed to save speedtest history: {e}");
                    }
                }

                // Buffer for heartbeat
                {
                    let mut buf = state.speedtest_buffer.write().await;
                    buf.push(result.clone());
                }

                Ok(Json(result))
            }
            Err(error) => {
                tracing::warn!("Speedtest failed: {error}");
                let _ = event_tx.send(BroadcastEvent {
                    modem_id: None,
                    event: ModemEvent::SpeedtestError {
                        test_id,
                        error: error.clone(),
                    },
                });
                Err(ApiError::internal(format!("Speedtest failed: {error}")))
            }
        }
    }
}

/// GET /api/speedtest/status
///
/// Returns whether a speedtest is currently running.
pub async fn get_status(
    State(state): State<Arc<AppState>>,
) -> Json<SpeedtestStatusResponse> {
    let running = state.speedtest_lock.try_lock().is_err();
    Json(SpeedtestStatusResponse { running })
}

/// GET /api/speedtest/history?wan_id=X&limit=N
///
/// Returns historical speedtest results, optionally filtered by wan_id
/// and limited to the most recent N entries.
pub async fn get_history(
    State(state): State<Arc<AppState>>,
    Query(query): Query<HistoryQuery>,
) -> Json<HistoryResponse> {
    let hist = state.speedtest_history.read().await;
    let limit = query.limit.unwrap_or(50);

    let results: Vec<SpeedtestResult> = hist
        .results
        .iter()
        .filter(|r| {
            query
                .wan_id
                .as_ref()
                .is_none_or(|wid| r.wan_id == *wid)
        })
        .rev() // most recent first
        .take(limit)
        .cloned()
        .collect();

    Json(HistoryResponse { results })
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::profiles::ProfileRegistry;
    use crate::hardware::AppConfig;
    use crate::security::license::LicenseState;
    use crate::security::users::{Role, UserStore};
    use axum::body::Body;
    use axum::extract::connect_info::ConnectInfo;
    use axum::http::{Request as HttpRequest, StatusCode as HttpStatusCode};
    use axum::routing::post;
    use axum::Router;
    use std::net::SocketAddr;
    use tower::ServiceExt; // for `oneshot`

    // --- Pure helper: loopback-or-valid-session → allow; else deny. ---

    #[test]
    fn helper_loopback_no_session_allows() {
        assert!(speedtest_access_allowed(true, false));
    }

    #[test]
    fn helper_session_no_loopback_allows() {
        assert!(speedtest_access_allowed(false, true));
    }

    #[test]
    fn helper_loopback_and_session_allows() {
        assert!(speedtest_access_allowed(true, true));
    }

    #[test]
    fn helper_no_loopback_no_session_denies() {
        assert!(!speedtest_access_allowed(false, false));
    }

    // --- Request-level: the gate runs before the feature split, so these are
    // deterministic regardless of the `tunnel` feature. ---

    async fn build_state() -> Arc<AppState> {
        let users = UserStore::load("/nonexistent/users.json").await;
        let registry = ProfileRegistry::load();
        Arc::new(AppState::new(
            AppConfig::default(),
            users,
            registry,
            "test-device-token".to_string(),
            Arc::new(crate::security::device_auth::DeviceAuth::ephemeral()),
            LicenseState::Unlicensed,
        ))
    }

    fn app(state: Arc<AppState>) -> Router {
        Router::new()
            .route("/speedtest/run-sync", post(run_speedtest_sync))
            .with_state(state)
    }

    fn run_sync_request() -> HttpRequest<Body> {
        HttpRequest::builder()
            .method("POST")
            .uri("/speedtest/run-sync")
            .header("content-type", "application/json")
            .body(Body::from(
                r#"{"mode":"quick","wan_id":"wan0"}"#.to_string(),
            ))
            .unwrap()
    }

    /// Non-loopback peer, no session → rejected with 401.
    #[tokio::test]
    async fn run_sync_non_loopback_no_session_rejected() {
        let state = build_state().await;

        let mut request = run_sync_request();
        let lan: SocketAddr = "192.168.1.50:44444".parse().unwrap();
        request.extensions_mut().insert(ConnectInfo(lan));

        let response = app(state).oneshot(request).await.unwrap();
        assert_eq!(response.status(), HttpStatusCode::UNAUTHORIZED);
    }

    /// No ConnectInfo wired at all (peer unknown) + no session → rejected 401.
    #[tokio::test]
    async fn run_sync_no_peer_no_session_rejected() {
        let state = build_state().await;

        let response = app(state).oneshot(run_sync_request()).await.unwrap();
        assert_eq!(response.status(), HttpStatusCode::UNAUTHORIZED);
    }

    /// Loopback peer, no session → passes the auth gate (must NOT be 401).
    #[tokio::test]
    async fn run_sync_loopback_no_session_passes_gate() {
        let state = build_state().await;

        let mut request = run_sync_request();
        let loopback: SocketAddr = "127.0.0.1:54321".parse().unwrap();
        request.extensions_mut().insert(ConnectInfo(loopback));

        let response = app(state).oneshot(request).await.unwrap();
        assert_ne!(
            response.status(),
            HttpStatusCode::UNAUTHORIZED,
            "loopback peer must pass the speedtest auth gate"
        );
    }

    /// Non-loopback peer + valid session → passes the auth gate (must NOT be 401).
    #[tokio::test]
    async fn run_sync_non_loopback_valid_session_passes_gate() {
        let state = build_state().await;
        let token = state
            .sessions
            .create("root".to_string(), Role::SuperAdmin)
            .await;

        let mut request = HttpRequest::builder()
            .method("POST")
            .uri("/speedtest/run-sync")
            .header("content-type", "application/json")
            .header("cookie", format!("{SESSION_COOKIE}={token}"))
            .body(Body::from(
                r#"{"mode":"quick","wan_id":"wan0"}"#.to_string(),
            ))
            .unwrap();
        let lan: SocketAddr = "192.168.1.50:44444".parse().unwrap();
        request.extensions_mut().insert(ConnectInfo(lan));

        let response = app(state).oneshot(request).await.unwrap();
        assert_ne!(
            response.status(),
            HttpStatusCode::UNAUTHORIZED,
            "valid session must pass the speedtest auth gate"
        );
    }

    /// Non-loopback peer + invalid/bogus session cookie → rejected 401.
    #[tokio::test]
    async fn run_sync_non_loopback_invalid_session_rejected() {
        let state = build_state().await;

        let mut request = HttpRequest::builder()
            .method("POST")
            .uri("/speedtest/run-sync")
            .header("content-type", "application/json")
            .header("cookie", format!("{SESSION_COOKIE}=not-a-real-token"))
            .body(Body::from(
                r#"{"mode":"quick","wan_id":"wan0"}"#.to_string(),
            ))
            .unwrap();
        let lan: SocketAddr = "192.168.1.50:44444".parse().unwrap();
        request.extensions_mut().insert(ConnectInfo(lan));

        let response = app(state).oneshot(request).await.unwrap();
        assert_eq!(response.status(), HttpStatusCode::UNAUTHORIZED);
    }
}
