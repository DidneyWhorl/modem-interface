//! Shared data types for the modem interface.
//!
//! These types match the API contract exactly and are used by both the HTTP/WebSocket
//! handlers and the hardware abstraction layer.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Network technology generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Technology {
    #[serde(rename = "2G")]
    Gen2,
    #[serde(rename = "3G")]
    Gen3,
    #[serde(rename = "4G")]
    Gen4,
    #[serde(rename = "5G")]
    Gen5,
}

/// Detected USB-net mode of a cellular modem. Diagnostic only — never surfaced
/// in operator-facing UI per the mode-agnostic principle (see
/// `feedback_modem_mode_agnostic.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UsbNetMode {
    Ecm,
    Qmi,
    Mbim,
    Rmnet,
    Ncm,
    Rndis,
    #[default]
    Unknown,
}

/// Device identification information.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub imei: String,
    pub manufacturer: String,
    pub model: String,
    pub firmware_version: String,
    pub supported_protocols: Vec<String>,
}

/// Current modem connection status.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModemStatus {
    pub connected: bool,
    pub technology: Option<Technology>,
    pub operator: Option<String>,
    /// Signal strength: 0-100 normalized percentage
    pub signal_strength: i32,
    pub ip_address: Option<String>,
}

/// Sentinel value stored in [`SignalInfo`] fields (`rssi`, `rsrp`, `rsrq`) when a
/// metric is genuinely unavailable from the modem. AT+CSQ reporting `99,99`
/// (unknown) and unparseable vendor signal fields both resolve to this value.
/// A reading is "unavailable" when it sits at or below this sentinel (real dBm
/// readings are far higher, e.g. RSRP bottoms out near -140 dBm).
pub const UNAVAILABLE_DBM: f64 = -999.0;

/// Detailed signal quality metrics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalInfo {
    /// Received Signal Strength Indicator (dBm), typically -113 to -51
    pub rssi: f64,
    /// Reference Signal Received Power (dBm), typically -140 to -44
    pub rsrp: f64,
    /// Reference Signal Received Quality (dB), typically -20 to -3
    pub rsrq: f64,
    /// Signal to Interference+Noise Ratio (dB), typically -20 to +30
    pub sinr: f64,
    /// Current band (e.g., "B3", "B7", "n78")
    pub band: String,
    /// Serving cell identifier
    pub cell_id: String,
    /// Detected technology from signal query (e.g., NR5G-NSA reports Gen5).
    /// When present, overrides the technology from AT+COPS? which may
    /// report the LTE anchor in NSA mode instead of the active 5G layer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub technology: Option<Technology>,
}

impl Default for SignalInfo {
    fn default() -> Self {
        Self {
            rssi: -113.0,
            rsrp: -140.0,
            rsrq: -20.0,
            sinr: 0.0,
            band: String::new(),
            cell_id: String::new(),
            technology: None,
        }
    }
}

impl SignalInfo {
    /// A dBm reading is "available" only if it is above the unavailable sentinel.
    /// (`<=` so a value exactly at the sentinel is treated as unavailable.)
    fn is_available(value: f64) -> bool {
        value > UNAVAILABLE_DBM
    }

    /// Coarse 0-100 "bars" signal indicator for status endpoints and the
    /// WebSocket master-cache push.
    ///
    /// Preference order, picking the first available metric:
    ///
    /// 1. **RSSI** (preferred — preserves historical behavior for every modem
    ///    that reports it, e.g. Telit). Mapped over the AT+CSQ-derived range
    ///    -113 dBm (worst) .. -51 dBm (best): `(rssi + 113) * 100 / 62`.
    /// 2. **RSRP** fallback. The Quectel RM520N-GL in 5G mode answers AT+CSQ
    ///    with `99,99` (unknown), so `rssi` is the unavailable sentinel; without
    ///    this fallback a healthy 5G link clamped to 0%. RSRP spans roughly
    ///    -140 dBm (no signal) .. -44 dBm (excellent), a 96 dB window:
    ///    `(rsrp + 140) * 100 / 96`. A connected modem at RSRP ≈ -90 yields ~52%.
    /// 3. If neither metric is available, returns 0 (the prior effective value).
    pub fn signal_strength_percent(&self) -> i32 {
        if Self::is_available(self.rssi) {
            ((self.rssi + 113.0) * 100.0 / 62.0).clamp(0.0, 100.0) as i32
        } else if Self::is_available(self.rsrp) {
            ((self.rsrp + 140.0) * 100.0 / 96.0).clamp(0.0, 100.0) as i32
        } else {
            0
        }
    }
}

/// GPS position information from GNSS module.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpsInfo {
    /// Latitude in decimal degrees (positive = North)
    pub latitude: f64,
    /// Longitude in decimal degrees (positive = East)
    pub longitude: f64,
    /// Altitude in meters above sea level
    #[serde(skip_serializing_if = "Option::is_none")]
    pub altitude: Option<f64>,
    /// Speed over ground in km/h
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed: Option<f64>,
    /// Fix type (e.g. "2D", "3D", "none")
    pub fix_type: String,
    /// Number of satellites in use
    pub satellites: u32,
    /// ISO 8601 timestamp
    pub timestamp: String,
}

impl Default for GpsInfo {
    fn default() -> Self {
        Self {
            latitude: 0.0,
            longitude: 0.0,
            altitude: None,
            speed: None,
            fix_type: String::new(),
            satellites: 0,
            timestamp: String::new(),
        }
    }
}

/// One-time discovery data read at modem initialization.
/// Never re-polled. Served from memory thereafter.
#[allow(dead_code)] // consumed by Backend session (cache refresh task)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DiscoveryInfo {
    /// Device identification (IMEI, manufacturer, model, firmware, protocols)
    pub device_info: DeviceInfo,
    /// SIM card status (state, ICCID, IMSI, operator)
    pub sim_status: SimStatus,
}

/// Connection state without signal metrics.
/// Used by the cache refresh task to avoid duplicate AT+CSQ calls.
/// The cache task calls get_signal() for metrics and get_connection_status()
/// for connection state, then derives signal_strength from SignalInfo.rssi.
#[allow(dead_code)] // consumed by Backend session (cache refresh task)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConnectionStatus {
    pub connected: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub technology: Option<Technology>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operator: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ip_address: Option<String>,
}

/// Master cache snapshot built every 60 seconds by the cache refresh task.
/// All normal panel data is served from this cache. On-demand queries
/// bypass it and hit the hardware directly.
#[allow(dead_code)] // consumed by Backend session (cache refresh task)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModemStateCache {
    /// Detailed signal metrics from the last cache refresh
    pub signal: SignalInfo,
    /// Connection state (no signal_strength — use the field below)
    pub connection: ConnectionStatus,
    /// Signal strength 0-100, derived from signal.rssi (no extra AT+CSQ)
    pub signal_strength: i32,
    /// Network registration state
    pub registration: RegistrationState,
    /// GPS position (only populated when gps_panel_active flag is true)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gps: Option<GpsInfo>,
    /// ISO 8601 timestamp of when this cache was last refreshed
    pub timestamp: String,
}

/// A single point-in-time signal quality sample for the history ring buffer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalSample {
    /// Unix epoch seconds
    pub ts: i64,
    /// Reference Signal Received Power (dBm)
    pub rsrp: f32,
    /// Reference Signal Received Quality (dB)
    pub rsrq: f32,
    /// Signal to Interference+Noise Ratio (dB)
    pub sinr: f32,
}

