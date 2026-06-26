//! Shared application state.
//!
//! AppState holds all modem contexts and configuration, shared across
//! all request handlers via Axum's State extractor.
//!
//! ## Concurrency Model (v1.0.0+)
//!
//! Each modem gets its own isolated context (ModemContext) with dedicated handler.
//! Hardware access per modem uses `Arc<Mutex<Box<dyn ModemHardware + Send>>>`.
//! Modem commands are inherently serial (can't read signal while connecting),
//! so we serialize hardware access through per-modem mutexes.
//!
//! The API layer wraps hardware calls with timeouts:
//! - 5s for quick queries
//! - 15s for state changes
//! - 60s for network scans

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
// AtomicU64 comes from portable-atomic: mips32 targets (mipsel_24kc) have no
// native 64-bit atomics; on 64-bit targets this is a zero-cost re-export.
use portable_atomic::AtomicU64;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::OnceLock;
use tokio::sync::{broadcast, Mutex, RwLock};

use crate::api::steering::SteeringRule;
use crate::hardware::{
    AllSimSlotConfig, AppConfig, ApnProfile, ConnectionConfig, DetectedModem, DiscoveryInfo,
    FailoverEvent, ModemEvent, ModemHardware, ModemHealth, ModemProfile, ModemStateCache,
    PlatformCapabilities, ProfileRegistry, RoutingTableEntry, SignalInfo, SignalSample,
    UsbNetMode, WanConfig, WanHealthCheckResult, WanModemStatus,
};
use crate::security::{
    AuditLog, LoginLockout, RateLimiter, SessionStore, UserStore, WhitelistOverrides, WsTokenStore,
};
use crate::security::device_auth::DeviceAuth;
use crate::security::license::LicenseState;

/// Wrapper for broadcast events that adds optional modem_id context.
/// The modem_id is injected into the JSON at serialization time in the
/// WebSocket send task, producing top-level { type, modem_id, payload }.
#[derive(Debug, Clone)]
pub struct BroadcastEvent {
    /// Modem that generated this event. None for global events (DebugTrace, WanStatusUpdate).
    pub modem_id: Option<String>,
    /// The underlying event.
    pub event: ModemEvent,
}

// Global debug trace sender — lets AT handler emit traces without AppState access.
static DEBUG_TX: OnceLock<broadcast::Sender<BroadcastEvent>> = OnceLock::new();

/// Emit a debug trace message to all WebSocket clients with a default source of "system".
/// Callable from anywhere (including AT handler's send_command).
#[allow(dead_code)] // Used by real-hardware feature (AT handler traces)
pub fn debug_trace(msg: impl Into<String>) {
    debug_trace_with_source(msg, "system");
}

/// Emit a debug trace message with an explicit source tag.
/// Source values: "manual", "cache", "discovery", "refresh", "antenna_live",
/// "network_scan", "reconnect", "apn", "system", "wan", "sim".
pub fn debug_trace_with_source(msg: impl Into<String>, source: &str) {
    if let Some(tx) = DEBUG_TX.get() {
        let _ = tx.send(BroadcastEvent {
            modem_id: None,
            event: ModemEvent::DebugTrace {
                message: msg.into(),
                source: source.to_string(),
            },
        });
    }
}

/// Per-modem context with isolated state.
///
/// Each physical modem gets its own ModemContext, providing complete isolation
/// between modems. The modem_id is stable across reboots (USB serial-based).
pub struct ModemContext {
    /// Unique modem identifier: {VID}:{PID}:{USB_SERIAL}
    /// Example: "2c7c:0122:e3183572"
    #[allow(dead_code)]
    pub id: String,

    /// AT command handler (scoped to this modem)
    pub handler: Arc<Mutex<Box<dyn ModemHardware + Send>>>,

    /// Modem profile (determines capabilities & commands)
    pub profile: Arc<ModemProfile>,

    /// Detection metadata (bus-port, VID:PID, etc)
    pub detected: DetectedModem,

    /// Per-modem health state
    pub health: Arc<RwLock<ModemHealth>>,

    /// Cached signal info (refreshed by broadcaster)
    pub last_signal: Arc<RwLock<Option<SignalInfo>>>,

