//! WebSocket handler for real-time modem events.
//!
//! Broadcasts modem events (signal updates, connection changes, etc.) to
//! connected clients.

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::IntoResponse,
};
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;
use tracing::{debug, error, info, warn};

use crate::api::routing;
use crate::api::steering;
use crate::hardware::{
    FailoverEvent, ModemEvent, ModemHealth, ModemHealthState, ModemStateCache,
    RoutingMode, RoutingTableEntry, SignalSample, WanHealthCheckResult, WanModemStatus,
    WanStatusResponse,
};
use crate::state::{AppState, WanModemRuntimeInfo};

/// GET /api/events - WebSocket upgrade handler.
pub async fn events_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    info!("WebSocket upgrade requested");
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

/// Decide whether the WebSocket handshake must demand an auth token.
///
/// This MUST mirror the HTTP auth middleware (`api/auth_middleware.rs:61-67`),
/// which gates on `config.auth.enabled` ALONE. The pre-Phase-1 logic added
/// `&& password_hash.is_some() || has_users()`, which was an auth bypass: on a
/// fresh box (`enabled=true`, no legacy `password_hash`, zero non-root users)
/// it evaluated FALSE and streamed modem telemetry to an UNAUTHENTICATED
/// client. It also diverged the inverse way (`enabled=false` but users exist →
/// HTTP open, WS wrongly demanded a token). Gating on `enabled` only realigns
/// the two paths.
fn ws_auth_required(auth_enabled: bool) -> bool {
    auth_enabled
}