/// A single telemetry snapshot for portal reporting.
/// Captured every 5 minutes from the master cache, batched and sent every 30 minutes.
#[allow(dead_code)] // Used in Task 3 (telemetry collector)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetrySnapshot {
    /// Stable WAN identifier. IMEI for modems, "eth:{device}" for Ethernet WANs.
    #[serde(default)]
    pub wan_id: String,
    /// WAN source type: "modem" or "ethernet"
    #[serde(default)]
    pub wan_type: String,
    /// Active bands: primary cell + CA secondary cells. Empty for Ethernet WANs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bands: Vec<String>,
    /// Network access technology detail (e.g., "FDD LTE", "NR5G-NSA", "NR5G-SA").
    /// None for Ethernet WANs or when extended signal query is unavailable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network_type: Option<String>,
    /// Modem model name (e.g., "RM551E-GL", "FN990"). None for Ethernet WANs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modem_name: Option<String>,
    /// When this snapshot was recorded (UTC ISO 8601)
    pub recorded_at: String,
    /// RSRP in dBm
    pub rsrp: f64,
    /// RSRQ in dB
    pub rsrq: f64,
    /// SINR in dB
    pub sinr: f64,
    /// RSSI in dBm
    pub rssi: f64,
    /// Network technology (4G / 5G-NSA / 5G-SA)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub technology: Option<String>,
    /// Carrier name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operator: Option<String>,
    /// Whether modem is connected
    pub connected: bool,
    /// Active WAN interface name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_wan: Option<String>,
    /// Whether a failover occurred since last snapshot
    pub failover_event: bool,
    /// Router uptime in seconds
    pub device_uptime_secs: u64,
    /// Current connection uptime in seconds
    pub connection_uptime_secs: u64,
}

/// Time-series signal quality history for a modem.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalHistory {
    pub modem_id: String,
    pub samples: Vec<SignalSample>,
}

/// Extended signal info: primary cell, secondary cells, carrier aggregation status.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExtendedSignalInfo {
    /// Primary serving cell signal
    pub primary: SignalInfo,
    /// Secondary cells (carrier aggregation components)
    pub secondary_cells: Vec<SignalInfo>,
    /// Whether carrier aggregation is active
    pub carrier_aggregation: bool,
    /// Network access technology (e.g. "FDD LTE", "NR5G-SA")
    pub network_type: String,
}

/// Per-antenna port signal measurement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AntennaPort {
    /// Antenna port index (0, 1, 2, 3) — resets per technology
    pub port: u32,
    /// RSRP in dBm
    pub rsrp: f64,
    /// RSRQ in dB
    pub rsrq: f64,
    /// SINR in dB
    pub sinr: f64,
    /// Technology tag (e.g., "LTE", "NR5G-NSA", "NR5G-SA") or None for legacy single-row
    #[serde(skip_serializing_if = "Option::is_none")]
    pub technology: Option<String>,
}

/// Aggregated per-antenna metrics.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AntennaMetrics {
    /// Per-antenna-port measurements
    pub ports: Vec<AntennaPort>,
}

/// Data transfer statistics for the current session.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DataStats {
    pub bytes_tx: u64,
    pub bytes_rx: u64,
    pub session_uptime_secs: u64,
}

/// Network registration state.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RegistrationState {
    #[default]
    NotRegistered,
    #[serde(rename = "registered")]
    Registered { home: bool },
    Searching,
    Denied,
    Unknown,
}

/// Authentication type for APN connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AuthType {
    #[default]
    None,
    Pap,
    Chap,
}

/// IP protocol version for connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum IpType {
    #[default]
    Ipv4,
    Ipv6,
    Ipv4v6,
}

/// Configuration for establishing a data connection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionConfig {
    /// PDP context ID (1-8). Most carriers use 1, Verizon uses 3.
    #[serde(default = "default_cid")]
    pub cid: u8,
    #[serde(default)]
    pub apn: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    #[serde(default)]
    pub auth_type: AuthType,
    #[serde(default)]
    pub ip_type: IpType,
}

fn default_cid() -> u8 {
    1
}

impl Default for ConnectionConfig {
    fn default() -> Self {
        Self {
            cid: 1,
            apn: String::new(),
            username: None,
            password: None,
            auth_type: AuthType::default(),
            ip_type: IpType::default(),
        }
    }
}

// =============================================================================
// APN Profiles — Saved Connection Presets
// =============================================================================

/// A saved APN connection preset.
///
/// Bundles APN settings + optional MBN carrier profile into a one-click apply
/// operation. Profiles are tagged with a modem profile ID since different modem
/// models use different AT command sequences.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApnProfile {
    /// Unique profile identifier (UUID v4).
    pub id: String,
    /// User-chosen display name, e.g. "TMO Home Internet".
    pub name: String,
    /// Modem profile ID this preset is for, e.g. "quectel_rm551e_gl".
    pub modem_profile_id: String,
    /// APN connection settings (APN, CID, IP type, auth).
    pub connection: ConnectionConfig,
    /// Optional MBN carrier profile name to select before connecting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mbn_profile: Option<String>,
    /// ISO 8601 creation timestamp.
    pub created_at: String,
    /// ISO 8601 last-updated timestamp.
    pub updated_at: String,
}

/// Request body for creating/updating an APN profile.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApnProfileRequest {
    pub name: String,
    pub modem_profile_id: String,
    pub connection: ConnectionConfig,
    #[serde(default)]
    pub mbn_profile: Option<String>,
}

/// Request body for applying an APN profile.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApnProfileApplyRequest {
    pub profile_id: String,
}

/// Result of applying an APN profile (template-driven AT command sequence).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApnProfileApplyResult {
    pub success: bool,
    pub had_errors: bool,
    pub step_log: Vec<String>,
    pub reboot_triggered: bool,
}

/// Result of importing APN profiles from a JSON array.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApnProfileImportResult {
    pub imported: usize,
    pub skipped: usize,
    pub message: String,
}

// =============================================================================
// Dual SIM Slot Types
// =============================================================================

/// Status of a single SIM slot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimSlotStatus {
    /// Slot number (1 or 2).
    pub slot: u8,
    /// Whether this slot is currently active.
    pub active: bool,
    /// SIM status (only populated for the active slot).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sim_status: Option<SimStatus>,
    /// Assigned APN profile ID for this slot (from config).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assigned_profile_id: Option<String>,
    /// Assigned APN profile name (resolved for display).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assigned_profile_name: Option<String>,
}

/// Full dual SIM slot information returned by GET /api/sim/slots.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DualSimInfo {
    /// Whether the modem supports dual SIM.
    pub supported: bool,
    /// Whether the user has disabled dual SIM for this modem.
    /// True when the modem profile supports dual SIM but the user toggled it off.
    #[serde(default)]
    pub dual_sim_disabled: bool,
    /// Number of SIM slots (typically 2).
    pub slot_count: u8,
    /// Currently active slot (1 or 2).
    pub active_slot: u8,
    /// Per-slot status.
    pub slots: Vec<SimSlotStatus>,
}

/// Request to switch SIM slot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimSlotSwitchRequest {
    /// Target slot number (1 or 2).
    pub target_slot: u8,
    /// If true, apply the assigned APN profile after switching and reboot.
    /// If false, just switch the slot (live, no reboot).
    #[serde(default)]
    pub apply_profile: bool,
}