    /// Per-modem connection config (APN, CID, etc)
    pub config: Arc<RwLock<ConnectionConfig>>,

    /// Boot-time discovery (device info + SIM status). Writable for SIM swap/refresh.
    pub discovery: Arc<RwLock<DiscoveryInfo>>,

    /// Master cache snapshot, refreshed every 60s. None until first refresh.
    pub state_cache: Arc<RwLock<Option<ModemStateCache>>>,

    /// Ring buffer of signal quality samples (up to 1440 = 24h at 60s intervals).
    pub signal_history: Arc<RwLock<VecDeque<SignalSample>>>,

    /// Detected USB-net mode (boot-time cache; never re-polled). Diagnostic only.
    /// Populated after `add_modem` by the caller; default is `UsbNetMode::Unknown`.
    /// Per `feedback_modem_mode_agnostic.md`, this value MUST NOT be surfaced in
    /// operator-facing UI — it exists for engineer-facing diagnostics only.
    pub usbnet_mode: Arc<RwLock<UsbNetMode>>,

    /// Shared handle to the live AT-port path the handler is actually using,
    /// extracted from the handler at `add_modem`/`replace_handler` time. `None`
    /// for handlers with no serial fd (the mock). The 60s cache task reads this
    /// and reconciles changes into `detected.device_path` + the matching
    /// `detected_modems` entry. A `std::sync::Mutex` (sync, brief lock-clone) —
    /// never held across an `.await`.
    pub live_device_path: Option<std::sync::Arc<std::sync::Mutex<String>>>,
}

/// Application state shared across all handlers.
pub struct AppState {
    /// All modems on the system, keyed by stable modem_id (USB serial-based).
    /// Each modem is completely isolated with its own handler, config, and state.
    pub modems: Arc<RwLock<HashMap<String, ModemContext>>>,

    /// All detected modems on the system (for detection/scanning operations).
    pub detected_modems: Arc<RwLock<Vec<DetectedModem>>>,

    /// Registry of all known modem profiles.
    pub profile_registry: Arc<ProfileRegistry>,

    /// Event broadcast channel for WebSocket clients.
    pub event_tx: broadcast::Sender<BroadcastEvent>,

    /// Persistent configuration.
    pub config: Arc<RwLock<AppConfig>>,

    /// Session store for authentication.
    pub sessions: Arc<SessionStore>,

    /// Short-lived single-use tokens for WebSocket auth.
    pub ws_tokens: Arc<WsTokenStore>,

    /// User account store.
    pub users: Arc<UserStore>,

    /// Per-IP rate limiter.
    pub rate_limiter: Arc<RateLimiter>,

    /// Per-account failed-login lockout (temporary exponential backoff).
    /// Complements the per-IP rate limiter: keyed on the submitted username so
    /// an attacker rotating IPs cannot get unlimited guesses against a single
    /// account. Self-clearing — never a permanent lock (root recovery preserved).
    pub login_lockout: Arc<LoginLockout>,

    /// Security audit log.
    pub audit: Arc<AuditLog>,

    /// Number of active WebSocket clients. Broadcaster skips polling when 0.
    pub ws_client_count: AtomicUsize,

    /// Runtime AT whitelist overrides, loaded from disk.
    pub at_whitelist_overrides: Arc<RwLock<WhitelistOverrides>>,

    /// Saved APN connection presets, loaded from disk.
    pub apn_profiles: Arc<RwLock<Vec<ApnProfile>>>,

    /// Per-modem SIM slot profile assignments for dual SIM, loaded from disk.
    pub sim_slot_config: Arc<RwLock<AllSimSlotConfig>>,

    /// WAN manager persistent configuration, loaded from disk.
    pub wan_config: Arc<RwLock<WanConfig>>,

    /// WAN manager runtime state (health check results, failure counts, failover history).
    pub wan_runtime: Arc<RwLock<WanRuntimeState>>,

    /// Platform routing capabilities detected at startup (ip rule, ip route support).
    pub platform_capabilities: Arc<RwLock<PlatformCapabilities>>,

    /// Active policy routing table entries, keyed by modem_id.
    pub routing_state: Arc<RwLock<HashMap<String, RoutingTableEntry>>>,