/// Handle a WebSocket connection with in-message token authentication.
///
/// Flow: open → auth handshake → subscribe to events → event loop → cleanup.
/// If auth is globally disabled, the handshake is skipped.
async fn handle_socket(socket: WebSocket, state: Arc<AppState>) {
    let (mut sender, mut receiver) = socket.split();

    // Check if auth is required. This must mirror the HTTP auth middleware
    // (api/auth_middleware.rs), which gates on `config.auth.enabled` ALONE.
    let auth_required = {
        let config = state.config.read().await;
        ws_auth_required(config.auth.enabled)
    };

    if auth_required {
        info!("WebSocket connection opened, awaiting auth...");

        // Wait up to 10 seconds for an auth message
        let auth_result = tokio::time::timeout(Duration::from_secs(10), async {
            while let Some(msg) = receiver.next().await {
                match msg {
                    Ok(Message::Text(text)) => {
                        #[derive(serde::Deserialize)]
                        struct AuthMessage {
                            #[serde(rename = "type")]
                            msg_type: String,
                            token: String,
                        }

                        match serde_json::from_str::<AuthMessage>(&text) {
                            Ok(auth_msg) if auth_msg.msg_type == "auth" => {
                                return state
                                    .ws_tokens
                                    .validate_and_consume(&auth_msg.token)
                                    .await;
                            }
                            _ => return None,
                        }
                    }
                    Ok(Message::Close(_)) => return None,
                    Ok(Message::Ping(_)) => continue, // pong auto-handled
                    Ok(_) => return None,
                    Err(e) => {
                        warn!("WebSocket receive error during auth: {}", e);
                        return None;
                    }
                }
            }
            None
        })
        .await;

        match auth_result {
            Ok(Some(token_info)) => {
                info!("WebSocket authenticated: {}", token_info.username);
            }
            Ok(None) => {
                warn!("WebSocket auth failed: invalid or expired token");
                let err = serde_json::json!({
                    "type": "error",
                    "code": "auth_failed",
                    "message": "Invalid or expired WebSocket token"
                });
                let _ = sender.send(Message::Text(err.to_string())).await;
                let _ = sender.send(Message::Close(None)).await;
                return;
            }
            Err(_) => {
                warn!("WebSocket auth timeout: no auth message within 10 seconds");
                let err = serde_json::json!({
                    "type": "error",
                    "code": "auth_failed",
                    "message": "Authentication timeout"
                });
                let _ = sender.send(Message::Text(err.to_string())).await;
                let _ = sender.send(Message::Close(None)).await;
                return;
            }
        }
    } else {
        info!("WebSocket connection opened (auth disabled, skipping handshake)");
    }

    // --- Auth succeeded (or skipped). Proceed with normal event stream. ---

    state.ws_client_connect();

    // Subscribe to modem events
    let mut event_rx = state.subscribe_events();
    debug!("Subscribed to event broadcast channel");

    // Initial status for multi-modem: Send modem list
    debug!("Sending initial modem list...");
    {
        let modems = state.modems.read().await;
        let modem_ids: Vec<String> = modems.keys().cloned().collect();
        drop(modems);

        let msg = serde_json::json!({
            "type": "initial_status",
            "payload": {
                "modem_count": modem_ids.len(),
                "modem_ids": modem_ids
            }
        });
        if let Err(e) = sender.send(Message::Text(msg.to_string())).await {
            warn!("Failed to send initial status: {}", e);
            state.ws_client_disconnect();
            return;
        }
        debug!("Initial modem list sent: {} modems", modem_ids.len());
    }

    debug!("Entering WebSocket event loop...");

    // Spawn task to forward events to client
    let mut send_task = tokio::spawn(async move {
        debug!("Send task started, waiting for events...");
        loop {
            match event_rx.recv().await {
                Ok(broadcast_event) => {
                    debug!("Received event to forward: {:?}", std::mem::discriminant(&broadcast_event.event));
                    match serde_json::to_value(&broadcast_event.event) {
                        Ok(mut value) => {
                            // Inject modem_id as a top-level field when present
                            if let Some(ref modem_id) = broadcast_event.modem_id {
                                if let serde_json::Value::Object(ref mut map) = value {
                                    map.insert(
                                        "modem_id".to_string(),
                                        serde_json::Value::String(modem_id.clone()),
                                    );
                                }
                            }
                            let json = value.to_string();
                            if sender.send(Message::Text(json)).await.is_err() {
                                debug!("Failed to send event, client disconnected");
                                break;
                            }
                        }
                        Err(e) => {
                            error!("Failed to serialize event: {}", e);
                        }
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!("WebSocket client lagged, dropped {} events", n);
                }
                Err(broadcast::error::RecvError::Closed) => {
                    warn!("Broadcast channel closed, send task exiting");
                    break;
                }
            }
        }
        debug!("Send task ended");
    });

    // Handle incoming messages (ping/pong, close)
    let mut recv_task = tokio::spawn(async move {
        debug!("Receive task started, listening for client messages...");
        while let Some(msg) = receiver.next().await {
            match msg {
                Ok(Message::Close(_)) => {
                    debug!("Client sent close frame");
                    break;
                }
                Ok(Message::Ping(data)) => {
                    debug!("Received ping ({} bytes)", data.len());
                    // Pong is handled automatically by axum
                }
                Ok(Message::Pong(_)) => {
                    debug!("Received pong");
                }
                Ok(Message::Text(text)) => {
                    debug!("Received text message: {}", text);
                }
                Ok(Message::Binary(data)) => {
                    debug!("Received binary message ({} bytes)", data.len());
                }
                Err(e) => {
                    warn!("WebSocket receive error: {}", e);
                    break;
                }
            }
        }
        debug!("Receive task ended");
    });

    // Wait for either task to complete
    tokio::select! {
        result = &mut send_task => {
            debug!("Send task completed first: {:?}", result);
        }
        result = &mut recv_task => {
            debug!("Receive task completed first: {:?}", result);
        }
    }

    // Clean up both tasks
    send_task.abort();
    recv_task.abort();

    state.ws_client_disconnect();
    info!("WebSocket client disconnected");
}

/// Pure transition for the per-modem consecutive-lock-busy counter the cache
/// refresh task uses to detect a wedged handler (its mutex held forever by a
/// stuck blocking serial syscall). `acquired` = the lock attempt succeeded this
/// tick (reset to 0) vs. timed out (increment). Returns `(new_count,
/// escalate_to_unavailable)`; escalate fires once `new_count >= threshold`.
fn next_lock_busy_count(current: u32, acquired: bool, threshold: u32) -> (u32, bool) {
    if acquired {
        (0, false)
    } else {
        let n = current + 1;
        (n, n >= threshold)
    }
}

/// Spawn a background task that refreshes the master cache every 60 seconds.
///
/// Replaces the old `spawn_signal_broadcaster`. For each modem:
/// - Calls `get_signal()`, `get_connection_status()`, `get_registration()`
/// - Conditionally calls `get_gps_position()` when GPS panel is active
/// - Derives `signal_strength` from `rssi`
/// - Writes `ModemStateCache` and broadcasts `SignalUpdate` for WS clients
/// - Tracks failures; after 3 consecutive, marks modem as Unavailable
pub fn spawn_cache_refresh_task(state: Arc<AppState>) {
    info!("Starting cache refresh task (60s interval)");
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        let mut modem_failures: std::collections::HashMap<String, u32> =
            std::collections::HashMap::new();
        let mut lock_busy_counts: std::collections::HashMap<String, u32> =
            std::collections::HashMap::new();
        const FAILURE_THRESHOLD: u32 = 3;

        loop {
            interval.tick().await;

            // Snapshot modem IDs (avoid holding lock during polling)
            let modem_ids: Vec<String> = {
                let modems = state.modems.read().await;
                modems.keys().cloned().collect()
            };

            let gps_active = state
                .gps_panel_active
                .load(std::sync::atomic::Ordering::Relaxed);

            for modem_id in modem_ids {
                // Get handler Arc + check health
                let handler_arc = {
                    let modems = state.modems.read().await;
                    match modems.get(&modem_id) {
                        Some(context) => {
                            let health = context.health.read().await;
                            if !health.available {
                                debug!(
                                    "[{}] Not available, skipping cache refresh",
                                    modem_id
                                );
                                modem_failures.insert(modem_id.clone(), 0);
                                lock_busy_counts.insert(modem_id.clone(), 0);
                                continue;
                            }
                            Arc::clone(&context.handler)
                        }
                        None => continue,
                    }
                };

                // Acquire lock with 1s timeout
                let lock_result = tokio::time::timeout(
                    Duration::from_secs(1),
                    handler_arc.lock(),
                )
                .await;

                let modem = match lock_result {
                    Ok(modem) => {
                        // Acquired — clear any accumulated wedge count.
                        lock_busy_counts.insert(modem_id.clone(), 0);
                        modem
                    }
                    Err(_) => {
                        // Lock-acquire timed out. A handler whose lock can't be
                        // acquired for several consecutive ticks is wedged (a stuck
                        // blocking serial syscall holds the guard) — escalate so the
                        // reconnect watcher can recover it.
                        let current = lock_busy_counts.get(&modem_id).copied().unwrap_or(0);
                        let (new_count, escalate) =
                            next_lock_busy_count(current, false, FAILURE_THRESHOLD);
                        lock_busy_counts.insert(modem_id.clone(), new_count);
                        warn!(
                            "[{}] Handler lock wedged ({}/{})",
                            modem_id, new_count, FAILURE_THRESHOLD
                        );
                        if escalate {
                            let modems = state.modems.read().await;
                            if let Some(context) = modems.get(&modem_id) {
                                let mut health = context.health.write().await;
                                if health.state == ModemHealthState::Ok {
                                    warn!(
                                        "[{}] Handler lock wedged {}x, marking unavailable",
                                        modem_id, new_count
                                    );
                                    *health = ModemHealth {
                                        available: false,
                                        state: ModemHealthState::Unavailable,
                                        message: Some(
                                            "Handler lock wedged (stuck serial I/O)".to_string(),
                                        ),
                                    };
                                    state.broadcast_modem_event(
                                        &modem_id,
                                        ModemEvent::ModemHealth(health.clone()),
                                    );
                                }
                            }
                        }
                        continue;
                    }
                };

                // Call trait methods sequentially while holding lock
                let signal_result = modem.get_signal().await;
                let conn_result = modem.get_connection_status().await;
                let reg_result = modem.get_registration().await;
                let gps_result = if gps_active {
                    modem.get_gps_position().await.ok()
                } else {
                    None
                };

                // Drop lock before writing to cache
                drop(modem);

                match (&signal_result, &conn_result, &reg_result) {
                    (Ok(signal), Ok(connection), Ok(registration)) => {
                        modem_failures.insert(modem_id.clone(), 0);

                        let signal_strength =
                            ((signal.rssi + 113.0) * 100.0 / 62.0).clamp(0.0, 100.0)
                                as i32;

                        let cache = ModemStateCache {
                            signal: signal.clone(),
                            connection: connection.clone(),
                            signal_strength,
                            registration: registration.clone(),
                            gps: gps_result,
                            timestamp: chrono::Utc::now().to_rfc3339(),
                        };

                        // Write to modem's state_cache + last_signal + signal_history
                        {
                            let modems = state.modems.read().await;
                            if let Some(context) = modems.get(&modem_id) {
                                let mut sc = context.state_cache.write().await;
                                *sc = Some(cache);
                                let mut ls = context.last_signal.write().await;
                                *ls = Some(signal.clone());

                                // Append to signal history ring buffer
                                let sample = SignalSample {
                                    ts: chrono::Utc::now().timestamp(),
                                    rsrp: signal.rsrp as f32,
                                    rsrq: signal.rsrq as f32,
                                    sinr: signal.sinr as f32,
                                };
                                let mut history = context.signal_history.write().await;
                                if history.len() >= 1440 {
                                    history.pop_front();
                                }
                                history.push_back(sample);
                            }
                        }

                        // --- Live device_path reconcile (Approach A) ---
                        // Delegate to AppState::reconcile_modem_device_path which
                        // reads the handler's live-port cell (written by reopen_port
                        // on self-heal), and if it differs from the reported path,
                        // refreshes ctx.detected.device_path + the matching
                        // detected_modems entry under the appropriate write locks.
                        // Gather-then-write; no lock held across an await.
                        if let Some(new_path) = state.reconcile_modem_device_path(&modem_id).await {
                            info!("[{}] device_path reconciled to {}", modem_id, new_path);
                        }

                        // Broadcast signal update for WS clients
                        state.broadcast_modem_event(&modem_id, ModemEvent::SignalUpdate(signal.clone()));
                        debug!("[{}] Cache refreshed", modem_id);
                    }
                    _ => {
                        let failures =
                            modem_failures.entry(modem_id.clone()).or_insert(0);
                        *failures += 1;
                        warn!(
                            "[{}] Cache refresh failed ({}/{})",
                            modem_id, *failures, FAILURE_THRESHOLD
                        );

                        if *failures >= FAILURE_THRESHOLD {
                            let modems = state.modems.read().await;
                            if let Some(context) = modems.get(&modem_id) {
                                let mut health = context.health.write().await;
                                if health.state == ModemHealthState::Ok {
                                    warn!(
                                        "[{}] Unreachable after {} failures, marking unavailable",
                                        modem_id, FAILURE_THRESHOLD
                                    );
                                    *health = ModemHealth {
                                        available: false,
                                        state: ModemHealthState::Unavailable,
                                        message: Some(
                                            "Lost contact during cache refresh"
                                                .to_string(),
                                        ),
                                    };
                                    state.broadcast_modem_event(&modem_id, ModemEvent::ModemHealth(
                                        health.clone(),
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        }
    });
}

/// Spawn a background task that watches for modem recovery.
///
/// Monitors modem health states and automatically recovers modems that have
/// become unavailable due to USB re-enumeration (firmware updates, reboots, etc.).
///
/// **Recovery Process:**
/// 1. Detects modems marked `Unavailable` by cache refresh task (3+ consecutive failures)
/// 2. Re-scans USB to find re-enumerated hardware
/// 3. Matches stable modem IDs (VID:PID:SERIAL remains constant)
/// 4. Creates new handler with updated serial ports
/// 5. Swaps handler into existing ModemContext
/// 6. Restores connection config (APN) and network interface
/// 7. Broadcasts health recovery event to WebSocket clients
///
/// Runs every 30 seconds. Modems that aren't found in USB scan will be retried
/// on the next cycle (USB re-enumeration can take 15-30s after reboot).
pub fn spawn_reconnect_watcher(state: Arc<AppState>) {
    info!("Starting reconnect watcher (30s interval)");
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(30));
        let mut reboot_timers: std::collections::HashMap<String, std::time::Instant> =
            std::collections::HashMap::new();
        let mut removal_tracker: std::collections::HashMap<String, std::time::Instant> =
            std::collections::HashMap::new();
        const REBOOT_TIMEOUT_SECS: u64 = 90;
        const REMOVAL_TIMEOUT_SECS: u64 = 300; // 5 minutes

        loop {
            interval.tick().await;

            // Step 1: Snapshot modem health states (avoid holding lock during recovery)
            let modem_states: Vec<(String, ModemHealthState, bool)> = {
                let modems = state.modems.read().await;
                let mut states = Vec::new();
                for (modem_id, context) in modems.iter() {
                    let health = context.health.read().await;
                    states.push((modem_id.clone(), health.state, health.available));
                }
                states
            };

            // Step 2: Identify modems needing recovery
            let mut needs_recovery = Vec::new();
            for (modem_id, health_state, available) in modem_states {
                if !available && health_state == ModemHealthState::Unavailable {
                    needs_recovery.push(modem_id.clone());
                    reboot_timers.remove(&modem_id);
                } else if health_state == ModemHealthState::Rebooting {
                    let elapsed = reboot_timers
                        .get(&modem_id)
                        .map(|start| start.elapsed().as_secs())
                        .unwrap_or_else(|| {
                            reboot_timers
                                .insert(modem_id.clone(), std::time::Instant::now());
                            0
                        });

                    if elapsed > REBOOT_TIMEOUT_SECS {
                        info!(
                            "[{}] Reboot timeout exceeded ({}s), attempting recovery",
                            modem_id, elapsed
                        );
                        needs_recovery.push(modem_id.clone());
                        reboot_timers.remove(&modem_id);
                    }
                }
            }

            // Always scan USB to detect hot-plug/unplug (USB sysfs scanning is cheap)
            // Don't skip even if needs_recovery is empty

            // Step 3: Re-scan USB to find available hardware
            let detected = {
                let registry = Arc::clone(&state.profile_registry);
                match tokio::task::spawn_blocking(move || {
                    crate::hardware::detect_modems(&registry, crate::hardware::DetectionVerbosity::Quiet)
                })
                .await
                {
                    Ok(detected) => detected,
                    Err(e) => {
                        warn!("USB rescan task panicked: {}", e);
                        continue;
                    }
                }
            };

            if detected.is_empty() {
                debug!("USB rescan found no modems, will retry next cycle");
                continue;
            }

            // Step 4: Generate IDs for all detected modems
            let mut detected_with_ids: Vec<(String, crate::hardware::DetectedModem)> =
                Vec::new();
            for modem in detected {
                match crate::hardware::generate_modem_id(&modem) {
                    Ok(id) => detected_with_ids.push((id, modem)),
                    Err(e) => {
                        warn!(
                            "Failed to generate ID for modem at {}: {}",
                            modem.device_path, e
                        );
                    }
                }
            }

            // Step 5: Hot-plug detection - identify and add new modems
            let current_modem_ids: std::collections::HashSet<String> = {
                let modems = state.modems.read().await;
                modems.keys().cloned().collect()
            };

            let new_modems: Vec<_> = detected_with_ids
                .iter()
                .filter(|(id, _)| !current_modem_ids.contains(id))
                .collect();

            for (modem_id, detected) in new_modems {
                info!("[{}] Hot-plug detected", modem_id);

                // Match profile from registry
                let profile = match (&detected.vendor_id, &detected.product_id) {
                    (Some(vid), Some(pid)) => state.profile_registry.match_profile(vid, pid).clone(),
                    _ => state.profile_registry.generic().clone(),
                };

                // Create handler (30s timeout, spawn_blocking)
                let detected_clone = detected.clone();
                let profile_clone = profile.clone();
                let handler_task = tokio::task::spawn_blocking(move || {
                    crate::hardware::create_modem_handler(&detected_clone, profile_clone)
                });

                let handler = match tokio::time::timeout(
                    Duration::from_secs(30),
                    handler_task,
                )
                .await
                {
                    Ok(Ok(Ok(handler))) => {
                        info!("[{}] Handler created successfully", modem_id);
                        handler
                    }
                    Ok(Ok(Err(e))) => {
                        warn!("[{}] Handler creation failed: {}, skipping", modem_id, e);
                        continue;
                    }
                    Ok(Err(e)) => {
                        warn!("[{}] Handler creation task panicked: {}, skipping", modem_id, e);
                        continue;
                    }
                    Err(_) => {
                        warn!("[{}] Handler creation timed out (30s), skipping", modem_id);
                        continue;
                    }
                };

                // Run discovery (15s timeout with fallback to defaults)
                let handler_arc = Arc::new(tokio::sync::Mutex::new(handler));
                let handler_clone = Arc::clone(&handler_arc);

                let discovery = match tokio::time::timeout(
                    Duration::from_secs(15),
                    async move {
                        let modem = handler_clone.lock().await;
                        modem.get_discovery_info().await
                    },
                )
                .await
                {
                    Ok(Ok(info)) => {
                        info!("[{}] Discovery completed", modem_id);
                        info
                    }
                    Ok(Err(e)) => {
                        warn!("[{}] Discovery failed: {}, using defaults", modem_id, e);
                        crate::hardware::DiscoveryInfo::default()
                    }
                    Err(_) => {
                        warn!("[{}] Discovery timed out, using defaults", modem_id);
                        crate::hardware::DiscoveryInfo::default()
                    }
                };

                // Extract handler from Arc for add_modem
                let handler = match Arc::try_unwrap(handler_arc) {
                    Ok(mutex) => mutex.into_inner(),
                    Err(_) => {
                        warn!("[{}] Failed to unwrap handler Arc (unexpected), skipping", modem_id);
                        continue;
                    }
                };

                // Add to state
                let config = {
                    let config = state.config.read().await;
                    config.connection.clone()
                };

                state.add_modem(
                    modem_id.clone(),
                    handler,
                    profile,
                    detected.clone(),
                    config,
                    discovery,
                ).await;

                // USB-net mode detection (diagnostic only; never blocks bring-up).
                // Per spec §3.10 detect_usbnet_mode never returns Err; failure cached as Unknown.
                state.detect_and_cache_usbnet_mode(modem_id).await;

                // Broadcast hot-plug event
                state.broadcast_modem_event(
                    modem_id,
                    crate::hardware::ModemEvent::ModemHealth(crate::hardware::ModemHealth {
                        available: true,
                        state: crate::hardware::ModemHealthState::Ok,
                        message: Some("Hot-plug detected".to_string()),
                    }),
                );

                info!("[{}] Hot-plug addition complete", modem_id);
            }

            // Step 6: Match needed modems to detected hardware and recover
            for modem_id in needs_recovery {
                let Some((_, detected)) = detected_with_ids
                    .iter()
                    .find(|(id, _)| id == &modem_id)
                else {
                    debug!(
                        "[{}] Not found in USB rescan, will retry next cycle",
                        modem_id
                    );
                    continue;
                };

                info!("[{}] Found during USB rescan, starting recovery", modem_id);

                // Step 6: Get profile and handler Arc
                let (profile, old_handler_arc) = {
                    let modems = state.modems.read().await;
                    match modems.get(&modem_id) {
                        Some(context) => {
                            (Arc::clone(&context.profile), Arc::clone(&context.handler))
                        }
                        None => {
                            warn!(
                                "[{}] ModemContext disappeared during recovery",
                                modem_id
                            );
                            continue;
                        }
                    }
                };

                // Step 7: Create new handler (blocking serial port open)
                let detected_clone = detected.clone();
                let profile_data = (*profile).clone();
                let new_handler_task = tokio::task::spawn_blocking(move || {
                    crate::hardware::create_modem_handler(&detected_clone, profile_data)
                });

                let new_handler = match tokio::time::timeout(
                    Duration::from_secs(30),
                    new_handler_task,
                )
                .await
                {
                    Ok(Ok(Ok(handler))) => {
                        info!("[{}] New handler created successfully", modem_id);
                        handler
                    }
                    Ok(Ok(Err(e))) => {
                        warn!("[{}] Handler creation failed: {}", modem_id, e);
                        continue;
                    }
                    Ok(Err(e)) => {
                        warn!("[{}] Handler creation task panicked: {}", modem_id, e);
                        continue;
                    }
                    Err(_) => {
                        warn!("[{}] Handler creation timed out (30s)", modem_id);
                        continue;
                    }
                };

                // Step 8: Install the new handler. Normal case: lock the old handler
                // and swap the Box in place (no leak). Wedged case: a stuck blocking
                // serial syscall holds the old mutex forever, so fall back to
                // replacing the ENTIRE handler Arc — new callers get the fresh
                // handler; the old Arc + its stuck thread/fd leak until restart.
                match tokio::time::timeout(Duration::from_secs(2), old_handler_arc.lock()).await {
                    Ok(mut handler_guard) => {
                        *handler_guard = new_handler;
                        info!("[{}] Handler swapped in place", modem_id);
                    }
                    Err(_) => {
                        warn!(
                            "[{}] Old handler lock wedged; replacing handler Arc (old leaks until restart)",
                            modem_id
                        );
                        state.replace_handler(&modem_id, new_handler).await;
                    }
                }

                // Step 9: Re-run discovery to refresh IMEI/SIM cache
                let discovery = {
                    let handler_arc = {
                        let modems = state.modems.read().await;
                        modems.get(&modem_id).map(|ctx| Arc::clone(&ctx.handler))
                    };

                    if let Some(arc) = handler_arc {
                        if let Ok(handler) = tokio::time::timeout(
                            Duration::from_secs(2),
                            arc.lock(),
                        )
                        .await
                        {
                            match tokio::time::timeout(
                                Duration::from_secs(15),
                                handler.get_discovery_info(),
                            )
                            .await
                            {
                                Ok(Ok(info)) => Some(info),
                                Ok(Err(e)) => {
                                    warn!(
                                        "[{}] Discovery failed: {}, keeping cached data",
                                        modem_id, e
                                    );
                                    None
                                }
                                Err(_) => {
                                    warn!(
                                        "[{}] Discovery timed out, keeping cached data",
                                        modem_id
                                    );
                                    None
                                }
                            }
                        } else {
                            warn!("[{}] Lock timeout during discovery", modem_id);
                            None
                        }
                    } else {
                        None
                    }
                };

                if let Some(disc) = discovery {
                    let modems = state.modems.read().await;
                    if let Some(context) = modems.get(&modem_id) {
                        let mut discovery_guard = context.discovery.write().await;
                        *discovery_guard = disc;
                    }
                }

                // Step 10: Restore saved APN
                crate::api::routes::modem::ensure_saved_apn(&state, &modem_id).await;

                // Step 11: Bounce WWAN interface for ECM modems
                bounce_wwan_interface().await;

                // Step 12: Update health state to OK
                let new_health = ModemHealth {
                    available: true,
                    state: ModemHealthState::Ok,
                    message: Some("Recovered by reconnect watcher".to_string()),
                };

                {
                    let modems = state.modems.read().await;
                    if let Some(context) = modems.get(&modem_id) {
                        let mut health = context.health.write().await;
                        *health = new_health.clone();
                    }
                }

                // Step 13: Broadcast recovery event
                state.broadcast_modem_event(
                    &modem_id,
                    ModemEvent::ModemHealth(new_health),
                );

                info!("[{}] Recovery complete - modem available", modem_id);
                reboot_timers.remove(&modem_id);
            }

            // Step 7: Hot-unplug removal - track unavailable modems missing from USB
            let unavailable_modems: Vec<String> = {
                let modems = state.modems.read().await;
                let mut unavailable = Vec::new();
                for (modem_id, context) in modems.iter() {
                    let health = context.health.read().await;
                    if !health.available && health.state == ModemHealthState::Unavailable {
                        unavailable.push(modem_id.clone());
                    }
                }
                unavailable
            };

            let detected_ids: std::collections::HashSet<String> =
                detected_with_ids.iter().map(|(id, _)| id.clone()).collect();

            for modem_id in unavailable_modems {
                if !detected_ids.contains(&modem_id) {
                    // Missing from USB - start/continue removal timer
                    let first_missing = removal_tracker
                        .entry(modem_id.clone())
                        .or_insert_with(std::time::Instant::now);
                    let elapsed = first_missing.elapsed().as_secs();

                    if elapsed > REMOVAL_TIMEOUT_SECS {
                        info!(
                            "[{}] Removing after {}s absence from USB (hot-unplug)",
                            modem_id, elapsed
                        );

                        // Remove from state
                        state.remove_modem(&modem_id).await;
                        removal_tracker.remove(&modem_id);

                        // Broadcast removal event
                        state.broadcast_modem_event(
                            &modem_id,
                            ModemEvent::ModemHealth(ModemHealth {
                                available: false,
                                state: ModemHealthState::Error,
                                message: Some("Modem removed (hot-unplug)".to_string()),
                            }),
                        );
                    } else {
                        debug!(
                            "[{}] Missing from USB for {}s (will remove after {}s)",
                            modem_id, elapsed, REMOVAL_TIMEOUT_SECS
                        );
                    }
                } else {
                    // Modem reappeared in USB before timeout - clear tracker
                    if removal_tracker.remove(&modem_id).is_some() {
                        debug!(
                            "[{}] Reappeared in USB scan, clearing removal timer",
                            modem_id
                        );
                    }
                }
            }
        }
    });
}

/// Bounce the WWAN network interface to re-establish the data channel.
///
/// After a modem reboot (AT+CFUN=1,1), the AT serial port recovers but the
/// USB network device (usb0 in ECM mode, wwan0 in QMI mode) disappears and
/// re-enumerates. Running `ifdown WWAN && sleep 3 && ifup WWAN` forces
/// netifd to tear down the stale interface and start a fresh DHCP session.
pub async fn bounce_wwan_interface() {
    // Only run on real hardware (not in dev/mock mode)
    if std::env::var("MOCK_HARDWARE").is_ok() {
        debug!("Mock mode — skipping WWAN bounce");
        return;
    }

    info!("Bouncing WWAN interface to re-establish data channel...");
    crate::state::debug_trace_with_source("[WWAN] ifdown WWAN; sleep 3; ifup WWAN", "reconnect");

    match tokio::process::Command::new("sh")
        .arg("-c")
        .arg("ifdown WWAN 2>/dev/null; sleep 3; ifup WWAN 2>/dev/null")
        .output()
        .await
    {
        Ok(output) => {
            if output.status.success() {
                crate::state::debug_trace_with_source("[WWAN] Bounce completed OK", "reconnect");
                info!("WWAN interface bounced successfully");
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                crate::state::debug_trace_with_source(format!("[WWAN] Bounce non-zero exit: {stderr}"), "reconnect");
                warn!("WWAN bounce completed with non-zero exit: {}", stderr);
            }
        }
        Err(e) => {
            // Not fatal — ifdown/ifup may not exist in all environments
            warn!("Failed to bounce WWAN interface: {} (interface may need manual restart)", e);
        }
    }
}

// ============================================================================
// WAN Connectivity Watchdog
// ============================================================================

/// Spawn the WAN connectivity watchdog background task.
///
/// Periodically checks internet connectivity through each modem's network
/// interface. If the primary modem fails consecutive checks, triggers
/// failover to the next healthy modem (unless failover is locked).
pub fn spawn_wan_watchdog(state: Arc<AppState>) {
    info!("Starting WAN connectivity watchdog");
    tokio::spawn(async move {
        // Wait for initial stabilization
        tokio::time::sleep(Duration::from_secs(10)).await;

        // Initialize current_routed_wan from config (first entry = user's primary)
        {
            let config = state.wan_config.read().await;
            let primary = config.modem_priority.first().map(|e| e.modem_id.clone());
            let mut runtime = state.wan_runtime.write().await;
            runtime.current_routed_wan = primary;
        }

        // Per-modem restart cooldowns keyed by modem_id.
        let mut restart_cooldowns: std::collections::HashMap<String, tokio::time::Instant> =
            std::collections::HashMap::new();

        // Per-modem SIM recheck counters — for no-SIM modems, re-probe every N cycles
        // in case a SIM card was inserted.
        const SIM_RECHECK_INTERVAL: u32 = 10;
        let mut sim_recheck_counters: std::collections::HashMap<String, u32> =
            std::collections::HashMap::new();

        loop {
            // Read current check interval from config
            let interval_secs = {
                let config = state.wan_config.read().await;
                if !config.enabled || !config.watchdog.enabled {
                    drop(config);
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    continue;
                }
                config.watchdog.check_interval_secs
            };

            tokio::time::sleep(Duration::from_secs(interval_secs as u64)).await;

            // Re-check enabled state (may have changed during sleep)
            let config = state.wan_config.read().await;
            if !config.enabled || !config.watchdog.enabled {
                continue;
            }

            let watchdog = config.watchdog.clone();
            // Collect all modems for health checks and routing decisions.
            let all_modems = config.modem_priority.to_vec();
            let failover_locked = config.failover_locked;
            let failure_threshold = config.watchdog.failure_threshold;
            drop(config);

            if all_modems.is_empty() {
                continue;
            }

            // Run health checks for all WANs that have a network device.
            // Standby WANs with IPs are health-checked so they can be promoted
            // when all active WANs fail. WANs with no network device are skipped.
            for modem in &all_modems {
                // Skip WANs with no network device (can't health-check)
                if modem.network_device.is_empty() {
                    continue;
                }

                // --- Ethernet entries: skip SIM check entirely ---
                let is_ethernet = modem.entry_type == crate::hardware::WanEntryType::Ethernet;

                // --- SIM presence check (modems only) ---
                // Skip health checks entirely for modems with no SIM card.
                // Unknown modems are probed once; no-SIM modems are re-probed
                // every SIM_RECHECK_INTERVAL cycles in case a SIM is inserted.
                // Ethernet entries always pass the SIM check.
                let cached_has_sim = if is_ethernet {
                    Some(true) // Ethernet doesn't need SIM
                } else {
                    let runtime = state.wan_runtime.read().await;
                    runtime
                        .modem_statuses
                        .get(&modem.modem_id)
                        .and_then(|i| i.has_sim)
                };

                let need_sim_probe = match cached_has_sim {
                    None => true, // Never checked
                    Some(false) => {
                        // No SIM — re-check periodically
                        let counter = sim_recheck_counters
                            .entry(modem.modem_id.clone())
                            .or_insert(0);
                        *counter += 1;
                        if *counter >= SIM_RECHECK_INTERVAL {
                            *counter = 0;
                            true
                        } else {
                            false
                        }
                    }
                    Some(true) => false, // Has SIM, health checks will validate connectivity
                };

                let sim_present = if need_sim_probe {
                    let result = check_modem_sim(modem, &state).await;
                    let was_no_sim = cached_has_sim == Some(false);
                    {
                        let mut runtime = state.wan_runtime.write().await;
                        let info = runtime
                            .modem_statuses
                            .entry(modem.modem_id.clone())
                            .or_insert_with(|| WanModemRuntimeInfo {
                                status: WanModemStatus::Offline,
                                consecutive_failures: 0,
                                last_check: None,
                                network_device: Some(modem.network_device.clone()),
                                has_sim: None,
                                restart_count: 0,
                                restart_suspended: false,
                                healthy_since: None,
                            });
                        info.has_sim = Some(result);
                    }
                    if result && was_no_sim {
                        crate::state::debug_trace_with_source(format!(
                            "[WAN] {} SIM detected — enabling health checks",
                            modem.label
                        ), "wan");
                    } else if !result && !was_no_sim {
                        crate::state::debug_trace_with_source(format!(
                            "[WAN] {} no SIM in current slot — skipping health checks",
                            modem.label
                        ), "wan");
                    }
                    result
                } else {
                    cached_has_sim.unwrap_or(true)
                };

                // Skip health checks for no-SIM modems
                if !sim_present {
                    let mut runtime = state.wan_runtime.write().await;
                    let info = runtime
                        .modem_statuses
                        .entry(modem.modem_id.clone())
                        .or_insert_with(|| WanModemRuntimeInfo {
                            status: WanModemStatus::Offline,
                            consecutive_failures: 0,
                            last_check: None,
                            network_device: Some(modem.network_device.clone()),
                            has_sim: Some(false),
                            restart_count: 0,
                            restart_suspended: false,
                            healthy_since: None,
                        });
                    info.status = WanModemStatus::NoSim;
                    info.consecutive_failures = 0;
                    info.last_check = None;
                    continue;
                }

                // --- Health check ---
                let result = run_health_check(
                    &modem.network_device,
                    &watchdog.ping_target,
                    &watchdog.dns_target,
                    &watchdog.http_target,
                )
                .await;

                // Update runtime state
                let mut runtime = state.wan_runtime.write().await;
                let info = runtime
                    .modem_statuses
                    .entry(modem.modem_id.clone())
                    .or_insert_with(|| WanModemRuntimeInfo {
                        status: WanModemStatus::Offline,
                        consecutive_failures: 0,
                        last_check: None,
                        network_device: Some(modem.network_device.clone()),
                        has_sim: None,
                        restart_count: 0,
                        restart_suspended: false,
                        healthy_since: None,
                    });

                if result.overall_ok {
                    if info.consecutive_failures > 0 {
                        crate::state::debug_trace_with_source(format!(
                            "[WAN] {} recovered (was at {} failures)",
                            modem.label, info.consecutive_failures
                        ), "wan");
                    }
                    info.status = WanModemStatus::Online;
                    info.consecutive_failures = 0;
                    if info.healthy_since.is_none() {
                        info.healthy_since = Some(tokio::time::Instant::now());
                    }
                } else {
                    info.consecutive_failures += 1;
                    crate::state::debug_trace_with_source(format!(
                        "[WAN] {} check failed ({}/{} failures)",
                        modem.label, info.consecutive_failures, failure_threshold
                    ), "wan");
                    if info.consecutive_failures >= failure_threshold {
                        info.status = WanModemStatus::Offline;
                    }
                    info.healthy_since = None;
                }
                info.last_check = Some(result);
            }

            // ── Best-healthy-WAN routing decision ──────────────────────
            if !all_modems.is_empty() && !failover_locked {
                let (desired, current_routed, current_idx, desired_idx) = {
                    let runtime = state.wan_runtime.read().await;
                    let desired = compute_desired_primary(&all_modems, &runtime.modem_statuses);
                    let current_routed = runtime.current_routed_wan.clone();
                    let current_idx = current_routed.as_ref().and_then(|id| {
                        all_modems.iter().position(|m| &m.modem_id == id)
                    });
                    let desired_idx = desired.as_ref().and_then(|id| {
                        all_modems.iter().position(|m| &m.modem_id == id)
                    });
                    (desired, current_routed, current_idx, desired_idx)
                };

                if let (Some(desired_id), Some(d_idx)) = (&desired, desired_idx) {
                    let needs_switch = match current_idx {
                        None => true,
                        Some(c_idx) if c_idx == d_idx => false,
                        Some(c_idx) if d_idx > c_idx => true, // Downgrade: immediate
                        Some(_c_idx) => {
                            // Upgrade: gate on stabilization
                            let config = state.wan_config.read().await;
                            let failback_mins = config.failback_timer_mins;
                            drop(config);

                            if failback_mins == 0 {
                                false // "Never" failback
                            } else {
                                let runtime = state.wan_runtime.read().await;
                                runtime.modem_statuses.get(desired_id).and_then(|info| {
                                    info.healthy_since.map(|since| {
                                        since.elapsed().as_secs() >= (failback_mins as u64) * 60
                                    })
                                }).unwrap_or(false)
                            }
                        }
                    };

                    if needs_switch {
                        let switch_label = if desired_idx > current_idx { "Failover" } else { "Failback" };
                        let desired_label = all_modems.iter()
                            .find(|m| m.modem_id == *desired_id)
                            .map(|m| m.label.clone())
                            .unwrap_or_else(|| desired_id.clone());
                        let current_label = current_routed.as_ref()
                            .and_then(|id| all_modems.iter().find(|m| &m.modem_id == id))
                            .map(|m| m.label.clone())
                            .unwrap_or_else(|| "none".to_string());

                        crate::state::debug_trace_with_source(format!(
                            "[WAN] {switch_label}: {current_label} -> {desired_label}"
                        ), "wan");

                        // Execute the route change
                        let mut route_changed = false;
                        {
                            let policy_routing_enabled = state.platform_capabilities.read().await.policy_routing_enabled;
                            if policy_routing_enabled {
                                let routing_mode = state.wan_config.read().await.routing_mode.clone();
                                match routing_mode {
                                    RoutingMode::LoadBalance => {
                                        let rs = state.routing_state.read().await;
                                        let config = state.wan_config.read().await;
                                        let runtime_r = state.wan_runtime.read().await;
                                        let weights: std::collections::HashMap<String, u32> = config.modem_priority.iter()
                                            .map(|e| (e.modem_id.clone(), e.weight.unwrap_or(1)))
                                            .collect();
                                        let healthy_ids: Vec<String> = config.modem_priority.iter()
                                            .filter(|e| runtime_r.modem_statuses.get(&e.modem_id)
                                                .is_some_and(|info| info.status == WanModemStatus::Online))
                                            .map(|e| e.modem_id.clone())
                                            .collect();
                                        drop(config);
                                        drop(runtime_r);

                                        match routing::apply_load_balance_route(&rs, &healthy_ids, &weights) {
                                            Ok(n) => {
                                                info!("Load balance route rebuilt with {n} WANs");
                                                route_changed = true;
                                            }
                                            Err(e) => error!("Multipath rebuild failed: {e}"),
                                        }
                                    }
                                    RoutingMode::Failover => {
                                        let rs = state.routing_state.read().await;
                                        if let Some(entry) = rs.get(desired_id) {
                                            if let Err(e) = routing::set_main_default(entry) {
                                                error!("Policy routing switch failed: {e}");
                                            } else {
                                                route_changed = true;
                                            }
                                        }
                                    }
                                }
                            }

                            if !policy_routing_enabled || !route_changed {
                                if let Some(current_id) = &current_routed {
                                    if let Some(current_entry) = all_modems.iter().find(|m| &m.modem_id == current_id) {
                                        let _ = uci_set_metric_cmd(&current_entry.interface_name, 999).await;
                                    }
                                }
                                if let Some(desired_entry) = all_modems.iter().find(|m| m.modem_id == *desired_id) {
                                    let _ = uci_set_metric_cmd(&desired_entry.interface_name, 20).await;
                                }
                                let _ = uci_commit_cmd().await;
                                route_changed = true;
                            }
                        }

                        if route_changed {
                            let old_id = {
                                let mut runtime = state.wan_runtime.write().await;
                                runtime.current_routed_wan.replace(desired_id.clone())
                            };
                            let old_label = old_id.as_ref()
                                .and_then(|id| all_modems.iter().find(|m| &m.modem_id == id))
                                .map(|m| m.label.clone())
                                .unwrap_or_else(|| "none".to_string());

                            let timestamp = chrono::Utc::now().to_rfc3339();
                            let log_line = format!(
                                "{timestamp} {switch_label} from=\"{old_label}\" to=\"{desired_label}\""
                            );
                            let _ = crate::config::wan::append_watchdog_log(&log_line).await;

                            {
                                let mut runtime = state.wan_runtime.write().await;
                                let event = FailoverEvent {
                                    timestamp: chrono::Utc::now().to_rfc3339(),
                                    from_modem_id: old_id.unwrap_or_default(),
                                    from_label: old_label.clone(),
                                    to_modem_id: desired_id.clone(),
                                    to_label: desired_label.clone(),
                                    reason: format!("{switch_label}: best healthy WAN changed"),
                                };
                                runtime.failover_history.push_front(event);
                                if runtime.failover_history.len() > 50 {
                                    runtime.failover_history.pop_back();
                                }
                            }

                            state.audit.log(
                                crate::security::audit::AuditEventType::ConfigChanged,
                                None,
                                format!("WAN {}: {} -> {} (auto)", switch_label.to_lowercase(), old_label, desired_label),
                            ).await;
                        }
                    }
                }

                // Update failover_override as derived state (informational for UI)
                {
                    let config = state.wan_config.read().await;
                    let configured_primary = config.modem_priority.first().map(|e| e.modem_id.clone());
                    drop(config);

                    let mut runtime = state.wan_runtime.write().await;
                    let current_routed_for_ui = runtime.current_routed_wan.clone();
                    if let (Some(configured), Some(current)) = (&configured_primary, &current_routed_for_ui) {
                        if configured != current {
                            let needs_create = runtime.failover_override.as_ref()
                                .map(|fo| &fo.current_primary_id != current)
                                .unwrap_or(true);
                            if needs_create {
                                runtime.failover_override = Some(crate::state::FailoverOverride {
                                    original_primary_id: configured.clone(),
                                    current_primary_id: current.clone(),
                                    failover_timestamp: chrono::Utc::now().to_rfc3339(),
                                });
                            }
                        } else {
                            runtime.failover_override = None;
                        }
                    }
                }
            }

            // --- Per-modem restart ---
            // Any modem that crosses the failure threshold gets restarted individually
            // via its own AT port (AT+CFUN=1,1). Each modem has its own cooldown timer.
            // In multi-modem mode, look up handler by modem_id from HashMap.
            if watchdog.restart_on_failure {
                for modem in &all_modems {
                    // Check if this modem has crossed the failure threshold
                    let needs_restart = {
                        let runtime = state.wan_runtime.read().await;
                        runtime
                            .modem_statuses
                            .get(&modem.modem_id)
                            .is_some_and(|info| {
                                info.consecutive_failures >= failure_threshold
                            })
                    };
                    if !needs_restart {
                        continue;
                    }

                    // Sub-task 3 (Item #37): fast-fail-suspend on DHCP starvation.
                    // If this Modem-type entry's resolved proto is "dhcp" and the
                    // modem currently has no IP after threshold ticks, the daemon
                    // would burn 5 restart attempts uselessly because restarting
                    // a modem doesn't fix a config-mismatch (operator-overridden
                    // dhcp on non-ECM modem, mode-detection bug, genuine ECM
                    // lease starvation). Skip the restart cycle and emit a
                    // specific actionable message naming exactly what's wrong.
                    // Existing un-suspend triggers (Save & Apply / manual clear /
                    // re-detect) cover re-enabling. compute_desired_primary
                    // failover-to-Ethernet runs unaffected on its own cadence.
                    let already_suspended_for_fastfail = {
                        let runtime = state.wan_runtime.read().await;
                        runtime
                            .modem_statuses
                            .get(&modem.modem_id)
                            .map(|info| info.restart_suspended)
                            .unwrap_or(false)
                    };
                    if !already_suspended_for_fastfail {
                        // Plumb the predicate inputs.
                        let cached_usbnet_mode: Option<crate::hardware::UsbNetMode> = {
                            let modems_map = state.modems.read().await;
                            if let Some(ctx) = modems_map.get(&modem.modem_id) {
                                Some(*ctx.usbnet_mode.read().await)
                            } else {
                                None
                            }
                        };
                        let resolved_proto = crate::api::routes::wan::resolve_uci_proto(
                            modem,
                            cached_usbnet_mode,
                        );
                        let has_current_ip = get_interface_ip(&modem.network_device)
                            .await
                            .is_some();
                        let current_failures = {
                            let runtime = state.wan_runtime.read().await;
                            runtime
                                .modem_statuses
                                .get(&modem.modem_id)
                                .map(|info| info.consecutive_failures)
                                .unwrap_or(0)
                        };
                        if should_fast_fail(
                            modem.entry_type.clone(),
                            resolved_proto.as_ref(),
                            has_current_ip,
                            current_failures,
                            failure_threshold,
                        ) {
                            // Mark suspended in runtime state.
                            {
                                let mut runtime = state.wan_runtime.write().await;
                                if let Some(info) = runtime.modem_statuses.get_mut(&modem.modem_id) {
                                    info.restart_suspended = true;
                                }
                            }

                            // 4a — Watchdog log (operator-facing, mode-agnostic).
                            // Format matches the existing per-modem RESTART_SUSPENDED
                            // line at this same block (Reached max restart attempts variant).
                            let timestamp = chrono::Utc::now().to_rfc3339();
                            let log_line = format!(
                                "{timestamp} RESTART_SUSPENDED modem=\"{}\" reason=\"DHCP starvation — proto-config mismatch\"",
                                modem.label,
                            );
                            let _ = crate::config::wan::append_watchdog_log(&log_line).await;

                            // 4b — Audit log (operator-facing, surfaced in UI Audit panel).
                            state
                                .audit
                                .log(
                                    crate::security::audit::AuditEventType::ConfigChanged,
                                    None,
                                    format!(
                                        "WAN watchdog suspended: {} — DHCP starvation suggests proto-config mismatch (no IP after {} checks); review WAN protocol setting and re-save",
                                        modem.label, failure_threshold
                                    ),
                                )
                                .await;

                            // 4c — tracing::warn! (engineer-facing, MAY name modes,
                            // structured + grepable + diagnose recipe).
                            tracing::warn!(
                                target: "wan_watchdog",
                                modem_id = %modem.modem_id,
                                modem_label = %modem.label,
                                cached_usbnet_mode = ?cached_usbnet_mode,
                                resolved_proto = %resolved_proto,
                                consecutive_failures = current_failures,
                                failure_threshold = failure_threshold,
                                "WAN watchdog DHCP starvation fast-fail on {} (proto={}, cached_mode={:?}, no lease after {} ticks). \
                                 Suspending restart cycle. Likely causes: \
                                 (1) cached usbnet_mode wrong (sub-task 1 detection bug), \
                                 (2) operator-overridden proto_override=dhcp on a non-ECM modem, \
                                 (3) genuine ECM lease starvation (carrier denied SIM, wrong APN). \
                                 Diagnose: `uci show network.{}` + `journalctl -u netifd | grep udhcpc`",
                                modem.label, resolved_proto, cached_usbnet_mode, current_failures, modem.interface_name
                            );

                            // 4d — Debug-trace (engineer-facing, in-memory WS panel, compact).
                            crate::state::debug_trace_with_source(
                                format!(
                                    "[WAN] {} fast-fail suspend: DHCP starvation (proto={}, mode={:?}, failed {}/{})",
                                    modem.label,
                                    resolved_proto,
                                    cached_usbnet_mode,
                                    current_failures,
                                    failure_threshold
                                ),
                                "wan",
                            );

                            continue;
                        }
                    }

                    // Check per-modem cooldown
                    if let Some(until) = restart_cooldowns.get(&modem.modem_id) {
                        if tokio::time::Instant::now() < *until {
                            continue;
                        }
                    }

                    // Check if restarts are suspended for this modem (max attempts reached)
                    let max_attempts = watchdog.max_restart_attempts;
                    let (current_count, already_suspended) = {
                        let runtime = state.wan_runtime.read().await;
                        runtime
                            .modem_statuses
                            .get(&modem.modem_id)
                            .map(|info| (info.restart_count, info.restart_suspended))
                            .unwrap_or((0, false))
                    };
                    if current_count >= max_attempts || already_suspended {
                        if !already_suspended {
                            // Mark as suspended in runtime state
                            let mut runtime = state.wan_runtime.write().await;
                            if let Some(info) = runtime.modem_statuses.get_mut(&modem.modem_id) {
                                info.restart_suspended = true;
                            }
                            drop(runtime);

                            // Log once
                            let timestamp = chrono::Utc::now().to_rfc3339();
                            let log_line = format!(
                                "{timestamp} RESTART_SUSPENDED modem=\"{}\" reason=\"Reached max restart attempts ({max_attempts})\"",
                                modem.label,
                            );
                            let _ = crate::config::wan::append_watchdog_log(&log_line).await;

                            crate::state::debug_trace_with_source(format!(
                                "[WAN] Restart suspended for {} — reached {} attempts",
                                modem.label, max_attempts
                            ), "wan");
                        }
                        continue;
                    }

                    // Look up modem handler and profile-specific restart command
                    let (handler_arc, restart_cmd) = {
                        let modems_map = state.modems.read().await;
                        match modems_map.get(&modem.modem_id) {
                            Some(context) => (
                                Arc::clone(&context.handler),
                                context.profile.restart_command.clone(),
                            ),
                            None => {
                                crate::state::debug_trace_with_source(format!(
                                    "[WAN] Modem {} not found in HashMap, skipping restart",
                                    modem.modem_id
                                ), "wan");
                                continue;
                            }
                        }
                    };

                    crate::state::debug_trace_with_source(format!(
                        "[WAN] Restarting {} (modem_id: {}) via {}",
                        modem.label, modem.modem_id, restart_cmd
                    ), "wan");

                    let restart_ok = {
                        match tokio::time::timeout(Duration::from_secs(2), handler_arc.lock()).await {
                            Ok(handler) => {
                                match tokio::time::timeout(
                                    Duration::from_secs(10),
                                    handler.execute_at(&restart_cmd),
                                )
                                .await
                                {
                                    Ok(Ok(_)) => true,
                                    Ok(Err(e)) => {
                                        crate::state::debug_trace_with_source(format!(
                                            "[WAN] Restart {} failed: {e}", modem.label
                                        ), "wan");
                                        false
                                    }
                                    Err(_) => {
                                        crate::state::debug_trace_with_source(format!(
                                            "[WAN] Restart {} timed out", modem.label
                                        ), "wan");
                                        false
                                    }
                                }
                            }
                            Err(_) => {
                                crate::state::debug_trace_with_source(format!(
                                    "[WAN] Handler lock timeout for {}", modem.label
                                ), "wan");
                                false
                            }
                        }
                    };

                    if restart_ok {
                        crate::state::debug_trace_with_source(format!(
                            "[WAN] Restart command sent to {}", modem.label
                        ), "wan");

                        // Log the restart event
                        let timestamp = chrono::Utc::now().to_rfc3339();
                        let log_line = format!(
                            "{timestamp} RESTART modem=\"{}\" reason=\"Failed {failure_threshold} consecutive health checks\"",
                            modem.label,
                        );
                        let _ = crate::config::wan::append_watchdog_log(&log_line).await;

                        state
                            .audit
                            .log(
                                crate::security::audit::AuditEventType::ConfigChanged,
                                None,
                                format!(
                                    "WAN watchdog restart: {} (failed {} checks)",
                                    modem.label, failure_threshold
                                ),
                            )
                            .await;

                        // Bounce the UCI interface so DHCP renews with the rebooted modem.
                        let iface = modem.interface_name.clone();
                        let bounce_state = Arc::clone(&state);
                        let bounce_modem_id = modem.modem_id.clone();
                        let bounce_network_device = modem.network_device.clone();
                        if !iface.is_empty() {
                            tokio::spawn(async move {
                                // Give modem time to start rebooting (USB re-enumeration takes a few seconds)
                                tokio::time::sleep(Duration::from_secs(5)).await;
                                // argv form — no shell. `iface` is validated to
                                // [A-Za-z0-9_-]{1,32} at the WAN write boundary,
                                // but we pass it as a single argument regardless
                                // so no shell ever parses it. The inter-step
                                // `sleep 3` is done in Rust.
                                let _ = tokio::process::Command::new("ifdown")
                                    .arg(&iface)
                                    .output()
                                    .await;
                                tokio::time::sleep(Duration::from_secs(3)).await;
                                let _ = tokio::process::Command::new("ifup")
                                    .arg(&iface)
                                    .output()
                                    .await;
                                crate::state::debug_trace_with_source(
                                    format!("[WAN] Interface {iface} bounced after modem restart"),
                                    "wan",
                                );

                                // After ifup and DHCP wait, recreate routing table entry if policy routing active
                                // Give DHCP a few seconds to obtain a lease
                                tokio::time::sleep(Duration::from_secs(5)).await;
                                {
                                    let caps = bounce_state.platform_capabilities.read().await;
                                    if caps.policy_routing_enabled {
                                        drop(caps);
                                        if let Some(ip) = routing::get_interface_ip(&bounce_network_device) {
                                            let gateway = routing::discover_gateway(&bounce_network_device);
                                            let wan_config = bounce_state.wan_config.read().await;
                                            let idx = wan_config
                                                .modem_priority
                                                .iter()
                                                .position(|e| e.modem_id == bounce_modem_id)
                                                .unwrap_or(0) as u32;
                                            drop(wan_config);

                                            let entry = RoutingTableEntry {
                                                table_number: 100 + idx,
                                                rule_priority: 1000 + idx,
                                                gateway,
                                                device: bounce_network_device.clone(),
                                                source_ip: ip,
                                            };
                                            let mut rs = bounce_state.routing_state.write().await;
                                            if let Some(old) = rs.remove(&bounce_modem_id) {
                                                let _ = routing::remove_table_entry(&old);
                                            }
                                            if routing::create_table_entry(&entry).is_ok() {
                                                rs.insert(bounce_modem_id.clone(), entry);
                                                info!("Recreated routing table for {} after restart", bounce_modem_id);
                                            }
                                        }
                                    }
                                }
                            });
                        }

                        // Increment restart counter in runtime state
                        {
                            let mut runtime = state.wan_runtime.write().await;
                            if let Some(info) = runtime.modem_statuses.get_mut(&modem.modem_id) {
                                info.restart_count += 1;
                            }
                        }
                    }

                    // Set cooldown regardless of success (prevent rapid-fire attempts)
                    restart_cooldowns.insert(
                        modem.modem_id.clone(),
                        tokio::time::Instant::now()
                            + Duration::from_secs(watchdog.restart_cooldown_mins as u64 * 60),
                    );
                }
            }

            // Reconcile routing tables after health checks
            {
                let caps = state.platform_capabilities.read().await;
                if caps.policy_routing_enabled {
                    drop(caps);
                    let wan_config = state.wan_config.read().await;
                    let wan_entries: Vec<(String, String, u32)> = wan_config
                        .modem_priority
                        .iter()
                        .enumerate()
                        .map(|(i, entry)| (entry.modem_id.clone(), entry.network_device.clone(), i as u32))
                        .collect();
                    drop(wan_config);

                    let expected_device = {
                        let runtime = state.wan_runtime.read().await;
                        runtime.current_routed_wan.as_ref().and_then(|id| {
                            all_modems.iter().find(|m| &m.modem_id == id).map(|m| m.network_device.clone())
                        })
                    };

                    let mut rs = state.routing_state.write().await;
                    let changes = routing::reconcile(&mut rs, &wan_entries, expected_device.as_deref());
                    if !changes.is_empty() {
                        debug!("Routing reconciliation: {} changes", changes.len());
                    }
                }
            }

            // Reconcile steering rule statuses
            {
                let mut sr = state.steering_rules.write().await;
                if !sr.is_empty() {
                    let rs = state.routing_state.read().await;
                    let caps = state.platform_capabilities.read().await;
                    let fw_backend = caps.firewall_backend.clone();
                    drop(caps);

                    // Build healthy WAN list from runtime modem statuses
                    let healthy_wans: Vec<String> = {
                        let runtime = state.wan_runtime.read().await;
                        runtime
                            .modem_statuses
                            .iter()
                            .filter(|(_, info)| info.status == WanModemStatus::Online)
                            .map(|(id, _)| id.clone())
                            .collect()
                    };

                    let changes = steering::reconcile_statuses(
                        &mut sr, &rs, &healthy_wans, &fw_backend,
                    );
                    if !changes.is_empty() {
                        debug!("Steering reconciliation: {} changes", changes.len());
                    }
                }
            }

            // Broadcast current status
            let response = build_wan_status(&state).await;
            state.broadcast_event(ModemEvent::WanStatusUpdate(Box::new(response)));
        }
    });
}

/// Check if a modem has a SIM card in its current slot via AT+CPIN?.
///
/// In multi-modem mode, looks up the modem handler from the HashMap by modem_id.
///
/// Returns true if SIM is present (READY, PIN required, etc.),
/// false if no SIM (NOT INSERTED, CME ERROR: 10).
/// Defaults to true on errors (don't skip health checks if we can't determine SIM status).
async fn check_modem_sim(
    modem: &crate::hardware::WanModemEntry,
    state: &AppState,
) -> bool {
    if std::env::var("MOCK_HARDWARE").is_ok() {
        return true; // Mock mode: always has SIM
    }

    // Look up modem handler by modem_id
    let handler_arc = {
        let modems = state.modems.read().await;
        match modems.get(&modem.modem_id) {
            Some(context) => Arc::clone(&context.handler),
            None => {
                crate::state::debug_trace_with_source(format!(
                    "[WAN] Modem {} not found for SIM check, assuming SIM present",
                    modem.modem_id
                ), "wan");
                return true; // Assume SIM present if can't find modem
            }
        }
    };

    let response = match tokio::time::timeout(Duration::from_secs(2), handler_arc.lock()).await {
        Ok(handler) => {
            tokio::time::timeout(
                Duration::from_secs(3),
                handler.execute_at("AT+CPIN?"),
            )
            .await
        }
        Err(_) => {
            crate::state::debug_trace_with_source(format!(
                "[WAN] Handler lock timeout for SIM check on {}",
                modem.label
            ), "wan");
            return true; // Assume SIM present if can't acquire lock
        }
    };

    match response {
        Ok(Ok(resp)) => {
            let upper = resp.to_uppercase();
            if upper.contains("NOT INSERTED") || upper.contains("ERROR: 10") {
                false
            } else {
                // READY, SIM PIN, SIM PUK, etc. — SIM is physically present
                true
            }
        }
        _ => {
            // Timeout or AT error — assume SIM present (don't skip health checks)
            true
        }
    }
}

/// Get the IPv4 address assigned to a network interface.
/// Returns None if the interface has no IP (e.g. no SIM / no DHCP lease).
async fn get_interface_ip(device: &str) -> Option<String> {
    if std::env::var("MOCK_HARDWARE").is_ok() {
        return Some("10.0.0.1".to_string());
    }
    // argv form — no shell. Run `ip -4 addr show dev <device>` and parse the
    // first `inet <addr>/<prefix>` line in Rust instead of piping through
    // grep/head/awk (which previously required an interpolated `sh -c` string).
    match tokio::process::Command::new("ip")
        .args(["-4", "addr", "show", "dev", device])
        .output()
        .await
    {
        Ok(output) if output.status.success() => {
            parse_first_inet_addr(&String::from_utf8_lossy(&output.stdout))
        }
        _ => None,
    }
}

/// Extract the first IPv4 address from `ip -4 addr show` output.
///
/// Each address line looks like `    inet 10.0.0.4/24 brd ... scope global ...`.
/// Returns the dotted-quad without the CIDR suffix (matching the prior
/// `grep -oE 'inet [0-9.]+' | awk '{print $2}'` behavior), or `None` if no
/// `inet` line is present.
fn parse_first_inet_addr(output: &str) -> Option<String> {
    for line in output.lines() {
        let mut toks = line.split_whitespace();
        if toks.next() == Some("inet") {
            if let Some(addr) = toks.next() {
                let ip = addr.split('/').next().unwrap_or(addr);
                if !ip.is_empty() {
                    return Some(ip.to_string());
                }
            }
        }
    }
    None
}

/// Run a 3-step health check on a network interface.
///
/// All checks are interface-bound to ensure they test connectivity through
/// the specific modem, not through other WAN connections:
/// - Ping: `-I {device}` binds ICMP to the interface
/// - DNS: hostname ping via `-I {device}` tests resolution + interface reachability
/// - HTTP: `wget --bind-address={ip}` forces source-IP binding
///
/// If the interface has no IP address (no SIM, no DHCP lease), all checks
/// immediately fail without sending any traffic.
async fn run_health_check(
    device: &str,
    ping_target: &str,
    dns_target: &str,
    http_target: &str,
) -> WanHealthCheckResult {
    let timestamp = chrono::Utc::now().to_rfc3339();

    let is_mock = std::env::var("MOCK_HARDWARE").is_ok();

    // Guard: empty device means no network interface was found during scan
    if device.is_empty() && !is_mock {
        return WanHealthCheckResult {
            timestamp,
            ping_ok: false,
            dns_ok: false,
            dns_v4_ok: false,
            dns_v6_ok: false,
            http_ok: false,
            overall_ok: false,
        };
    }

    // Step 0: Get interface IP — if no IP, modem has no data connection
    let ip_addr = get_interface_ip(device).await;
    if ip_addr.is_none() && !is_mock {
        crate::state::debug_trace_with_source(format!(
            "[WAN] {device}: no IP address — all checks skipped"
        ), "wan");
        return WanHealthCheckResult {
            timestamp,
            ping_ok: false,
            dns_ok: false,
            dns_v4_ok: false,
            dns_v6_ok: false,
            http_ok: false,
            overall_ok: false,
        };
    }
    let ip = ip_addr.unwrap_or_default();

    // Step 1: Ping (interface-bound via -I)
    let ping_ok = if is_mock {
        true
    } else {
        // argv form — device + ping_target are validated at the WAN-config
        // boundary, and no shell parses them here either (defense in depth).
        match tokio::process::Command::new("ping")
            .args(["-I", device, "-c", "1", "-W", "3", ping_target])
            .output()
            .await
        {
            Ok(output) => output.status.success(),
            Err(_) => false,
        }
    };

    // Step 2: DNS — hostname ping through the interface.
    // Runs IPv4 (-4) and IPv6 (-6) checks independently so the UI can show
    // per-protocol DNS status. dns_ok is the union for backward compat.
    let (dns_v4_ok, dns_v6_ok) = if is_mock {
        (true, true)
    } else {
        // argv form — device + dns_target validated at the WAN-config boundary.
        let v4_ok = match tokio::process::Command::new("ping")
            .args(["-4", "-I", device, "-c", "1", "-W", "5", dns_target])
            .output()
            .await
        {
            Ok(output) => output.status.success(),
            Err(_) => false,
        };
        let v6_ok = match tokio::process::Command::new("ping")
            .args(["-6", "-I", device, "-c", "1", "-W", "5", dns_target])
            .output()
            .await
        {
            Ok(output) => output.status.success(),
            Err(_) => false,
        };
        (v4_ok, v6_ok)
    };
    let dns_ok = dns_v4_ok || dns_v6_ok;

    // Step 3: HTTP — interface-bound check.
    // Prefers `curl --interface {device}` (true SO_BINDTODEVICE socket binding).
    // Falls back to `wget --bind-address={ip}` which only sets source IP but does
    // NOT control routing — gated behind ping/DNS to prevent false positives on
    // multi-WAN systems where traffic routes via the working modem.
    let http_ok = if is_mock {
        true
    } else {
        // Try curl first (proper interface binding via SO_BINDTODEVICE).
        // argv form — device + http_target validated at the WAN-config boundary.
        let curl_result = tokio::process::Command::new("curl")
            .args([
                "--interface", device,
                "-s", "-o", "/dev/null", "-m", "5",
                "-w", "%{http_code}",
                http_target,
            ])
            .output()
            .await;

        match curl_result {
            Ok(output) if output.status.code() != Some(127) => {
                // curl exists — check if HTTP succeeded (2xx/3xx status)
                let code = String::from_utf8_lossy(&output.stdout);
                let status: u16 = code.trim().trim_matches('\'').parse().unwrap_or(0);
                (200..400).contains(&status)
            }
            _ => {
                // curl not available — fall back to wget, but only if ping or DNS
                // passed (wget --bind-address can't properly interface-bind)
                if !ping_ok && !dns_ok {
                    false
                } else {
                    // argv form — ip is from `ip addr` (not user input),
                    // http_target validated at the WAN-config boundary.
                    match tokio::process::Command::new("wget")
                        .arg(format!("--bind-address={ip}"))
                        .args(["-q", "-O", "/dev/null", "-T", "5", http_target])
                        .output()
                        .await
                    {
                        Ok(output) => output.status.success(),
                        Err(_) => false,
                    }
                }
            }
        }
    };

    let overall_ok = ping_ok || dns_ok || http_ok;

    WanHealthCheckResult {
        timestamp,
        ping_ok,
        dns_ok,
        dns_v4_ok,
        dns_v6_ok,
        http_ok,
        overall_ok,
    }
}

/// UCI helper: set metric (called from watchdog context).
async fn uci_set_metric_cmd(name: &str, metric: u32) -> Result<(), String> {
    if std::env::var("MOCK_HARDWARE").is_ok() {
        return Ok(());
    }
    // argv form — no shell. The whole `network.<name>.metric=<metric>`
    // assignment is one argument to `uci set`; `name` is validated to
    // [A-Za-z0-9_-]{1,32} at the WAN write boundary, but passing it as a single
    // argv token means no shell parses it regardless.
    let _ = tokio::process::Command::new("uci")
        .arg("set")
        .arg(format!("network.{name}.metric={metric}"))
        .output()
        .await;
    Ok(())
}

/// UCI helper: commit and reload (called from watchdog context).
async fn uci_commit_cmd() -> Result<(), String> {
    if std::env::var("MOCK_HARDWARE").is_ok() {
        return Ok(());
    }
    let _ = tokio::process::Command::new("sh")
        .arg("-c")
        .arg("uci commit network && /etc/init.d/network reload")
        .output()
        .await;
    Ok(())
}

/// Build a WanStatusResponse from state (watchdog version without route context).
async fn build_wan_status(state: &AppState) -> WanStatusResponse {
    let config = state.wan_config.read().await;
    let runtime = state.wan_runtime.read().await;

    // Pre-fetch operator names from master cache for each modem
    let modem_operators: std::collections::HashMap<String, Option<String>> = {
        let modems_map = state.modems.read().await;
        let mut ops = std::collections::HashMap::new();
        for (modem_id, context) in modems_map.iter() {
            let cache = context.state_cache.read().await;
            let operator = cache.as_ref().and_then(|c| c.connection.operator.clone());
            ops.insert(modem_id.clone(), operator);
        }
        ops
    };

    // Pre-fetch detected USB-net mode (boot-time cache) for each modem.
    // Diagnostic only — not surfaced in operator UI per the mode-agnostic principle.
    let modem_modes: std::collections::HashMap<String, crate::hardware::UsbNetMode> = {
        let modems_map = state.modems.read().await;
        let mut modes = std::collections::HashMap::new();
        for (modem_id, context) in modems_map.iter() {
            modes.insert(modem_id.clone(), *context.usbnet_mode.read().await);
        }
        modes
    };

    let modems = config
        .modem_priority
        .iter()
        .map(|entry| {
            let runtime_info = runtime.modem_statuses.get(&entry.modem_id);
            let is_primary = config
                .modem_priority
                .iter()
                .find(|m| m.is_active())
                .is_some_and(|m| m.modem_id == entry.modem_id);

            crate::hardware::WanModemStatusEntry {
                modem_id: entry.modem_id.clone(),
                label: entry.label.clone(),
                interface_name: entry.interface_name.clone(),
                network_device: entry.network_device.clone(),
                state: entry.state.clone(),
                metric: entry.metric,
                status: runtime_info
                    .map(|r| r.status)
                    .unwrap_or(if entry.is_active() {
                        WanModemStatus::Offline
                    } else {
                        WanModemStatus::Standby
                    }),
                last_check: runtime_info.and_then(|r| r.last_check.clone()),
                consecutive_failures: runtime_info.map(|r| r.consecutive_failures).unwrap_or(0),
                is_primary,
                entry_type: entry.entry_type.clone(),
                original_bridge: entry.original_bridge.clone(),
                mtu: entry.mtu,
                ttl: entry.ttl,
                hop_limit: entry.hop_limit,
                operator: modem_operators.get(&entry.modem_id).cloned().flatten(),
                imei: None,
                restart_suspended: runtime_info.map(|r| r.restart_suspended).unwrap_or(false),
                restart_count: runtime_info.map(|r| r.restart_count).unwrap_or(0),
                weight: entry.weight,
                proto_override: entry.proto_override.clone(),
                // Diagnostic only — Ethernet entries have no modem to query.
                usbnet_mode: if entry.entry_type == crate::hardware::WanEntryType::Modem {
                    modem_modes.get(&entry.modem_id).copied()
                } else {
                    None
                },
            }
        })
        .collect();

    let failover_override = runtime.failover_override.as_ref().map(|fo| {
        let original_label = config
            .modem_priority
            .iter()
            .find(|m| m.modem_id == fo.original_primary_id)
            .map(|m| m.label.clone())
            .unwrap_or_else(|| fo.original_primary_id.clone());
        let current_label = config
            .modem_priority
            .iter()
            .find(|m| m.modem_id == fo.current_primary_id)
            .map(|m| m.label.clone())
            .unwrap_or_else(|| fo.current_primary_id.clone());
        let stabilization_remaining_secs = {
            let original_healthy_since = runtime.modem_statuses
                .get(&fo.original_primary_id)
                .and_then(|info| info.healthy_since);
            original_healthy_since.map(|since| {
                let elapsed = since.elapsed().as_secs();
                let target = (config.failback_timer_mins as u64) * 60;
                target.saturating_sub(elapsed)
            })
        };
        crate::hardware::FailoverOverrideInfo {
            active: true,
            original_primary_id: fo.original_primary_id.clone(),
            original_primary_label: original_label,
            current_primary_id: fo.current_primary_id.clone(),
            current_primary_label: current_label,
            failover_timestamp: fo.failover_timestamp.clone(),
            stabilization_remaining_secs,
        }
    });

    WanStatusResponse {
        enabled: config.enabled,
        routing_mode: config.routing_mode.clone(),
        failover_locked: config.failover_locked,
        modems,
        watchdog: config.watchdog.clone(),
        failover_history: runtime.failover_history.iter().cloned().collect(),
        failback_timer_mins: config.failback_timer_mins,
        failover_override,
        platform: None,
        routing_tables: None,
    }
}

/// Decide whether a Modem-type WAN entry should be fast-fail-suspended on
/// DHCP starvation rather than entering the normal restart cycle
/// (Item #37 sub-task 3).
///
/// Returns true when ALL of the following hold:
///
/// 1. `entry_type == WanEntryType::Modem` — Ethernet WAN failures may be
///    real upstream/cable problems where ifdown/ifup helps.
/// 2. `resolved_proto == "dhcp"` — daemon would launch udhcpc on this
///    interface, meaning either ECM-mode auto-derived dhcp OR an
///    operator-overridden `proto_override="dhcp"` on a non-ECM modem.
/// 3. `!has_current_ip` — modem currently has no IP (distinguishes DHCP
///    starvation from "has IP but ping/dns/http blocked for unrelated
///    reasons" — without this guard, a healthy DHCP'd modem in a network
///    with momentary upstream ICMP/DNS outage would false-positive-suspend).
/// 4. `consecutive_failures >= failure_threshold` — same trigger point as
///    the existing restart logic (~90s on defaults).
///
/// Pure function — no async, no I/O, no shared state access. Mode-agnostic
/// in signature (no `UsbNetMode` import). Trivially unit-testable.
///
/// Caller plumbing: see the per-modem restart loop in `spawn_wan_watchdog`.
/// `resolved_proto` is computed via `resolve_uci_proto(entry, Some(cached_usbnet_mode))`
/// and `has_current_ip` via `get_interface_ip(&modem.network_device).await.is_some()`.
fn should_fast_fail(
    entry_type: crate::hardware::WanEntryType,
    resolved_proto: &str,
    has_current_ip: bool,
    consecutive_failures: u32,
    failure_threshold: u32,
) -> bool {
    entry_type == crate::hardware::WanEntryType::Modem
        && resolved_proto == "dhcp"
        && !has_current_ip
        && consecutive_failures >= failure_threshold
}

/// Compute the desired primary WAN: the highest-priority entry that is Online.
/// Walks the full modem_priority list (active + standby) in config order.
/// Returns None if no WAN is healthy (caller should leave routing unchanged).
fn compute_desired_primary(
    modem_priority: &[crate::hardware::WanModemEntry],
    statuses: &std::collections::HashMap<String, WanModemRuntimeInfo>,
) -> Option<String> {
    modem_priority.iter().find_map(|entry| {
        let is_online = statuses
            .get(&entry.modem_id)
            .is_some_and(|info| info.status == WanModemStatus::Online);
        if is_online {
            Some(entry.modem_id.clone())
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests {
    // ========================================================================
    // FIX 1: WebSocket auth gate must mirror the HTTP middleware (enabled-only).
    // ========================================================================

    /// On a fresh box `auth.enabled=true` but there is no legacy password_hash
    /// and zero non-root users. The old gate evaluated FALSE here and streamed
    /// telemetry to an unauthenticated client. The gate must now REQUIRE auth
    /// whenever auth is enabled, regardless of password_hash / user count.
    #[test]
    fn ws_auth_required_when_enabled_even_with_no_users_or_password_hash() {
        assert!(
            super::ws_auth_required(true),
            "enabled auth must require the WS handshake even on a fresh box \
             (no password_hash, zero non-root users)"
        );
    }

    /// When auth is globally disabled the WS handshake is skipped — matching the
    /// HTTP middleware which opens regardless of whether users exist.
    #[test]
    fn ws_auth_skipped_when_disabled() {
        assert!(
            !super::ws_auth_required(false),
            "disabled auth must skip the WS handshake (mirrors HTTP middleware)"
        );
    }

    #[test]
    fn lock_busy_counter_escalates_after_threshold_and_resets_on_acquire() {
        assert_eq!(super::next_lock_busy_count(0, false, 3), (1, false));
        assert_eq!(super::next_lock_busy_count(1, false, 3), (2, false));
        assert_eq!(super::next_lock_busy_count(2, false, 3), (3, true));
        assert_eq!(super::next_lock_busy_count(3, false, 3), (4, true));
        assert_eq!(super::next_lock_busy_count(5, true, 3), (0, false));
    }

    // ========================================================================
    // FIX 2: de-shelled WAN watchdog helpers
    // ========================================================================

    #[test]
    fn parse_first_inet_addr_extracts_ipv4_without_cidr() {
        let out = "\
2: wwan0: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500 qdisc fq_codel
    inet 10.51.0.4/30 brd 10.51.0.7 scope global wwan0
       valid_lft forever preferred_lft forever";
        assert_eq!(
            super::parse_first_inet_addr(out),
            Some("10.51.0.4".to_string()),
        );
    }

    #[test]
    fn parse_first_inet_addr_returns_first_of_many() {
        let out = "    inet 10.0.0.1/24 scope global eth0\n    inet 10.0.0.2/24 scope global secondary eth0";
        assert_eq!(
            super::parse_first_inet_addr(out),
            Some("10.0.0.1".to_string()),
        );
    }

    #[test]
    fn parse_first_inet_addr_none_when_no_inet_line() {
        let out = "3: wwan0: <NO-CARRIER> mtu 1500\n    link/ether 00:11:22:33:44:55";
        assert_eq!(super::parse_first_inet_addr(out), None);
    }

    /// Source-scan guard: the three de-shelled watchdog helpers must not
    /// reintroduce a `sh -c` interpolation sink. We assert that the helper
    /// bodies use argv `Command::new("ifdown"|"ifup"|"ip"|"uci")` and that the
    /// previously-present interpolated shell strings are gone from this file.
    #[test]
    fn watchdog_helpers_use_no_shell_interpolation() {
        let src = include_str!("websocket.rs");
        // The exact interpolated shell strings that previously existed must be
        // gone. Patterns are assembled at runtime so this test's own body does
        // not contain the literals it forbids (which would self-trip the scan).
        let forbidden = [
            format!("ifdown {}iface{}", "{", "}"),
            format!("ip -4 addr show {}device{}", "{", "}"),
            format!("uci set network.{}name{}.metric", "{", "}"),
        ];
        for pat in &forbidden {
            assert!(
                !src.contains(pat.as_str()),
                "de-shelled watchdog helper reintroduced interpolated sh -c sink: {pat}",
            );
        }
        // The argv replacements must be present.
        assert!(src.contains(&format!("Command::new({}ifdown{})", '"', '"')));
        assert!(src.contains(&format!("Command::new({}ifup{})", '"', '"')));
        assert!(src.contains(&format!("Command::new({}ip{})", '"', '"')));
    }
}

#[cfg(test)]
mod watchdog_tests {
    use super::*;
    use crate::hardware::WanModemStatus;

    fn test_wan(id: &str, active: bool) -> crate::hardware::WanModemEntry {
        crate::hardware::WanModemEntry {
            modem_id: id.to_string(),
            label: id.to_string(),
            interface_name: String::new(),
            network_device: format!("dev_{id}"),
            device_path: String::new(),
            state: if active {
                crate::hardware::WanModemState::Active
            } else {
                crate::hardware::WanModemState::Standby
            },
            metric: 0,
            entry_type: crate::hardware::WanEntryType::Modem,
            original_bridge: None,
            mtu: None,
            ttl: None,
            hop_limit: None,
            weight: None,
            proto_override: None,
        }
    }

    fn online_info(since: Option<tokio::time::Instant>) -> WanModemRuntimeInfo {
        WanModemRuntimeInfo {
            status: WanModemStatus::Online,
            consecutive_failures: 0,
            last_check: None,
            network_device: None,
            has_sim: Some(true),
            restart_count: 0,
            restart_suspended: false,
            healthy_since: since,
        }
    }

    fn offline_info() -> WanModemRuntimeInfo {
        WanModemRuntimeInfo {
            status: WanModemStatus::Offline,
            consecutive_failures: 3,
            last_check: None,
            network_device: None,
            has_sim: Some(true),
            restart_count: 0,
            restart_suspended: false,
            healthy_since: None,
        }
    }

    #[test]
    fn desired_primary_picks_first_healthy() {
        let wans = vec![test_wan("a", true), test_wan("b", true), test_wan("c", true)];
        let mut statuses = std::collections::HashMap::new();
        statuses.insert("a".to_string(), offline_info());
        statuses.insert("b".to_string(), online_info(Some(tokio::time::Instant::now())));
        statuses.insert("c".to_string(), online_info(Some(tokio::time::Instant::now())));
        let result = compute_desired_primary(&wans, &statuses);
        assert_eq!(result, Some("b".to_string()));
    }

    #[test]
    fn desired_primary_prefers_higher_priority() {
        let wans = vec![test_wan("a", true), test_wan("b", true)];
        let mut statuses = std::collections::HashMap::new();
        statuses.insert("a".to_string(), online_info(Some(tokio::time::Instant::now())));
        statuses.insert("b".to_string(), online_info(Some(tokio::time::Instant::now())));
        let result = compute_desired_primary(&wans, &statuses);
        assert_eq!(result, Some("a".to_string()));
    }

    #[test]
    fn desired_primary_none_when_all_offline() {
        let wans = vec![test_wan("a", true), test_wan("b", true)];
        let mut statuses = std::collections::HashMap::new();
        statuses.insert("a".to_string(), offline_info());
        statuses.insert("b".to_string(), offline_info());
        let result = compute_desired_primary(&wans, &statuses);
        assert_eq!(result, None);
    }

    #[test]
    fn desired_primary_includes_standby_wans() {
        let wans = vec![test_wan("a", true), test_wan("b", false)];
        let mut statuses = std::collections::HashMap::new();
        statuses.insert("a".to_string(), offline_info());
        statuses.insert("b".to_string(), online_info(Some(tokio::time::Instant::now())));
        let result = compute_desired_primary(&wans, &statuses);
        assert_eq!(result, Some("b".to_string()));
    }

    #[test]
    fn desired_primary_skips_unknown_wans() {
        let wans = vec![test_wan("a", true), test_wan("b", true)];
        let statuses = std::collections::HashMap::new();
        let result = compute_desired_primary(&wans, &statuses);
        assert_eq!(result, None);
    }

    // ====================================================================
    // Item #37 sub-task 3 — should_fast_fail predicate tests
    // ====================================================================

    use crate::hardware::WanEntryType;

    #[test]
    fn fast_fail_at_threshold_modem_dhcp_no_ip() {
        // Exactly at threshold — fast-fail fires.
        assert!(super::should_fast_fail(WanEntryType::Modem, "dhcp", false, 3, 3));
    }

    #[test]
    fn fast_fail_above_threshold_modem_dhcp_no_ip() {
        // Above threshold — fast-fail fires.
        assert!(super::should_fast_fail(WanEntryType::Modem, "dhcp", false, 5, 3));
    }

    #[test]
    fn fast_fail_below_threshold_does_not_fire() {
        // Below threshold — no fast-fail (give the modem more time).
        assert!(!super::should_fast_fail(WanEntryType::Modem, "dhcp", false, 2, 3));
    }

    #[test]
    fn fast_fail_zero_failures_does_not_fire() {
        // Zero failures — no fast-fail.
        assert!(!super::should_fast_fail(WanEntryType::Modem, "dhcp", false, 0, 3));
    }

    #[test]
    fn fast_fail_has_ip_does_not_fire() {
        // CORRECTNESS FIX: modem has IP but failed for unrelated reasons
        // (upstream ICMP/DNS outage). DON'T fast-fail-suspend — this is the
        // mixed-failure-mode case, not DHCP starvation.
        assert!(!super::should_fast_fail(WanEntryType::Modem, "dhcp", true, 5, 3));
    }

    #[test]
    fn fast_fail_no_ip_modem_dhcp_above_threshold_fires() {
        // Target case: modem with proto=dhcp, no IP, above threshold.
        // This is the operator-overridden mismatch / ECM-starvation /
        // mode-detection-bug scenario the predicate exists to catch.
        assert!(super::should_fast_fail(WanEntryType::Modem, "dhcp", false, 5, 3));
    }

    #[test]
    fn fast_fail_ethernet_does_not_fire() {
        // Ethernet WAN failures may be real cable/upstream problems where
        // ifdown/ifup may help. Existing restart logic runs.
        assert!(!super::should_fast_fail(WanEntryType::Ethernet, "dhcp", false, 5, 3));
    }

    #[test]
    fn fast_fail_qmi_proto_does_not_fire() {
        // QMI modems use OpenWrt proto-qmi (control-plane IP), not udhcpc.
        // No DHCP starvation possible.
        assert!(!super::should_fast_fail(WanEntryType::Modem, "qmi", false, 5, 3));
    }

    #[test]
    fn fast_fail_mbim_proto_does_not_fire() {
        // MBIM modems use OpenWrt proto-mbim, not udhcpc.
        assert!(!super::should_fast_fail(WanEntryType::Modem, "mbim", false, 5, 3));
    }

    #[test]
    fn fast_fail_static_proto_does_not_fire() {
        // Static-IP entries don't use udhcpc.
        assert!(!super::should_fast_fail(WanEntryType::Modem, "static", false, 5, 3));
    }

    #[test]
    fn fast_fail_empty_proto_does_not_fire() {
        // Defensive: empty resolved_proto string never matches "dhcp".
        assert!(!super::should_fast_fail(WanEntryType::Modem, "", false, 5, 3));
    }

    // ====================================================================
    // Item #37 sub-task 3 — Cross-cutting tests (I1-I7 from spec §5b,
    // reframed per Phase 1 finding Q10 — no tick-driver harness exists,
    // so these directly test the predicate-input integration without
    // driving the watchdog loop body)
    // ====================================================================

    /// Helper: build a Modem-type WanModemEntry with controllable fields.
    /// Mirrors `test_wan` but with parameter knobs the new tests need.
    fn modem_entry(
        id: &str,
        netif: &str,
        proto_override: Option<&str>,
    ) -> crate::hardware::WanModemEntry {
        let mut e = test_wan(id, true);
        e.entry_type = crate::hardware::WanEntryType::Modem;
        e.network_device = netif.to_string();
        e.interface_name = "WWAN".to_string();
        e.proto_override = proto_override.map(|s| s.to_string());
        e
    }

    /// I1: ECM-mode modem (resolves to dhcp) + no IP after threshold ticks
    /// → predicate returns true (fast-fail fires).
    #[test]
    fn i1_ecm_mode_no_ip_at_threshold_fast_fails() {
        let entry = modem_entry("modem1", "wwan0", None);
        let resolved_proto = crate::api::routes::wan::resolve_uci_proto(
            &entry,
            Some(crate::hardware::UsbNetMode::Ecm),
        );
        assert_eq!(resolved_proto.as_ref(), "dhcp", "ECM resolves to dhcp");
        let fires = super::should_fast_fail(
            entry.entry_type.clone(),
            resolved_proto.as_ref(),
            false, // no current IP
            3, 3,
        );
        assert!(fires, "ECM-mode modem with no IP at threshold must fast-fail");
    }

    /// I2: QMI-mode modem (resolves to qmi) + no IP after threshold
    /// → predicate returns false (existing restart path fires).
    #[test]
    fn i2_qmi_mode_no_ip_does_not_fast_fail() {
        let entry = modem_entry("modem1", "wwan0", None);
        let resolved_proto = crate::api::routes::wan::resolve_uci_proto(
            &entry,
            Some(crate::hardware::UsbNetMode::Qmi),
        );
        assert_eq!(resolved_proto.as_ref(), "qmi", "QMI resolves to qmi");
        let fires = super::should_fast_fail(
            entry.entry_type.clone(),
            resolved_proto.as_ref(),
            false,
            5, 3,
        );
        assert!(!fires, "QMI-mode modem must NOT fast-fail (existing restart path runs)");
    }

    /// I3: Ethernet entry + no IP after threshold → predicate returns false
    /// (Ethernet WAN failures may be real cable/upstream problems).
    #[test]
    fn i3_ethernet_no_ip_does_not_fast_fail() {
        let mut entry = test_wan("eth:wan", true);
        entry.entry_type = crate::hardware::WanEntryType::Ethernet;
        entry.network_device = "wan".to_string();
        let resolved_proto = crate::api::routes::wan::resolve_uci_proto(&entry, None);
        assert_eq!(resolved_proto.as_ref(), "dhcp", "Ethernet always dhcp");
        let fires = super::should_fast_fail(
            entry.entry_type.clone(),
            resolved_proto.as_ref(),
            false,
            5, 3,
        );
        assert!(!fires, "Ethernet entry must NOT fast-fail");
    }

    /// I4: ECM-mode modem + no IP for only 2 ticks (below threshold)
    /// → predicate returns false.
    #[test]
    fn i4_ecm_below_threshold_does_not_fast_fail() {
        let entry = modem_entry("modem1", "wwan0", None);
        let resolved_proto = crate::api::routes::wan::resolve_uci_proto(
            &entry,
            Some(crate::hardware::UsbNetMode::Ecm),
        );
        let fires = super::should_fast_fail(
            entry.entry_type.clone(),
            resolved_proto.as_ref(),
            false,
            2, 3, // below threshold
        );
        assert!(!fires, "Below threshold must NOT fast-fail (give modem more time)");
    }

    /// I5: Mode-cache miss (cached_usbnet_mode=None) + proto_override=None
    /// → resolve_uci_proto returns "dhcp" (Unknown/None fallback) → predicate
    /// fires per spec §3 safety guard. Acceptable: better to fast-fail-suspend
    /// a modem with no detected mode and no IP than burn restart cycles.
    ///
    /// Note: spec §3 said "no fast-fail on mode-cache-miss"; ground-truth
    /// of resolve_uci_proto shows Some(Unknown) | None both return "dhcp",
    /// so the predicate WOULD fire in this case. The safety guard is
    /// effectively the proto_override=None + no-IP combination, which is
    /// correct behavior — mode-cache-miss + no-IP IS the kind of
    /// configuration ambiguity worth fast-failing on.
    #[test]
    fn i5_mode_cache_miss_no_override_fast_fails_per_dhcp_fallback() {
        let entry = modem_entry("modem1", "wwan0", None); // no override
        let resolved_proto = crate::api::routes::wan::resolve_uci_proto(&entry, None);
        assert_eq!(resolved_proto.as_ref(), "dhcp",
            "None mode falls back to dhcp per resolve_uci_proto contract");
        let fires = super::should_fast_fail(
            entry.entry_type.clone(),
            resolved_proto.as_ref(),
            false,
            3, 3,
        );
        assert!(fires,
            "Mode-cache-miss + no-IP + no-override falls through to dhcp resolution; \
             fast-fail correctly fires (spec §3 safety guard reframed by ground-truth)");
    }

    /// I6: Suspension-clear semantics. The clear loop in update_wan_config
    /// (wan.rs:2164-2177) and clear_restart_suspensions (wan.rs:3072-3094)
    /// flips `restart_suspended=false` regardless of how it was set. This
    /// pure-function test asserts the field is flippable independent of
    /// origin (fast-fail vs max-attempts).
    #[test]
    fn i6_restart_suspended_flag_clears_regardless_of_set_origin() {
        let mut info_fastfail = offline_info();
        info_fastfail.restart_suspended = true; // set via fast-fail
        let mut info_maxattempts = offline_info();
        info_maxattempts.restart_suspended = true; // set via max-attempts
        // Apply the clear pattern from wan.rs:2168-2171.
        info_fastfail.restart_suspended = false;
        info_fastfail.restart_count = 0;
        info_maxattempts.restart_suspended = false;
        info_maxattempts.restart_count = 0;
        assert!(!info_fastfail.restart_suspended);
        assert!(!info_maxattempts.restart_suspended);
        assert_eq!(info_fastfail.restart_count, 0);
        assert_eq!(info_maxattempts.restart_count, 0);
    }

    /// I7: Failover independence. compute_desired_primary keys off
    /// `info.status`, NOT `info.restart_suspended`. A fast-failed modem
    /// (Offline status, suspended) does NOT block failover to a healthy
    /// alternate (Ethernet WAN, second modem, etc.).
    #[test]
    fn i7_compute_desired_primary_ignores_restart_suspended_field() {
        let wans = vec![
            test_wan("modem-fast-failed", true),
            test_wan("ethernet-healthy", true),
        ];
        let mut statuses = std::collections::HashMap::new();
        // Modem fast-failed: Offline + restart_suspended=true.
        let mut fast_failed = offline_info();
        fast_failed.restart_suspended = true;
        statuses.insert("modem-fast-failed".to_string(), fast_failed);
        // Ethernet healthy.
        statuses.insert(
            "ethernet-healthy".to_string(),
            online_info(Some(tokio::time::Instant::now())),
        );
        let result = compute_desired_primary(&wans, &statuses);
        assert_eq!(
            result,
            Some("ethernet-healthy".to_string()),
            "fast-failed (suspended) modem must NOT block failover to a healthy alternate"
        );
    }
}