/// Result of a SIM slot switch operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimSlotSwitchResult {
    pub success: bool,
    pub rebooting: bool,
    pub message: String,
    pub steps: Vec<String>,
}

/// Persistent per-slot APN profile assignment for a single modem.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SimSlotConfig {
    /// APN profile ID assigned to slot 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot1_profile_id: Option<String>,
    /// APN profile ID assigned to slot 2.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot2_profile_id: Option<String>,
    /// User toggle to disable dual SIM UI for this modem.
    #[serde(default)]
    pub dual_sim_disabled: bool,
}

/// All modems' SIM slot configs, keyed by "VID:PID" (e.g. "2c7c:0122").
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AllSimSlotConfig {
    #[serde(default)]
    pub modems: std::collections::HashMap<String, SimSlotConfig>,
}

/// SIM card state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SimState {
    Ready,
    PinRequired,
    PukRequired,
    NotInserted,
    Error,
}

/// SIM card status information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimStatus {
    pub present: bool,
    pub state: SimState,
    /// Integrated Circuit Card Identifier
    pub iccid: Option<String>,
    /// International Mobile Subscriber Identity (only when unlocked)
    pub imsi: Option<String>,
    /// Operator name from SIM
    pub operator_name: Option<String>,
}

impl Default for SimStatus {
    fn default() -> Self {
        Self {
            present: false,
            state: SimState::NotInserted,
            iccid: None,
            imsi: None,
            operator_name: None,
        }
    }
}

/// PIN operation request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PinOperation {
    pub operation: PinOpType,
    pub pin: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_pin: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PinOpType {
    Verify,
    Change,
    Enable,
    Disable,
}

/// Available network from scan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AvailableNetwork {
    pub operator_name: String,
    /// MCC+MNC code
    pub operator_code: String,
    pub technology: Technology,
    pub status: NetworkStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NetworkStatus {
    Available,
    Current,
    Forbidden,
}

/// Network selection request.
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkSelection {
    /// Set to None for automatic selection
    pub operator_code: Option<String>,
    pub technology: Option<Technology>,
}

/// AT command execution request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AtCommandRequest {
    pub command: String,
    /// Set to true to confirm execution of commands that require confirmation
    #[serde(default)]
    pub confirmed: bool,
}

/// Request body for airplane mode toggle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AirplaneModeRequest {
    /// true = radio off (AT+CFUN=0), false = radio on (AT+CFUN=1)
    pub enabled: bool,
}

/// AT command execution response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AtCommandResponse {
    pub command: String,
    pub response: String,
    pub success: bool,
}

// ============================================================================
// WebSocket Event Types
// ============================================================================

/// Events broadcast over WebSocket.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload")]
#[serde(rename_all = "snake_case")]
pub enum ModemEvent {
    SignalUpdate(SignalInfo),
    ConnectionState {
        state: ConnectionState,
        network: Option<String>,
        ip: Option<String>,
    },
    RegistrationChange {
        status: RegistrationState,
        operator: Option<String>,
        technology: Option<Technology>,
    },
    SimEvent {
        event: SimEventType,
        state: SimState,
    },
    Error {
        code: String,
        message: String,
    },
    ModemHealth(ModemHealth),
    DebugTrace {
        message: String,
        source: String,
    },
    WanStatusUpdate(Box<WanStatusResponse>),
    SpeedtestProgress(SpeedtestProgress),
    SpeedtestComplete(Box<SpeedtestResult>),
    SpeedtestError {
        test_id: String,
        error: String,
    },
    WanCollisionDisplaced {
        our_name: String,
        displaced_name: String,
        device: String,
    },
    /// USB-net mode detected at boot. Diagnostic only — engineer-facing debug-trace
    /// surface only; never rendered on operator UI per `feedback_modem_mode_agnostic.md`.
    UsbNetModeDetected {
        mode: UsbNetMode,
    },
    /// A modem WAN is in a persistent WDS-wedge: radio registered but the data
    /// bearer is unrecoverable after the watchdog exhausted its restarts. Operator
    /// must reboot/power-cycle (or the opt-in guarded auto-reboot will, if enabled).
    ModemWanWedged {
        modem_id: String,
        label: String,
        restart_count: u32,
        message: String,
    },
}

/// Modem hardware availability state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModemHealth {
    /// Whether the modem is reachable via AT commands.
    pub available: bool,
    /// Current state: "online", "offline", "recovering", "rebooting".
    pub state: ModemHealthState,
    /// Human-readable message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl Default for ModemHealth {
    fn default() -> Self {
        Self {
            available: true,
            state: ModemHealthState::Ok,
            message: None,
        }
    }
}

/// Modem health states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModemHealthState {
    /// Modem responding to AT commands.
    Ok,
    /// Modem not responding (USB disappeared or I/O errors).
    Unavailable,
    /// Modem was commanded to reboot, waiting for it to come back.
    Rebooting,
    /// Error state (re-detection in progress or other error).
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Disconnecting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SimEventType {
    Inserted,
    Removed,
    Locked,
    Unlocked,
}

// ============================================================================
// Speedtest Types
// ============================================================================

/// Speedtest execution mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpeedtestMode {
    Quick,
    Medium,
    Full,
}

/// Active phase of a speedtest run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpeedtestPhase {
    Latency,
    Download,
    Upload,
}

/// Progress update emitted during a speedtest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpeedtestProgress {
    pub test_id: String,
    pub phase: SpeedtestPhase,
    pub progress_pct: u8,
    pub current_speed_mbps: f64,
    pub bytes_transferred: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub running_p90_mbps: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_label: Option<String>,
}

/// Final result of a completed speedtest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpeedtestResult {
    pub id: String,
    pub timestamp: String,
    pub mode: SpeedtestMode,
    pub wan_id: String,
    pub wan_name: String,
    pub interface: String,
    pub download_mbps: f64,
    pub upload_mbps: f64,
    pub latency_ms: f64,
    pub jitter_ms: f64,
    pub bytes_consumed: u64,
    pub server: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub download_loaded_latency_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub download_loaded_jitter_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upload_loaded_latency_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upload_loaded_jitter_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bufferbloat_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection: Option<ConnectionMetadata>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scores: Option<AimScores>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tcp_loss_ratio: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub download_measurements: Option<Vec<MeasurementBreakdown>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upload_measurements: Option<Vec<MeasurementBreakdown>>,
}

/// A single bandwidth measurement from one HTTP request.
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BandwidthPoint {
    pub size_label: String,
    pub bytes: u64,
    pub duration_ms: f64,
    pub bps: f64,
}

/// Connection metadata from Cloudflare cf-meta-* headers.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConnectionMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ip: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub colo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub city: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asn: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asn_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latitude: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub longitude: Option<f64>,
}

/// AIM (Aggregated Internet Measurement) quality scores.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AimScores {
    pub streaming: String,
    pub gaming: String,
    pub video_calls: String,
}

/// Per-payload-size measurement breakdown.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeasurementBreakdown {
    pub size_label: String,
    pub count: usize,
    pub points_bps: Vec<f64>,
}

/// TCP quality stats from Server-Timing header.
#[allow(dead_code)]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TcpStats {
    pub rtt_us: u64,
    pub min_rtt_us: u64,
    pub lost: u64,
    pub retrans: u64,
    pub cwnd: u64,
    pub delivery_rate_bps: u64,
}