    /// Active traffic steering rules (Level 2), loaded from disk.
    pub steering_rules: Arc<RwLock<Vec<SteeringRule>>>,

    /// Currently selected modem ID for backward-compat single-modem API routes.
    /// Phase 1: Defaults to first modem, switchable via POST /api/modem/select.
    pub selected_modem_id: Arc<RwLock<Option<String>>>,

    /// Whether TLS is actually serving (not just configured).
    /// Used to decide Secure flag on session cookies.
    pub tls_active: std::sync::atomic::AtomicBool,

    /// Controls whether GPS is polled in cache refresh task.
    /// Frontend toggles via POST /api/gps/panel. Default: false.
    pub gps_panel_active: std::sync::atomic::AtomicBool,

    /// Whether the portal has enabled telemetry for this device.
    /// Updated from heartbeat response. Combined with config.telemetry_enabled
    /// to determine if telemetry should be collected.
    pub telemetry_portal_enabled: std::sync::atomic::AtomicBool,

    /// Current license verification state.
    pub license_state: Arc<RwLock<LicenseState>>,

    /// Hardware device token (computed once at startup).
    pub device_token: String,

    /// Per-device Ed25519 signing keypair (Item #3 — signed device auth).
    /// Generated/loaded once at startup; exposes the public key + key-id and can
    /// sign canonical request bytes. Phase 1 wires it in but no path uses it yet.
    #[allow(dead_code)]
    pub device_auth: Arc<DeviceAuth>,

    /// Portal-issued single-use replay nonce (Item #3 — signed device auth).
    /// Written from each heartbeat response (`next_nonce`) starting Phase 2;
    /// read by the Phase-3 request signer. In-memory only (no flash write).
    #[allow(dead_code)]
    pub device_nonce: Arc<RwLock<Option<String>>>,

    /// Telemetry snapshot buffer, filled by collector, drained by heartbeat.
    pub telemetry_buffer: crate::api::telemetry::TelemetryBuffer,

    /// Current heartbeat interval in seconds (can be changed by fast mode).
    /// Default: 1800 (30 minutes).
    pub heartbeat_interval_secs: AtomicU64,

    /// When fast mode should auto-revert (None = normal mode).
    pub fast_mode_until: Arc<RwLock<Option<tokio::time::Instant>>>,

    /// Whether a "poll now" one-shot is pending.
    pub poll_now_pending: AtomicBool,

    /// Shutdown signal for the tunnel client task.
    pub tunnel_shutdown: Arc<tokio::sync::Notify>,

    /// Speedtest result history (ring buffer, persisted to disk).
    pub speedtest_history: Arc<RwLock<crate::hardware::speedtest::SpeedtestHistory>>,

    /// Concurrency guard — only one speedtest may run at a time.
    pub speedtest_lock: Arc<tokio::sync::Mutex<()>>,

    /// Unsent speedtest results, drained by heartbeat.
    pub speedtest_buffer: Arc<RwLock<Vec<crate::hardware::SpeedtestResult>>>,
}

/// Tracks an active failover override (runtime only, not persisted).
pub struct FailoverOverride {
    /// The modem ID that the user configured as primary.
    pub original_primary_id: String,
    /// The modem ID currently handling traffic after failover.
    pub current_primary_id: String,
    /// ISO 8601 timestamp of when the failover happened.
    pub failover_timestamp: String,
}

/// Runtime WAN manager state (not persisted to disk).
#[derive(Default)]
pub struct WanRuntimeState {
    /// Per-modem health status keyed by modem_id (IMEI).
    pub modem_statuses: HashMap<String, WanModemRuntimeInfo>,
    /// Recent failover events (capped at 50).
    pub failover_history: VecDeque<FailoverEvent>,
    /// Active failover override — set when the watchdog triggers a failover,
    /// cleared on failback or manual acceptance.
    pub failover_override: Option<FailoverOverride>,
    /// Which WAN modem_id currently has the main default route.
    /// Updated by the watchdog loop and by manual accept/reject endpoints.
    #[allow(dead_code)]
    pub current_routed_wan: Option<String>,
}