// ============================================================================
// Configuration Types
// ============================================================================

/// Remote access tunnel configuration.
///
/// The portal endpoint URL lives on `PortalConfig` — readers call
/// `PortalConfig::resolved_tunnel_url()`. Legacy `[tunnel].url` keys in
/// existing `config.toml` files deserialize cleanly (no
/// `#[serde(deny_unknown_fields)]`), so upgrades need no config rewrite.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TunnelConfig {
    /// Whether the tunnel client is enabled (requires remote_access license feature).
    #[serde(default = "default_tunnel_enabled")]
    pub enabled: bool,
    /// Local ports the tunnel is allowed to proxy to.
    #[serde(default = "default_tunnel_ports")]
    pub ports: Vec<u16>,
}

fn default_tunnel_enabled() -> bool { true }
fn default_tunnel_ports() -> Vec<u16> { vec![443, 8443] }

impl Default for TunnelConfig {
    fn default() -> Self {
        Self {
            enabled: default_tunnel_enabled(),
            ports: default_tunnel_ports(),
        }
    }
}

/// Persistent application configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// Default APN settings
    #[serde(default)]
    pub connection: ConnectionConfig,
    /// Preferred bands (empty = automatic)
    #[serde(default)]
    pub preferred_bands: Vec<String>,
    /// Enable automatic connection on startup
    #[serde(default)]
    pub auto_connect: bool,
    /// Signal polling interval in seconds
    #[serde(default = "default_poll_interval")]
    pub signal_poll_interval: u64,
    /// Authentication configuration
    #[serde(default)]
    pub auth: crate::config::auth::AuthConfig,
    /// TLS/HTTPS configuration
    #[serde(default)]
    pub tls: TlsConfig,
    /// Rate limiting configuration
    #[serde(default)]
    pub rate_limit: RateLimitConfig,
    /// Whether to send telemetry data to the portal (local opt-in).
    #[serde(default)]
    pub telemetry_enabled: bool,
    /// Remote access tunnel configuration.
    #[serde(default)]
    pub tunnel: TunnelConfig,
    /// Portal URLs (heartbeat, telemetry, tunnel, licensing).
    /// Missing section in config.toml gets production defaults.
    #[serde(default)]
    pub portal: crate::config::PortalConfig,
}

fn default_poll_interval() -> u64 {
    2
}

/// TLS configuration for HTTPS support.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsConfig {
    /// Enable TLS (HTTPS).
    #[serde(default = "default_tls_enabled")]
    pub enabled: bool,
    /// Path to PEM-encoded certificate file.
    #[serde(default = "default_cert_path")]
    pub cert_path: String,
    /// Path to PEM-encoded private key file.
    #[serde(default = "default_key_path")]
    pub key_path: String,
    /// HTTPS listen port.
    #[serde(default = "default_https_port")]
    pub https_port: u16,
    /// Redirect HTTP to HTTPS when TLS is active.
    #[serde(default = "default_redirect_http")]
    pub redirect_http: bool,
}

fn default_tls_enabled() -> bool { true }
fn default_cert_path() -> String { "/etc/modem-interface/tls/cert.pem".to_string() }
fn default_key_path() -> String { "/etc/modem-interface/tls/key.pem".to_string() }
fn default_https_port() -> u16 { 8443 }
fn default_redirect_http() -> bool { true }

impl Default for TlsConfig {
    fn default() -> Self {
        Self {
            enabled: default_tls_enabled(),
            cert_path: default_cert_path(),
            key_path: default_key_path(),
            https_port: default_https_port(),
            redirect_http: default_redirect_http(),
        }
    }
}

/// Rate limiting configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitConfig {
    /// Enable rate limiting.
    #[serde(default = "default_rate_limit_enabled")]
    pub enabled: bool,
    /// Max login attempts per IP within the window.
    #[serde(default = "default_login_max_attempts")]
    pub login_max_attempts: u32,
    /// Login rate limit window in seconds.
    #[serde(default = "default_login_window_secs")]
    pub login_window_secs: u64,
    /// Max setup attempts per IP within the window.
    #[serde(default = "default_setup_max_attempts")]
    pub setup_max_attempts: u32,
    /// Setup rate limit window in seconds.
    #[serde(default = "default_setup_window_secs")]
    pub setup_window_secs: u64,
    /// Max general API requests per IP within the window.
    #[serde(default = "default_general_max_requests")]
    pub general_max_requests: u32,
    /// General rate limit window in seconds.
    #[serde(default = "default_general_window_secs")]
    pub general_window_secs: u64,
}

fn default_rate_limit_enabled() -> bool { true }
fn default_login_max_attempts() -> u32 { 5 }
fn default_login_window_secs() -> u64 { 900 }
fn default_setup_max_attempts() -> u32 { 3 }
fn default_setup_window_secs() -> u64 { 3600 }
fn default_general_max_requests() -> u32 { 100 }
fn default_general_window_secs() -> u64 { 60 }

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            enabled: default_rate_limit_enabled(),
            login_max_attempts: default_login_max_attempts(),
            login_window_secs: default_login_window_secs(),
            setup_max_attempts: default_setup_max_attempts(),
            setup_window_secs: default_setup_window_secs(),
            general_max_requests: default_general_max_requests(),
            general_window_secs: default_general_window_secs(),
        }
    }
}

// ============================================================================
// Band & Mode Configuration Types
// ============================================================================

use crate::hardware::profiles::NetworkModeOption;

/// Response from GET /api/modem/bands — current band/mode configuration
/// combined with profile metadata for UI rendering.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BandConfigResponse {
    // Profile metadata (what the modem supports)
    pub supported_modes: Vec<NetworkModeOption>,
    pub supported_lte_bands: Vec<u32>,
    pub supported_nsa_bands: Vec<u32>,
    pub supported_sa_bands: Vec<u32>,
    pub supported_nrdc_bands: Vec<u32>,
    pub has_nrdc: bool,
    pub reboot_on_band_change: bool,
    pub has_restore: bool,
    // Current modem state
    pub active_mode_id: Option<String>,
    pub active_mode_raw: Option<String>,
    pub nr5g_disable_mode: Option<u8>,
    pub active_lte_bands: Vec<u32>,
    pub active_nsa_bands: Vec<u32>,
    pub active_sa_bands: Vec<u32>,
    pub active_nrdc_bands: Vec<u32>,
    pub nrdc_enabled: Option<bool>,
}

/// Request body for POST /api/modem/bands — apply band/mode changes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BandConfigRequest {
    pub mode_id: String,
    pub lte_bands: Vec<u32>,
    pub nsa_bands: Vec<u32>,
    pub sa_bands: Vec<u32>,
    #[serde(default)]
    pub nrdc_bands: Option<Vec<u32>>,
    #[serde(default)]
    pub nrdc_enabled: Option<bool>,
}

// ============================================================================
// MBN Carrier Profile Types
// ============================================================================

/// A single MBN carrier profile entry from the modem.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MbnProfile {
    /// Profile index number
    pub index: u32,
    /// Whether this profile is currently selected
    pub selected: bool,
    /// Whether this profile is currently activated
    pub activated: bool,
    /// Profile name (e.g. "ROW_Commercial", "VoLTE-ATT")
    pub name: String,
    /// Version hex string (e.g. "0x0A010809")
    pub version: String,
    /// Revision identifier (e.g. "202408051")
    pub revision: String,
}