/// Per-modem runtime health info.
pub struct WanModemRuntimeInfo {
    pub status: WanModemStatus,
    pub consecutive_failures: u32,
    pub last_check: Option<WanHealthCheckResult>,
    #[allow(dead_code)]
    pub network_device: Option<String>,
    /// SIM presence in the modem's current slot.
    /// None = not yet checked, Some(true) = SIM present, Some(false) = no SIM.
    pub has_sim: Option<bool>,
    /// Number of restart attempts by the watchdog.
    pub restart_count: u32,
    /// Whether watchdog restarts have been suspended (max attempts reached).
    pub restart_suspended: bool,
    /// When this WAN last transitioned to healthy. Used for stabilization gating
    /// on failback decisions. None = not currently healthy or never checked.
    pub healthy_since: Option<tokio::time::Instant>,
    /// True once a persistent WDS-wedge has been classified (registered + data-down
    /// after restarts exhausted). Cleared when the data path returns healthy.
    pub wedged: bool,
    /// When the wedge was first classified — drives the reboot grace window.
    pub wedged_since: Option<tokio::time::Instant>,
}

impl WanModemRuntimeInfo {
    /// Minimal constructor for unit tests — all optional/bool fields start at
    /// their safe defaults. Not compiled into production builds.
    #[cfg(test)]
    pub fn new_for_test() -> Self {
        Self {
            status: crate::hardware::WanModemStatus::Offline,
            consecutive_failures: 0,
            last_check: None,
            network_device: None,
            has_sim: None,
            restart_count: 0,
            restart_suspended: false,
            healthy_since: None,
            wedged: false,
            wedged_since: None,
        }
    }
}