/// Request body for POST /api/modem/mbn/select
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MbnSelectRequest {
    /// Profile name to select (e.g. "ROW_Commercial")
    pub profile_name: String,
}

/// Request body for POST /api/modem/mbn/auto-select
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MbnAutoSelectRequest {
    /// true = enable auto-select, false = disable
    pub enabled: bool,
}

/// Response from MBN state-changing operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MbnActionResult {
    pub success: bool,
    pub reboot_recommended: bool,
    pub message: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            connection: ConnectionConfig {
                cid: 1,
                apn: String::new(),
                username: None,
                password: None,
                auth_type: AuthType::None,
                ip_type: IpType::Ipv4,
            },
            preferred_bands: Vec::new(),
            auto_connect: false,
            signal_poll_interval: default_poll_interval(),
            auth: Default::default(),
            tls: Default::default(),
            rate_limit: Default::default(),
            telemetry_enabled: false,
            tunnel: Default::default(),
            portal: crate::config::PortalConfig::default(),
        }
    }
}

// ============================================================================
// WAN Manager Types
// ============================================================================

/// Routing mode for the WAN manager.
///
/// - **Failover**: Traditional active/standby — only one WAN carries traffic at a time.
/// - **LoadBalance**: Multiple active WANs carry traffic simultaneously using ECMP multipath.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutingMode {
    #[default]
    Failover,
    LoadBalance,
}

/// Type discriminator for WAN priority entries.
///
/// - **Modem**: A cellular modem interface (WWAN).
/// - **Ethernet**: An Ethernet WAN interface (existing WAN or converted LAN port).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WanEntryType {
    #[default]
    Modem,
    Ethernet,
}

/// Persistent WAN manager configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WanConfig {
    /// Whether the WAN manager is active.
    #[serde(default)]
    pub enabled: bool,
    /// Ordered list of modems in priority order (index 0 = highest).
    #[serde(default)]
    pub modem_priority: Vec<WanModemEntry>,
    /// Watchdog configuration.
    #[serde(default)]
    pub watchdog: WatchdogConfig,
    /// Lock failover — prevent automatic failover/failback.
    #[serde(default)]
    pub failover_locked: bool,
    /// Auto-failback stabilization timer in minutes.
    /// Valid values: 0 (never/manual only), 15, 30, 60, 360, 720.
    #[serde(default = "default_failback_timer_mins")]
    pub failback_timer_mins: u32,
    /// Routing mode: failover (one active WAN) or load_balance (ECMP multipath).
    #[serde(default)]
    pub routing_mode: RoutingMode,
}

/// Modem participation state in the WAN priority list.
///
/// - **Active**: Participates in normal failover rotation with position-based metrics.
/// - **Standby**: Last-resort only (metric 998), health-checked but not in normal rotation.
/// - Removed modems are simply absent from the list (interface torn down on save).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WanModemState {
    Active,
    #[default]
    Standby,
}

/// A modem or Ethernet entry in the WAN priority list.
///
/// Supports migration from the old `enabled: bool` format:
/// - Old configs with `"enabled": true` map to `state: Active`
/// - Old configs with `"enabled": false` map to `state: Standby`
/// - New configs use `"state": "active"` or `"state": "standby"` directly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WanModemEntry {
    /// Persistent identifier. For modems: IMEI. For Ethernet: "eth:{device}" (e.g. "eth:br-wan").
    pub modem_id: String,
    /// Human-readable label (e.g. "RM551E-GL #1", "Ethernet WAN (br-wan)").
    pub label: String,
    /// OpenWRT network interface name (e.g. "WWAN", "WWAN2", "wan", "EWAN").
    pub interface_name: String,
    /// Linux network device (e.g. "usb0", "wwan0", "br-wan", "lan0").
    pub network_device: String,
    /// AT command port path (e.g. "/dev/ttyUSB2") for direct modem communication.
    /// Empty for Ethernet entries.
    #[serde(default)]
    pub device_path: String,
    /// Modem participation state: Active (normal failover) or Standby (last resort).
    /// Alias "enabled" supports migration from old configs where `enabled: true/false` was used.
    #[serde(default, alias = "enabled", deserialize_with = "deserialize_wan_modem_state")]
    pub state: WanModemState,
    /// Current routing metric (lower = higher priority).
    pub metric: u32,
    /// Entry type: modem or ethernet. Defaults to "modem" for backward compatibility.
    #[serde(default)]
    pub entry_type: WanEntryType,
    /// For LAN ports converted to WAN, tracks the original bridge (e.g. "br-lan")
    /// so the port can be reverted. None for modems and existing WAN interfaces.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_bridge: Option<String>,
    /// MTU override for this interface (576–9000). None = system default (typically 1500).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mtu: Option<u32>,
    /// IPv4 TTL mangle value for outbound traffic (1–255). None = no mangle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl: Option<u8>,
    /// IPv6 Hop Limit mangle value for outbound traffic (1–255). None = no mangle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hop_limit: Option<u8>,
    /// Load-balance weight for ECMP multipath (1–100). None = equal weight.
    /// Only meaningful when routing_mode is LoadBalance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight: Option<u32>,
    /// Optional UCI `proto` override. When set, this exact value is written
    /// to `uci set network.<iface>.proto=...`. When None, the daemon picks
    /// based on detected USB-net mode (Item #37). Free-form (UCI accepts
    /// arbitrary strings); typical values: "dhcp", "qmi", "mbim", "static",
    /// "pppoe". See feedback_modem_mode_agnostic.md — operator never sees
    /// a mode picker; this is an Advanced expert-mode escape hatch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proto_override: Option<String>,
}

/// Custom deserializer that handles both new `state` field and old `enabled` field.
///
/// Accepts:
/// - `"active"` / `"standby"` (new format)
/// - `true` → Active, `false` → Standby (old `enabled` bool migration)
/// - Missing field → default (Standby)
fn deserialize_wan_modem_state<'de, D>(deserializer: D) -> Result<WanModemState, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de;

    struct WanModemStateVisitor;

    impl<'de> de::Visitor<'de> for WanModemStateVisitor {
        type Value = WanModemState;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("\"active\", \"standby\", or a boolean")
        }

        fn visit_bool<E: de::Error>(self, v: bool) -> Result<WanModemState, E> {
            Ok(if v { WanModemState::Active } else { WanModemState::Standby })
        }

        fn visit_str<E: de::Error>(self, v: &str) -> Result<WanModemState, E> {
            match v {
                "active" => Ok(WanModemState::Active),
                "standby" => Ok(WanModemState::Standby),
                other => Err(E::unknown_variant(other, &["active", "standby"])),
            }
        }
    }

    deserializer.deserialize_any(WanModemStateVisitor)
}

impl WanModemEntry {
    /// Returns true if this modem is in Active state.
    pub fn is_active(&self) -> bool {
        self.state == WanModemState::Active
    }
}