impl AppState {
    /// Create new AppState with empty modem map.
    ///
    /// Used for initialization - modems are added via add_modem() after creation.
    pub fn new(
        config: AppConfig,
        users: UserStore,
        registry: ProfileRegistry,
        device_token: String,
        device_auth: Arc<DeviceAuth>,
        license_state: LicenseState,
    ) -> Self {
        let (event_tx, _) = broadcast::channel(512);
        // Store a clone in the global so AT handler can emit debug traces
        let _ = DEBUG_TX.set(event_tx.clone());
        let session_expiry = config.auth.session_expiry_hours;

        let rate_limiter = Arc::new(RateLimiter::new(&config.rate_limit));
        let login_lockout = Arc::new(LoginLockout::new());
        let audit = Arc::new(AuditLog::new());

        Self {
            modems: Arc::new(RwLock::new(HashMap::new())),
            detected_modems: Arc::new(RwLock::new(Vec::new())),
            profile_registry: Arc::new(registry),
            event_tx,
            sessions: Arc::new(SessionStore::new(session_expiry)),
            ws_tokens: Arc::new(WsTokenStore::new()),
            users: Arc::new(users),
            rate_limiter,
            login_lockout,
            audit,
            config: Arc::new(RwLock::new(config)),
            ws_client_count: AtomicUsize::new(0),
            at_whitelist_overrides: Arc::new(RwLock::new(WhitelistOverrides::default())),
            apn_profiles: Arc::new(RwLock::new(Vec::new())),
            sim_slot_config: Arc::new(RwLock::new(AllSimSlotConfig::default())),
            wan_config: Arc::new(RwLock::new(WanConfig::default())),
            wan_runtime: Arc::new(RwLock::new(WanRuntimeState::default())),
            platform_capabilities: Arc::new(RwLock::new(PlatformCapabilities::default())),
            routing_state: Arc::new(RwLock::new(HashMap::new())),
            steering_rules: Arc::new(RwLock::new(Vec::new())),
            selected_modem_id: Arc::new(RwLock::new(None)),
            tls_active: std::sync::atomic::AtomicBool::new(false),
            gps_panel_active: std::sync::atomic::AtomicBool::new(false),
            telemetry_portal_enabled: std::sync::atomic::AtomicBool::new(false),
            license_state: Arc::new(RwLock::new(license_state)),
            device_token,
            device_auth,
            device_nonce: Arc::new(RwLock::new(None)),
            telemetry_buffer: crate::api::telemetry::new_buffer(),
            heartbeat_interval_secs: AtomicU64::new(30 * 60),
            fast_mode_until: Arc::new(RwLock::new(None)),
            poll_now_pending: AtomicBool::new(false),
            tunnel_shutdown: Arc::new(tokio::sync::Notify::new()),
            speedtest_history: Arc::new(RwLock::new(crate::hardware::speedtest::SpeedtestHistory::new())),
            speedtest_lock: Arc::new(tokio::sync::Mutex::new(())),
            speedtest_buffer: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Add a modem to the state.
    ///
    /// Creates a ModemContext and inserts it into the HashMap.
    pub async fn add_modem(
        &self,
        modem_id: String,
        handler: Box<dyn ModemHardware + Send>,
        profile: ModemProfile,
        detected: DetectedModem,
        config: ConnectionConfig,
        discovery: DiscoveryInfo,
    ) {
        // Extract the live device-path handle BEFORE moving the handler into the
        // Arc<Mutex<..>> (after the move it is no longer directly callable).
        let live_device_path = handler.live_device_path_handle();
        let context = ModemContext {
            id: modem_id.clone(),
            handler: Arc::new(Mutex::new(handler)),
            profile: Arc::new(profile),
            detected,
            health: Arc::new(RwLock::new(ModemHealth::default())),
            last_signal: Arc::new(RwLock::new(None)),
            config: Arc::new(RwLock::new(config)),
            discovery: Arc::new(RwLock::new(discovery)),
            state_cache: Arc::new(RwLock::new(None)),
            signal_history: Arc::new(RwLock::new(VecDeque::with_capacity(1440))),
            usbnet_mode: Arc::new(RwLock::new(UsbNetMode::Unknown)),
            live_device_path,
        };

        let mut modems = self.modems.write().await;
        modems.insert(modem_id, context);
    }

    /// Replace a modem's handler by swapping the ENTIRE `Arc<Mutex<..>>` (not the
    /// `Box` inside it). Used by the reconnect watcher to recover a handler whose
    /// mutex is wedged — a stuck blocking serial syscall (e.g. `tcdrain` on a
    /// removed device) holds the guard forever, so the in-place swap (`*guard =
    /// new`) cannot acquire it. Replacing the whole Arc lets new callers resolve
    /// the fresh handler; the old Arc + its stuck thread/fd leak until the next
    /// restart (bounded, rare). Returns `false` if `modem_id` is absent.
    pub async fn replace_handler(
        &self,
        modem_id: &str,
        new_handler: Box<dyn ModemHardware + Send>,
    ) -> bool {
        // Re-extract the live device-path handle from the FRESH handler before
        // boxing it, so the context does not keep a stale handle into the old
        // (possibly wedged) handler's cell.
        let live_device_path = new_handler.live_device_path_handle();
        let mut modems = self.modems.write().await;
        match modems.get_mut(modem_id) {
            Some(ctx) => {
                ctx.handler = Arc::new(Mutex::new(new_handler));
                ctx.live_device_path = live_device_path;
                true
            }
            None => false,
        }
    }

    /// Remove a modem from the state.
    ///
    /// Removes the ModemContext from the HashMap and updates associated state.
    /// If the removed modem was the selected modem, clears the selection so
    /// get_selected_or_first_modem() falls back to the first remaining modem.
    pub async fn remove_modem(&self, modem_id: &str) {
        // Remove from HashMap
        let mut modems = self.modems.write().await;
        if modems.remove(modem_id).is_none() {
            return; // Already gone
        }
        drop(modems);

        // Clear selection if needed
        let mut selected = self.selected_modem_id.write().await;
        if selected.as_ref() == Some(&modem_id.to_string()) {
            *selected = None;
        }
        drop(selected);

        // Update detected_modems list (fuzzy match by VID:PID prefix)
        let mut detected = self.detected_modems.write().await;
        detected.retain(|d| {
            if let (Some(vid), Some(pid)) = (&d.vendor_id, &d.product_id) {
                !modem_id.starts_with(&format!("{vid}:{pid}:"))
            } else {
                true
            }
        });
        drop(detected);

        tracing::info!("[{}] Removed from state", modem_id);
    }

    /// Get a receiver for modem events.
    pub fn subscribe_events(&self) -> broadcast::Receiver<BroadcastEvent> {
        self.event_tx.subscribe()
    }

    /// Broadcast a global event (no modem_id) to all WebSocket clients.
    pub fn broadcast_event(&self, event: ModemEvent) {
        // Ignore send errors (no subscribers)
        let _ = self.event_tx.send(BroadcastEvent {
            modem_id: None,
            event,
        });
    }

    /// Broadcast a modem-scoped event to all WebSocket clients.
    /// The modem_id will be injected as a top-level field in the JSON payload.
    pub fn broadcast_modem_event(&self, modem_id: &str, event: ModemEvent) {
        let _ = self.event_tx.send(BroadcastEvent {
            modem_id: Some(modem_id.to_string()),
            event,
        });
    }

    /// Run boot-time USB-net mode detection for `modem_id`, write the result into
    /// `ModemContext.usbnet_mode`, and broadcast `ModemEvent::UsbNetModeDetected`.
    ///
    /// Diagnostic only — the detected mode is intended for engineer-facing surfaces
    /// (debug-trace WS panel, GET /api/wan/status JSON). Per the mode-agnostic
    /// principle (`feedback_modem_mode_agnostic.md`), it must never be rendered on
    /// operator-facing UI.
    ///
    /// Detection failure is silent — `detect_usbnet_mode_with_bus_port` degrades all errors to
    /// `UsbNetMode::Unknown` per spec §3.10 and never returns `Err`. Bring-up is
    /// not blocked by this routine.
    ///
    /// Lock-ordering: clones the per-modem `Arc`s (handler, profile, usbnet_mode)
    /// while holding only the modems-map read lock, then drops the map lock before
    /// taking the handler mutex. This avoids re-acquiring the modems read lock and
    /// keeps the dance minimal.
    ///
    /// Callers (Item #37 sub-task 1, spec §3.3): every site that creates a
    /// `ModemContext` via `add_modem` — daemon-startup real-hardware path, mock
    /// fallback, USB hot-plug handler, profile-rescan handler. The `main.rs:516`
    /// early-mock fast-path is intentionally excluded (dev-only `MOCK_HARDWARE=1`
    /// route).
    pub async fn detect_and_cache_usbnet_mode(&self, modem_id: &str) {
        let snapshot = {
            let modems = self.modems.read().await;
            modems.get(modem_id).map(|ctx| {
                (
                    ctx.handler.clone(),
                    ctx.profile.clone(),
                    ctx.usbnet_mode.clone(),
                    ctx.detected.bus_port.clone(),
                )
            })
        };
        let Some((handler_arc, profile_arc, mode_lock, bus_port)) = snapshot else {
            return;
        };

        let mode = {
            let handler = handler_arc.lock().await;
            crate::hardware::detect_usbnet_mode_with_bus_port(
                handler.as_ref(),
                &profile_arc,
                modem_id,
                bus_port.as_deref(),
            )
            .await
        };
        *mode_lock.write().await = mode;
        self.broadcast_modem_event(modem_id, ModemEvent::UsbNetModeDetected { mode });
    }

    /// Register a new WebSocket client connection.
    pub fn ws_client_connect(&self) {
        let count = self.ws_client_count.fetch_add(1, Ordering::Relaxed) + 1;
        tracing::debug!("WebSocket client connected (total: {})", count);
    }

    /// Unregister a WebSocket client disconnection.
    pub fn ws_client_disconnect(&self) {
        let count = self.ws_client_count.fetch_sub(1, Ordering::Relaxed) - 1;
        tracing::debug!("WebSocket client disconnected (total: {})", count);
    }

    /// Check if there are any active WebSocket clients.
    #[allow(dead_code)]
    pub fn has_ws_clients(&self) -> bool {
        self.ws_client_count.load(Ordering::Relaxed) > 0
    }

    /// Get the currently selected modem ID, or the first modem if none selected.
    ///
    /// Phase 1 backward-compat: single-modem API routes use this to determine
    /// which modem to operate on.
    pub async fn get_selected_or_first_modem(&self) -> Option<String> {
        let selected = self.selected_modem_id.read().await;
        if let Some(id) = selected.as_ref() {
            return Some(id.clone());
        }
        drop(selected);

        // No selection — default to lexicographically first modem for deterministic ordering
        let modems = self.modems.read().await;
        modems.keys().min().cloned()
    }

    /// Reconcile the live AT-port path for `modem_id` into the canonical records.
    ///
    /// Reads the modem's `live_device_path` cell (set by `reopen_port` on every
    /// successful self-heal), decides using the pure `reconcile_device_path`
    /// helper, and if changed applies the new path to both:
    ///   - `ctx.detected.device_path` (under `modems.write()`)
    ///   - the matching `detected_modems` entry matched by `bus_port` (under
    ///     `detected_modems.write()`)
    ///
    /// Returns `Some(new_path)` when a change was applied, `None` otherwise
    /// (including when the handler has no live-path cell, e.g. the mock).
    ///
    /// Lock discipline: takes `modems.read()` to gather+decide, drops it, then
    /// takes `modems.write()` and `detected_modems.write()` separately to apply.
    /// No lock is held across an `.await`; all awaits are on in-memory lock
    /// acquisitions (no I/O).
    pub async fn reconcile_modem_device_path(&self, modem_id: &str) -> Option<String> {
        // 1. Gather: read the cell + decide (no write locks held).
        let decision = {
            let modems = self.modems.read().await;
            modems.get(modem_id).and_then(|ctx| {
                let cell_path = ctx
                    .live_device_path
                    .as_ref()
                    .and_then(|c| c.lock().ok().map(|g| g.clone()));
                cell_path.and_then(|cp| {
                    reconcile_device_path(&ctx.detected.device_path, &cp)
                        .map(|new| (new, ctx.detected.bus_port.clone()))
                })
            })
        };

        // 2. Apply under the appropriate write locks.
        if let Some((new_path, bus_port)) = decision {
            {
                let mut modems = self.modems.write().await;
                if let Some(ctx) = modems.get_mut(modem_id) {
                    ctx.detected.device_path = new_path.clone();
                }
            }
            {
                let mut detected = self.detected_modems.write().await;
                if let Some(entry) = detected.iter_mut().find(|d| d.bus_port == bus_port) {
                    entry.device_path = new_path.clone();
                }
            }
            return Some(new_path);
        }

        None
    }
}

/// Pure decision for the cache-task device_path reconcile: given the currently
/// reported path and the live cell's path, return `Some(new)` iff they differ,
/// else `None`. Keeping the decision pure makes it unit-testable in isolation;
/// the caller (60s cache task via `reconcile_modem_device_path`) applies the
/// `Some` under the appropriate locks.
pub fn reconcile_device_path(current: &str, cell: &str) -> Option<String> {
    if current == cell {
        None
    } else {
        Some(cell.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconcile_device_path_some_only_when_changed() {
        // Unchanged → None (no write, idempotent).
        assert_eq!(reconcile_device_path("/dev/ttyUSB2", "/dev/ttyUSB2"), None);
        // Changed → Some(new).
        assert_eq!(
            reconcile_device_path("/dev/ttyUSB2", "/dev/ttyUSB3"),
            Some("/dev/ttyUSB3".to_string())
        );
        // Empty-string edges: empty current, non-empty cell → adopt the cell.
        assert_eq!(
            reconcile_device_path("", "/dev/ttyUSB0"),
            Some("/dev/ttyUSB0".to_string())
        );
        // Both empty → unchanged → None.
        assert_eq!(reconcile_device_path("", ""), None);
        // Non-empty current, empty cell (never expected, but defensive) → still
        // "changed" by string inequality → Some("") so the contract is "differ ⇒
        // Some". The cache caller gates on the cell being a real reopen value.
        assert_eq!(reconcile_device_path("/dev/ttyUSB2", ""), Some(String::new()));
    }
}

#[cfg(test)]
mod wedge_runtime_tests {
    use super::*;

    #[test]
    fn runtime_info_defaults_not_wedged() {
        let info = WanModemRuntimeInfo::new_for_test();
        assert!(!info.wedged);
        assert!(info.wedged_since.is_none());
    }
}