/// Connectivity watchdog settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchdogConfig {
    /// Enable the connectivity watchdog.
    #[serde(default = "default_watchdog_enabled")]
    pub enabled: bool,
    /// Seconds between health checks.
    #[serde(default = "default_check_interval")]
    pub check_interval_secs: u32,
    /// Consecutive failures before failover.
    #[serde(default = "default_failure_threshold")]
    pub failure_threshold: u32,
    /// ICMP ping target.
    #[serde(default = "default_ping_target")]
    pub ping_target: String,
    /// DNS resolution target.
    #[serde(default = "default_dns_target")]
    pub dns_target: String,
    /// HTTP connectivity check URL.
    #[serde(default = "default_http_target")]
    pub http_target: String,
    /// Log retention in days (1-30, default 14).
    #[serde(default = "default_log_retention_days")]
    pub log_retention_days: u32,
    /// Restart any modem that crosses the failure threshold (AT+CFUN=1,1).
    /// Each modem is restarted independently with its own cooldown timer.
    #[serde(default, alias = "restart_on_total_failure")]
    pub restart_on_failure: bool,
    /// Per-modem cooldown in minutes after a restart before another can be triggered (default 5).
    #[serde(default = "default_restart_cooldown_mins")]
    pub restart_cooldown_mins: u32,
    /// Maximum restart attempts before suspending restarts for a modem (default 5, range 1-50).
    #[serde(default = "default_max_restart_attempts")]
    pub max_restart_attempts: u32,
    /// Opt-in: escalate a persistent WDS-wedge to a controlled router reboot
    /// when the wedged modem is the sole live uplink. OFF by default.
    #[serde(default)]
    pub wedge_reboot_enabled: bool,
    /// Minutes a wedge must persist (after restarts exhausted) before a reboot.
    #[serde(default = "default_wedge_reboot_grace_mins")]
    pub wedge_reboot_grace_mins: u32,
    /// Hard ceiling of auto-reboots within a trailing 24h (anti-boot-loop).
    #[serde(default = "default_wedge_reboot_max_per_day")]
    pub wedge_reboot_max_per_day: u32,
    /// Never auto-reboot if router uptime is below this many minutes (boot-loop guard).
    #[serde(default = "default_wedge_reboot_min_uptime_mins")]
    pub wedge_reboot_min_uptime_mins: u32,
}

fn default_failback_timer_mins() -> u32 { 30 }
fn default_watchdog_enabled() -> bool { true }
fn default_check_interval() -> u32 { 30 }
fn default_failure_threshold() -> u32 { 3 }
fn default_ping_target() -> String { "8.8.8.8".to_string() }
fn default_dns_target() -> String { "google.com".to_string() }
fn default_http_target() -> String { "http://connectivitycheck.gstatic.com/generate_204".to_string() }
fn default_log_retention_days() -> u32 { 14 }
fn default_restart_cooldown_mins() -> u32 { 5 }
fn default_max_restart_attempts() -> u32 { 5 }
fn default_wedge_reboot_grace_mins() -> u32 { 10 }
fn default_wedge_reboot_max_per_day() -> u32 { 2 }
fn default_wedge_reboot_min_uptime_mins() -> u32 { 15 }

impl Default for WatchdogConfig {
    fn default() -> Self {
        Self {
            enabled: default_watchdog_enabled(),
            check_interval_secs: default_check_interval(),
            failure_threshold: default_failure_threshold(),
            ping_target: default_ping_target(),
            dns_target: default_dns_target(),
            http_target: default_http_target(),
            log_retention_days: default_log_retention_days(),
            restart_on_failure: false,
            restart_cooldown_mins: default_restart_cooldown_mins(),
            max_restart_attempts: default_max_restart_attempts(),
            wedge_reboot_enabled: false,
            wedge_reboot_grace_mins: default_wedge_reboot_grace_mins(),
            wedge_reboot_max_per_day: default_wedge_reboot_max_per_day(),
            wedge_reboot_min_uptime_mins: default_wedge_reboot_min_uptime_mins(),
        }
    }
}

/// Runtime WAN modem connectivity status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WanModemStatus {
    /// Modem has internet connectivity (passed health check).
    Online,
    /// Modem failed health checks.
    Offline,
    /// Health check in progress.
    Checking,
    /// Modem in standby — last resort only, health-checked but not in normal rotation.
    Standby,
    /// No SIM card detected in current slot — health checks skipped.
    NoSim,
}

/// Detected firewall backend on the OpenWRT system.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum FirewallBackend {
    Fw3,
    Fw4,
    #[default]
    Unknown,
}

/// Platform capabilities detected at startup.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformCapabilities {
    /// iproute2 `ip rule` command is functional
    pub policy_routing_available: bool,
    /// Available AND no mwan3 conflict — full policy routing active
    pub policy_routing_enabled: bool,
    /// Detected firewall backend (fw3/fw4/unknown)
    pub firewall_backend: FirewallBackend,
    /// mwan3 rules or init script detected
    pub mwan3_detected: bool,
    /// OpenWRT version string if available
    pub openwrt_version: Option<String>,
}

impl Default for PlatformCapabilities {
    fn default() -> Self {
        Self {
            policy_routing_available: false,
            policy_routing_enabled: false,
            firewall_backend: FirewallBackend::Unknown,
            mwan3_detected: false,
            openwrt_version: None,
        }
    }
}

/// Per-WAN routing table state tracked in memory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingTableEntry {
    /// Routing table number (100, 101, ...)
    pub table_number: u32,
    /// ip rule priority (1000, 1001, ...)
    pub rule_priority: u32,
    /// DHCP-learned gateway IP (None if point-to-point)
    pub gateway: Option<String>,
    /// Linux network device (e.g., "usb0")
    pub device: String,
    /// Interface source IP for ip rule
    pub source_ip: String,
}

/// A failover event in the history log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailoverEvent {
    pub timestamp: String,
    pub from_modem_id: String,
    pub from_label: String,
    pub to_modem_id: String,
    pub to_label: String,
    pub reason: String,
}

/// Result of a single health check cycle for one modem.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WanHealthCheckResult {
    pub timestamp: String,
    pub ping_ok: bool,
    pub dns_ok: bool,
    pub dns_v4_ok: bool,
    pub dns_v6_ok: bool,
    pub http_ok: bool,
    /// True if any check method succeeded.
    pub overall_ok: bool,
}

/// Per-modem/ethernet status in the WAN manager response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WanModemStatusEntry {
    pub modem_id: String,
    pub label: String,
    pub interface_name: String,
    pub network_device: String,
    pub state: WanModemState,
    pub metric: u32,
    pub status: WanModemStatus,
    /// Last health check result.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_check: Option<WanHealthCheckResult>,
    /// Consecutive failure count.
    pub consecutive_failures: u32,
    /// Whether this is currently the primary (lowest metric active modem).
    pub is_primary: bool,
    /// Entry type: modem or ethernet.
    #[serde(default)]
    pub entry_type: WanEntryType,
    /// For LAN ports converted to WAN, tracks the original bridge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_bridge: Option<String>,
    /// MTU override for this interface. None = system default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mtu: Option<u32>,
    /// IPv4 TTL mangle value. None = no mangle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl: Option<u8>,
    /// IPv6 Hop Limit mangle value. None = no mangle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hop_limit: Option<u8>,
    /// Carrier/operator name from the modem's connection status. None for Ethernet entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operator: Option<String>,
    /// IMEI from boot discovery. None for Ethernet entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imei: Option<String>,
    /// Whether watchdog restart has been suspended for this modem (max attempts reached).
    #[serde(default)]
    pub restart_suspended: bool,
    /// Current watchdog restart count for this modem.
    #[serde(default)]
    pub restart_count: u32,
    /// True when this modem WAN is in a persistent WDS-wedge (reboot required).
    #[serde(default)]
    pub wedged: bool,
    /// Load-balance weight for ECMP multipath (1–100). None = equal weight.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight: Option<u32>,
    /// Operator-set UCI `proto` override for this entry, mirrored from the
    /// corresponding `WanModemEntry`. None means the daemon picks based on
    /// detected USB-net mode (mode-agnostic principle — the resolved value
    /// is daemon-internal and is NOT surfaced here).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proto_override: Option<String>,
    /// Detected USB-net mode of this modem. `None` for Ethernet entries (no modem
    /// to query) or before detection has completed. DIAGNOSTIC ONLY — must not be
    /// surfaced in operator-facing UI per `feedback_modem_mode_agnostic.md`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usbnet_mode: Option<UsbNetMode>,
}

/// Information about an active failover override, exposed to the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailoverOverrideInfo {
    /// Whether a failover override is currently active.
    pub active: bool,
    /// The modem ID that the user configured as primary.
    pub original_primary_id: String,
    /// Human-readable label for the original primary.
    pub original_primary_label: String,
    /// The modem ID currently handling traffic.
    pub current_primary_id: String,
    /// Human-readable label for the current primary.
    pub current_primary_label: String,
    /// ISO 8601 timestamp of when the failover happened.
    pub failover_timestamp: String,
    /// Seconds remaining in the stabilization timer, or null if not stabilizing yet.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stabilization_remaining_secs: Option<u64>,
}

/// Full WAN manager state returned by GET /api/wan/status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WanStatusResponse {
    pub enabled: bool,
    pub routing_mode: RoutingMode,
    pub failover_locked: bool,
    pub modems: Vec<WanModemStatusEntry>,
    pub watchdog: WatchdogConfig,
    pub failover_history: Vec<FailoverEvent>,
    pub failback_timer_mins: u32,
    /// Active failover override info, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failover_override: Option<FailoverOverrideInfo>,
    pub platform: Option<PlatformCapabilities>,
    pub routing_tables: Option<HashMap<String, RoutingTableEntry>>,
}

/// An available Ethernet port that can be converted to WAN.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AvailableEthernetPort {
    /// Physical port name (e.g. "lan0", "lan3").
    pub port_name: String,
    /// Bridge this port currently belongs to (e.g. "br-lan").
    pub bridge: String,
    /// Link status: "up" or "down".
    pub link_status: String,
}

/// Scan response including modems and available Ethernet ports.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WanScanResponse {
    /// Full WAN status (modems + Ethernet entries in the priority list).
    #[serde(flatten)]
    pub status: WanStatusResponse,
    /// Ethernet ports available for conversion to WAN (not yet in the priority list).
    #[serde(default)]
    pub available_ethernet_ports: Vec<AvailableEthernetPort>,
}

/// Request body for POST /api/wan/add-ethernet.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddEthernetRequest {
    /// Physical port name to convert (e.g. "lan0").
    pub port_name: String,
    /// Optional human-readable label. Auto-generated if missing.
    #[serde(default)]
    pub label: Option<String>,
}

/// A single entry in the persistent watchdog recovery log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WanWatchdogLogEntry {
    pub timestamp: String,
    /// "FAILOVER" or "FAILBACK".
    pub action: String,
    /// Human-readable description of the event.
    pub details: String,
}

/// Response for GET /api/wan/watchdog/log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WanWatchdogLogResponse {
    pub entries: Vec<WanWatchdogLogEntry>,
    /// Most recent recovery action (if any).
    pub last_recovery: Option<WanWatchdogLogEntry>,
    pub retention_days: u32,
}

#[cfg(test)]
mod telemetry_snapshot_tests {
    use super::*;

    #[test]
    fn test_snapshot_serialization_with_new_fields() {
        let snapshot = TelemetrySnapshot {
            wan_id: "354832100459632".to_string(),
            wan_type: "modem".to_string(),
            bands: vec!["B2".to_string(), "B66".to_string()],
            network_type: None,
            modem_name: None,
            recorded_at: "2026-04-03T12:00:00Z".to_string(),
            rsrp: -85.0,
            rsrq: -10.0,
            sinr: 15.0,
            rssi: -65.0,
            technology: Some("4G".to_string()),
            operator: Some("AT&T".to_string()),
            connected: true,
            active_wan: Some("wwan0".to_string()),
            failover_event: false,
            device_uptime_secs: 86400,
            connection_uptime_secs: 3600,
        };

        let json = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(json["wan_id"], "354832100459632");
        assert_eq!(json["wan_type"], "modem");
        assert_eq!(json["bands"], serde_json::json!(["B2", "B66"]));
    }

    #[test]
    fn test_snapshot_deserialize_old_format_defaults() {
        let old_json = serde_json::json!({
            "recorded_at": "2026-04-03T12:00:00Z",
            "rsrp": -85.0,
            "rsrq": -10.0,
            "sinr": 15.0,
            "rssi": -65.0,
            "technology": "4G",
            "operator": "AT&T",
            "connected": true,
            "active_wan": "wwan0",
            "failover_event": false,
            "device_uptime_secs": 86400,
            "connection_uptime_secs": 3600,
        });

        let snapshot: TelemetrySnapshot = serde_json::from_value(old_json).unwrap();
        assert_eq!(snapshot.wan_id, "");
        assert_eq!(snapshot.wan_type, "");
        assert!(snapshot.bands.is_empty());
    }

    #[test]
    fn test_ethernet_snapshot_empty_bands_not_serialized() {
        let snapshot = TelemetrySnapshot {
            wan_id: "eth:br-wan".to_string(),
            wan_type: "ethernet".to_string(),
            bands: vec![],
            network_type: None,
            modem_name: None,
            recorded_at: "2026-04-03T12:00:00Z".to_string(),
            rsrp: 0.0,
            rsrq: 0.0,
            sinr: 0.0,
            rssi: 0.0,
            technology: None,
            operator: None,
            connected: true,
            active_wan: Some("br-wan".to_string()),
            failover_event: false,
            device_uptime_secs: 86400,
            connection_uptime_secs: 86400,
        };

        let json = serde_json::to_value(&snapshot).unwrap();
        assert!(json.get("bands").is_none());
        assert_eq!(json["wan_type"], "ethernet");
    }
}

#[cfg(test)]
mod tunnel_config_tests {
    use super::*;

    /// `TunnelConfig::default()` must construct from `enabled` + `ports` only.
    /// The `url` field has been removed in favour of
    /// `PortalConfig::resolved_tunnel_url()` (Phase 2 Task 2.5). Any future
    /// re-introduction of a `url` field on `TunnelConfig` would break the
    /// single-source-of-truth contract for portal URLs.
    #[test]
    fn default_tunnel_config_has_only_enabled_and_ports() {
        let cfg = TunnelConfig::default();
        assert!(cfg.enabled, "tunnel should default to enabled");
        assert_eq!(cfg.ports, vec![443, 8443]);

        // Round-trip through TOML and assert no `url` key leaks into the
        // serialized form. This protects the "drop TunnelConfig.url" contract.
        let serialized = toml::to_string(&cfg).unwrap();
        assert!(
            !serialized.contains("url"),
            "TunnelConfig must not serialize a url field, got: {serialized}"
        );
    }

    /// Legacy `config.toml` files written before Task 2.5 contain
    /// `[tunnel] url = "..."`. With no `#[serde(deny_unknown_fields)]` on
    /// `TunnelConfig`, that line must be silently ignored — existing routers
    /// upgrade without a config-rewrite step.
    #[test]
    fn legacy_tunnel_url_in_toml_is_silently_ignored() {
        let legacy = r#"
enabled = true
ports = [443, 8443]
url = "wss://portal.ctrl-modem.com/api/v1/tunnel"
"#;
        let cfg: TunnelConfig =
            toml::from_str(legacy).expect("legacy [tunnel].url must parse without error");
        assert!(cfg.enabled);
        assert_eq!(cfg.ports, vec![443, 8443]);
    }

    /// A whole `AppConfig` containing a legacy `[tunnel] url = "..."` must
    /// also deserialize cleanly. This is the realistic upgrade path: existing
    /// routers boot v1.2.0-dev.5 against an unmigrated `config.toml`.
    #[test]
    fn full_appconfig_with_legacy_tunnel_url_deserializes() {
        let legacy = r#"
auto_connect = false
signal_poll_interval = 2
telemetry_enabled = true

[tunnel]
enabled = true
ports = [443, 8443]
url = "wss://portal.ctrl-modem.com/api/v1/tunnel"
"#;
        let cfg: AppConfig =
            toml::from_str(legacy).expect("legacy AppConfig with [tunnel].url must parse");
        assert!(cfg.tunnel.enabled);
        assert_eq!(cfg.tunnel.ports, vec![443, 8443]);
    }
}

#[cfg(test)]
mod usbnet_mode_serde_tests {
    use super::UsbNetMode;

    #[test]
    fn serializes_lowercase() {
        assert_eq!(serde_json::to_string(&UsbNetMode::Qmi).unwrap(), "\"qmi\"");
        assert_eq!(serde_json::to_string(&UsbNetMode::Unknown).unwrap(), "\"unknown\"");
    }

    #[test]
    fn default_is_unknown() {
        assert_eq!(UsbNetMode::default(), UsbNetMode::Unknown);
    }
}

#[cfg(test)]
mod wan_modem_entry_proto_override_serde_tests {
    use super::{WanEntryType, WanModemEntry, WanModemState};

    #[test]
    fn wan_modem_entry_proto_override_roundtrip_some() {
        let entry = WanModemEntry {
            modem_id: "2c7c:0122:abcd".to_string(),
            label: "RM551E-GL".to_string(),
            interface_name: "WWAN".to_string(),
            network_device: "wwan0".to_string(),
            device_path: "/dev/ttyUSB2".to_string(),
            state: WanModemState::Active,
            metric: 10,
            entry_type: WanEntryType::Modem,
            original_bridge: None,
            mtu: None,
            ttl: None,
            hop_limit: None,
            weight: None,
            proto_override: Some("qmi".to_string()),
        };
        let json = serde_json::to_string(&entry).expect("serialize");
        // Field is present when Some.
        assert!(json.contains("\"proto_override\":\"qmi\""), "serialized JSON should contain proto_override: {json}");
        let parsed: WanModemEntry = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.proto_override.as_deref(), Some("qmi"));
        assert_eq!(parsed.modem_id, entry.modem_id);
        assert_eq!(parsed.interface_name, entry.interface_name);
    }

    #[test]
    fn wan_modem_entry_proto_override_legacy_missing_field_parses_none() {
        // Legacy wan-config.json files do not have proto_override at all.
        let legacy_json = r#"{
            "modem_id": "eth:br-wan",
            "label": "Ethernet WAN",
            "interface_name": "wan",
            "network_device": "br-wan",
            "device_path": "",
            "state": "active",
            "metric": 10,
            "entry_type": "ethernet"
        }"#;
        let parsed: WanModemEntry = serde_json::from_str(legacy_json).expect("legacy parse");
        assert!(parsed.proto_override.is_none(), "missing field must default to None");
        // And the field is omitted from re-serialization (skip_serializing_if).
        let json = serde_json::to_string(&parsed).expect("re-serialize");
        assert!(!json.contains("proto_override"), "skip_serializing_if must omit None: {json}");
    }
}

#[cfg(test)]
mod signal_strength_percent_tests {
    use super::{SignalInfo, UNAVAILABLE_DBM};

    fn signal(rssi: f64, rsrp: f64) -> SignalInfo {
        SignalInfo {
            rssi,
            rsrp,
            ..SignalInfo::default()
        }
    }

    #[test]
    fn rssi_present_matches_existing_formula_exactly() {
        // -65 dBm RSSI: (-65 + 113) * 100 / 62 = 4800 / 62 = 77.4 -> 77.
        // This is the unchanged historical behavior for modems that report RSSI.
        let s = signal(-65.0, -85.0);
        assert_eq!(s.signal_strength_percent(), 77);
        // RSSI wins even when RSRP is also available.
        assert_eq!(
            s.signal_strength_percent(),
            ((-65.0_f64 + 113.0) * 100.0 / 62.0).clamp(0.0, 100.0) as i32
        );
    }

    #[test]
    fn rssi_sentinel_falls_back_to_rsrp() {
        // RSSI unavailable (Quectel RM520N-GL in 5G mode), good RSRP -90:
        // (-90 + 140) * 100 / 96 = 5000 / 96 = 52.08 -> 52. Clearly non-zero.
        let s = signal(UNAVAILABLE_DBM, -90.0);
        assert_eq!(s.signal_strength_percent(), 52);
    }

    #[test]
    fn both_sentinels_yield_zero() {
        let s = signal(UNAVAILABLE_DBM, UNAVAILABLE_DBM);
        assert_eq!(s.signal_strength_percent(), 0);
    }

    #[test]
    fn rssi_sentinel_and_excellent_rsrp_clamps_to_100() {
        let s = signal(UNAVAILABLE_DBM, -44.0);
        assert_eq!(s.signal_strength_percent(), 100);
    }
}

#[cfg(test)]
mod wedge_config_tests {
    use super::*;

    #[test]
    fn watchdog_defaults_include_wedge_reboot_off() {
        let w = WatchdogConfig::default();
        assert!(!w.wedge_reboot_enabled);
        assert_eq!(w.wedge_reboot_grace_mins, 10);
        assert_eq!(w.wedge_reboot_max_per_day, 2);
        assert_eq!(w.wedge_reboot_min_uptime_mins, 15);
    }

    #[test]
    fn watchdog_config_deserializes_without_wedge_fields() {
        // Backward compat: old config files lack the new fields.
        let json = r#"{"enabled":true,"check_interval_secs":30,"failure_threshold":3,
            "ping_target":"8.8.8.8","dns_target":"google.com",
            "http_target":"http://x/generate_204","log_retention_days":14,
            "restart_on_failure":false,"restart_cooldown_mins":5,"max_restart_attempts":5}"#;
        let w: WatchdogConfig = serde_json::from_str(json).unwrap();
        assert!(!w.wedge_reboot_enabled);
        assert_eq!(w.wedge_reboot_grace_mins, 10);
    }
}

#[cfg(test)]
mod modem_wan_wedged_event_tests {
    use super::*;
    #[test]
    fn modem_wan_wedged_serializes_snake_case_tagged() {
        let e = ModemEvent::ModemWanWedged {
            modem_id: "2c7c:0801:c1b889a".into(),
            label: "Quectel RM520N-GL".into(),
            restart_count: 5,
            message: "registered but data path unrecoverable after 5 restarts".into(),
        };
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["type"], "modem_wan_wedged");
        assert_eq!(v["payload"]["restart_count"], 5);
    }
}
