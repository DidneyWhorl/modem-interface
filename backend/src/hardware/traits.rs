//! Hardware abstraction traits.
//!
//! These traits define the contract between the API layer and hardware implementations.
//! The Hardware layer implements `ModemHardware` for real modems via AT commands;
//! the API layer provides a `MockHardware` implementation for development.

use async_trait::async_trait;
use thiserror::Error;

use super::types::*;

/// Errors from hardware operations.
#[derive(Debug, Error, Clone)]
#[allow(dead_code)]
pub enum HardwareError {
    #[error("No modem detected")]
    NoModem,

    #[error("Modem not ready: {0}")]
    NotReady(String),

    #[error("Device not found: {0}")]
    DeviceNotFound(String),

    #[error("Protocol error: {0}")]
    Protocol(String),

    #[error("Communication timeout")]
    Timeout,

    #[error("SIM error: {0}")]
    SimError(String),

    #[error("Command rejected: {0}")]
    CommandRejected(String),

    #[error("Permission denied: {0}")]
    PermissionDenied(String),

    #[error("IO error: {0}")]
    Io(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

impl From<std::io::Error> for HardwareError {
    fn from(e: std::io::Error) -> Self {
        HardwareError::Io(e.to_string())
    }
}

/// Result type for hardware operations.
pub type HardwareResult<T> = Result<T, HardwareError>;

/// Unified modem hardware interface.
///
/// This trait provides the complete API for modem interactions. The Hardware
/// layer implements this for real modems; the API layer provides a mock
/// implementation for development.
///
/// The API layer wraps all calls with appropriate timeouts:
/// - 5s for quick queries (signal, status, device_info, sim_status)
/// - 15s for state changes (connect, disconnect, pin operations)
/// - 60s for long operations (network_scan)
#[async_trait]
pub trait ModemHardware: Send + Sync {
    // =========================================================================
    // Device Information
    // =========================================================================

    /// Get device identification info (IMEI, manufacturer, model, firmware).
    async fn get_device_info(&self) -> HardwareResult<DeviceInfo>;

    // =========================================================================
    // Modem Status & Signal
    // =========================================================================

    /// Get current modem status including connection state and network info.
    async fn get_status(&self) -> HardwareResult<ModemStatus>;

    /// Get detailed signal metrics: RSSI, RSRP, RSRQ, SINR, band info.
    async fn get_signal(&self) -> HardwareResult<SignalInfo>;

    /// Get data transfer statistics for current session.
    async fn get_data_stats(&self) -> HardwareResult<DataStats>;

    // =========================================================================
    // Connection Management
    // =========================================================================

    /// Establish a data connection with the provided APN configuration.
    async fn connect(&self, config: &ConnectionConfig) -> HardwareResult<()>;

    /// Terminate the data connection.
    async fn disconnect(&self) -> HardwareResult<()>;

    /// Bring the data bearer back up using the APN already saved on the modem.
    ///
    /// Performs a pure radio cycle (`AT+CFUN=0` → wait ~1s → `AT+CFUN=1`) with
    /// **no** `CGDCONT` write. This is deliberately distinct from [`connect`](Self::connect),
    /// which writes a new PDP context (`AT+CGDCONT`) before cycling the radio.
    /// Use `reconnect` when the saved APN is correct and only the bearer needs
    /// to be re-established (e.g. recovery after a transient drop).
    async fn reconnect(&self) -> HardwareResult<()>;

    // =========================================================================
    // SIM Operations
    // =========================================================================

    /// Get SIM card status.
    async fn get_sim_status(&self) -> HardwareResult<SimStatus>;

    /// Verify PIN code.
    async fn verify_pin(&self, pin: &str) -> HardwareResult<()>;

    /// Change PIN code.
    async fn change_pin(&self, old_pin: &str, new_pin: &str) -> HardwareResult<()>;

    /// Enable PIN requirement.
    async fn enable_pin(&self, pin: &str) -> HardwareResult<()>;

    /// Disable PIN requirement.
    async fn disable_pin(&self, pin: &str) -> HardwareResult<()>;

    // =========================================================================
    // Network Operations
    // =========================================================================

    /// Get current network registration state.
    async fn get_registration(&self) -> HardwareResult<RegistrationState>;

    /// Scan for available networks.
    /// This is a slow operation (30-60+ seconds) that may cause temporary disconnection.
    async fn scan_networks(&self) -> HardwareResult<Vec<AvailableNetwork>>;

    /// Manually select a network. Pass None for automatic selection.
    async fn select_network(&self, operator_code: Option<&str>) -> HardwareResult<()>;

    // =========================================================================
    // AT Commands
    // =========================================================================

    /// Execute an AT command and return the response.
    /// The command should already be validated against the whitelist by the API layer.
    async fn execute_at(&self, command: &str) -> HardwareResult<String>;

    // =========================================================================
    // GPS Operations (optional — default returns "not supported")
    // =========================================================================

    /// Get current GPS position. Returns GpsInfo with has_fix=false if no fix.
    async fn get_gps_position(&self) -> HardwareResult<GpsInfo> {
        Err(HardwareError::Internal("GPS not supported by this modem".to_string()))
    }

    /// Stop the GPS engine.
    async fn stop_gps(&self) -> HardwareResult<()> {
        Err(HardwareError::Internal("GPS not supported by this modem".to_string()))
    }

    // =========================================================================
    // Extended Signal Info (optional — default returns empty data)
    // =========================================================================

    /// Get extended signal information (carrier aggregation, network info, neighbour cells, AMBR).
    async fn get_extended_signal(&self) -> HardwareResult<ExtendedSignalInfo> {
        Ok(ExtendedSignalInfo::default())
    }

    // =========================================================================
    // Antenna Metrics (optional — default returns empty data)
    // =========================================================================

    /// Get per-antenna-port signal metrics (RSRP, RSRQ, SINR per RX port).
    async fn get_antenna_metrics(&self) -> HardwareResult<AntennaMetrics> {
        Ok(AntennaMetrics::default())
    }

    // =========================================================================
    // Cache Support (default impls — override in AtHandler for efficiency)
    // =========================================================================

    /// One-time discovery read: device info + SIM status.
    /// Called once at modem initialization. Result is cached for the
    /// lifetime of the modem session (never re-polled).
    #[allow(dead_code)] // consumed by Backend session (boot-time discovery)
    async fn get_discovery_info(&self) -> HardwareResult<DiscoveryInfo> {
        let device_info = self.get_device_info().await?;
        let sim_status = self.get_sim_status().await?;
        Ok(DiscoveryInfo { device_info, sim_status })
    }

    /// Get connection state without signal metrics (no AT+CSQ).
    /// Used by the cache refresh task alongside get_signal() to avoid
    /// issuing duplicate CSQ commands.
    #[allow(dead_code)] // consumed by Backend session (cache refresh task)
    async fn get_connection_status(&self) -> HardwareResult<ConnectionStatus> {
        let s = self.get_status().await?;
        Ok(ConnectionStatus {
            connected: s.connected,
            technology: s.technology,
            operator: s.operator,
            ip_address: s.ip_address,
        })
    }

    // =========================================================================
    // Live device-path handle (optional — default None)
    // =========================================================================

    /// Return a shared handle to the AT port path this handler is *actually*
    /// using right now. The real `AtHandler` owns a small thread-safe cell that
    /// `reopen_port` updates on every successful self-heal, so the value tracks
    /// USB re-enumeration (ttyUSB2↔ttyUSB3). The state layer clones this handle
    /// and reconciles it into the canonical `device_path` records once per 60s
    /// cache cycle. Implementations that have no stable serial fd (the mock)
    /// return `None` (also the default), making the reconcile a no-op.
    #[allow(dead_code)] // consumed by Backend session (60s cache task device_path reconcile)
    fn live_device_path_handle(&self) -> Option<std::sync::Arc<std::sync::Mutex<String>>> {
        None
    }
}

/// Information about a detected modem device.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DetectedModem {
    /// Device path (e.g., "/dev/cdc-wdm0", "/dev/ttyUSB2")
    pub device_path: String,
    /// Protocol type
    pub protocol: ModemProtocol,
    /// Human-readable description
    pub description: String,
    /// USB vendor ID (if detected from sysfs)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vendor_id: Option<String>,
    /// USB product ID (if detected from sysfs)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub product_id: Option<String>,
    /// Matched profile ID (e.g. "quectel_rm551e_gl" or "generic")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    /// Whether a specific (non-generic) profile was matched
    #[serde(default)]
    pub has_profile: bool,
    /// USB bus-port identifier (e.g. "4-1") for network device mapping.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bus_port: Option<String>,
    /// All ttyUSB ports in this modem's bus-port group, for fallback AT port probing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub all_ports: Vec<String>,
}

/// Supported modem communication protocols.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModemProtocol {
    Qmi,
    Mbim,
    Mhi,
    At,
}

// =============================================================================
// Antenna Metrics Validation (always available, not feature-gated)
// =============================================================================

/// Parse an antenna metric value, filtering by sentinel and valid range.
///
/// Returns `None` if the value equals the sentinel, falls outside
/// `[valid_min, valid_max]`, or cannot be parsed as an integer.
#[allow(dead_code)] // used by real-hardware feature and tests
fn parse_antenna_val(s: &str, sentinel: i32, valid_min: i32, valid_max: i32) -> Option<i32> {
    let v: i32 = s.trim().parse().ok()?;
    if v == sentinel || !(valid_min..=valid_max).contains(&v) { None } else { Some(v) }
}

/// Derive the AT response prefix from a command string.
///
/// Example: `"AT+QRSRP"` -> `"+QRSRP:"`, `"AT+QSINR"` -> `"+QSINR:"`
#[allow(dead_code)] // used by real-hardware feature and tests
fn response_prefix_from_cmd(cmd: &str) -> String {
    let stripped = cmd.strip_prefix("AT").unwrap_or(cmd);
    format!("{stripped}:")
}

/// Parse multi-row antenna metric response (unified for RSRP, SINR, RSRQ).
///
/// Each row: `<prefix> <rx0>,<rx1>,<rx2>,<rx3>[,<rat>]`
///
/// The prefix is the AT response prefix (e.g. `"+QRSRP:"`).
/// Values equal to sentinel or outside `[valid_min, valid_max]` become `None`.
/// The technology tag is extracted from position 5 (index 4) if present.
#[allow(dead_code)] // used by real-hardware feature and tests
fn parse_antenna_metric_multi(
    response: &str,
    prefix: &str,
    sentinel: i32,
    valid_min: i32,
    valid_max: i32,
) -> Vec<(String, [Option<i32>; 4])> {
    let mut results = Vec::new();
    for line in response.lines().filter(|l| l.contains(prefix)) {
        let data = line.split(':').nth(1).unwrap_or("");
        let parts: Vec<&str> = data.split(',').collect();
        let mut vals = [None; 4];
        for (i, val) in parts.iter().take(4).enumerate() {
            vals[i] = parse_antenna_val(val, sentinel, valid_min, valid_max);
        }
        let tech = parts.get(4).map(|s| s.trim().to_string()).unwrap_or_default();
        results.push((tech, vals));
    }
    results
}

/// Parse interleaved RSRP/RSRQ antenna metric response (Telit AT#LAPS / AT#NRAPS format).
///
/// Format: `<prefix> rsrp_rx0, rsrq_rx0, rsrp_rx1, rsrq_rx1[, rsrp_rx2, rsrq_rx2, rsrp_rx3, rsrq_rx3]`
///
/// Returns `(rsrp_vals, rsrq_vals)` — each is an array of 4 Option<i32>.
/// No technology tag is present in Telit responses (single technology per command).
#[allow(dead_code)] // used by real-hardware feature and tests
fn parse_antenna_interleaved(
    response: &str,
    prefix: &str,
    sentinel: i32,
    rsrp_min: i32,
    rsrp_max: i32,
    rsrq_min: i32,
    rsrq_max: i32,
) -> ([Option<i32>; 4], [Option<i32>; 4]) {
    let mut rsrp = [None; 4];
    let mut rsrq = [None; 4];
    if let Some(line) = response.lines().find(|l| l.contains(prefix)) {
        let data = line.split(':').nth(1).unwrap_or("");
        let parts: Vec<&str> = data.split(',').collect();
        // Values are interleaved: rsrp0, rsrq0, rsrp1, rsrq1, ...
        for port in 0..4 {
            let rsrp_idx = port * 2;
            let rsrq_idx = port * 2 + 1;
            if let Some(val) = parts.get(rsrp_idx) {
                rsrp[port] = parse_antenna_val(val, sentinel, rsrp_min, rsrp_max);
            }
            if let Some(val) = parts.get(rsrq_idx) {
                rsrq[port] = parse_antenna_val(val, sentinel, rsrq_min, rsrq_max);
            }
        }
    }
    (rsrp, rsrq)
}

// =============================================================================
// Carrier Aggregation Parsers (module-scope for testability)
// =============================================================================

use super::profiles::BandPrefixMapping;

/// Normalize a band name using prefix-to-short-form mappings.
/// First matching prefix wins. E.g. "LTE BAND 7" → "B7" with mapping ("LTE BAND ", "B").
#[allow(dead_code)] // used by real-hardware feature and tests
fn normalize_band_name(raw: &str, mappings: &[BandPrefixMapping]) -> String {
    let trimmed = raw.trim();
    for mapping in mappings {
        if let Some(n) = trimmed.strip_prefix(&mapping.prefix) {
            return format!("{}{}", mapping.replacement, n);
        }
    }
    trimmed.to_string()
}

/// Parse CA info response, returning SCC (secondary) carriers as SignalInfo.
///
/// LTE format:  +QCAINFO: "PCC|SCC",<earfcn>,<bw_rb>,"LTE BAND X",<mimo>,<pcid>,<rsrp>,<rsrq>,<rssi>,<sinr>[,...]
/// NR5G short:  +QCAINFO: "SCC",<nrarfcn>,<bw_idx>,"NR5G BAND X",<pcid>
/// NR5G long:   +QCAINFO: "SCC",<nrarfcn>,<bw_idx>,"NR5G BAND X",<mimo>,<pcid>,<rsrp>,<-|val>,<-|val>
#[allow(dead_code)] // used by real-hardware feature and tests
fn parse_qcainfo_secondary(
    response: &str,
    lte_re: Option<&regex::Regex>,
    nr_re: Option<&regex::Regex>,
    band_prefix_mappings: &[BandPrefixMapping],
) -> Vec<SignalInfo> {
    let mut secondaries = vec![];

    for line in response.lines() {
        let line = line.trim();
        if !line.contains("+QCAINFO:") || !line.contains("SCC") {
            continue;
        }

        // Try LTE pattern first
        if let Some(re) = lte_re {
            if let Some(caps) = re.captures(line) {
                if caps.get(1).map(|m| m.as_str()) == Some("SCC") {
                    let band_raw = caps.get(4).map(|m| m.as_str().trim()).unwrap_or("");
                    let band = normalize_band_name(band_raw, band_prefix_mappings);
                    secondaries.push(SignalInfo {
                        rsrp: caps.get(6).and_then(|m| m.as_str().parse::<f64>().ok()).unwrap_or(-999.0),
                        rsrq: caps.get(7).and_then(|m| m.as_str().parse::<f64>().ok()).unwrap_or(-999.0),
                        rssi: caps.get(8).and_then(|m| m.as_str().parse::<f64>().ok()).unwrap_or(-999.0),
                        sinr: caps.get(9).and_then(|m| m.as_str().parse::<f64>().ok()).unwrap_or(0.0),
                        band,
                        cell_id: String::new(),
                        technology: None,
                    });
                }
                continue;
            }
        }

        // Try NR5G pattern
        if let Some(re) = nr_re {
            if let Some(caps) = re.captures(line) {
                if caps.get(1).map(|m| m.as_str()) == Some("SCC") {
                    let remaining = caps.get(5).map(|m| m.as_str()).unwrap_or("");
                    let fields: Vec<&str> = remaining.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()).collect();

                    let (rsrp, rsrq, sinr) = if fields.len() >= 3 {
                        (
                            fields.get(2).and_then(|s| s.parse::<f64>().ok()).unwrap_or(-999.0),
                            fields.get(3).and_then(|s| s.parse::<f64>().ok()).unwrap_or(-999.0),
                            fields.get(4).and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0),
                        )
                    } else {
                        (-999.0, -999.0, 0.0)
                    };

                    let band_raw = caps.get(4).map(|m| m.as_str().trim()).unwrap_or("");
                    let band = normalize_band_name(band_raw, band_prefix_mappings);
                    secondaries.push(SignalInfo {
                        rsrp,
                        rsrq,
                        rssi: -999.0,
                        sinr,
                        band,
                        cell_id: String::new(),
                        technology: None,
                    });
                }
            }
        }
    }

    secondaries
}

/// Parse CA info response, returning the PCC (primary component carrier) as SignalInfo.
///
/// Same regex patterns as SCC parsing, but filters for "PCC" lines instead of "SCC".
/// Returns `Some(SignalInfo)` if a PCC line is found, `None` otherwise.
///
/// LTE format:  +QCAINFO: "PCC",<earfcn>,<bw_rb>,"LTE BAND X",<mimo>,<pcid>,<rsrp>,<rsrq>,<rssi>,<sinr>[,...]
/// NR5G long:   +QCAINFO: "PCC",<nrarfcn>,<bw_idx>,"NR5G BAND X",<mimo>,<pcid>,<rsrp>,<-|val>,<-|val>
/// NR5G short:  +QCAINFO: "PCC",<nrarfcn>,<bw_idx>,"NR5G BAND X",<pcid>
#[allow(dead_code)] // used by real-hardware feature and tests
fn parse_qcainfo_pcc(
    response: &str,
    lte_re: Option<&regex::Regex>,
    nr_re: Option<&regex::Regex>,
    band_prefix_mappings: &[BandPrefixMapping],
) -> Option<SignalInfo> {
    for line in response.lines() {
        let line = line.trim();
        if !line.contains("+QCAINFO:") {
            continue;
        }
        // Case-insensitive check for PCC
        let upper = line.to_uppercase();
        if !upper.contains("\"PCC\"") {
            continue;
        }

        // Try LTE pattern first
        if let Some(re) = lte_re {
            if let Some(caps) = re.captures(line) {
                if caps.get(1).map(|m| m.as_str()) == Some("PCC") {
                    let band_raw = caps.get(4).map(|m| m.as_str().trim()).unwrap_or("");
                    let band = normalize_band_name(band_raw, band_prefix_mappings);
                    return Some(SignalInfo {
                        rsrp: caps.get(6).and_then(|m| m.as_str().parse::<f64>().ok()).unwrap_or(-999.0),
                        rsrq: caps.get(7).and_then(|m| m.as_str().parse::<f64>().ok()).unwrap_or(-999.0),
                        rssi: caps.get(8).and_then(|m| m.as_str().parse::<f64>().ok()).unwrap_or(-999.0),
                        sinr: caps.get(9).and_then(|m| m.as_str().parse::<f64>().ok()).unwrap_or(0.0),
                        band,
                        cell_id: String::new(),
                        technology: None,
                    });
                }
            }
        }

        // Try NR5G pattern
        if let Some(re) = nr_re {
            if let Some(caps) = re.captures(line) {
                if caps.get(1).map(|m| m.as_str()) == Some("PCC") {
                    let remaining = caps.get(5).map(|m| m.as_str()).unwrap_or("");
                    let fields: Vec<&str> = remaining.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()).collect();

                    let (rsrp, rsrq, sinr) = if fields.len() >= 3 {
                        (
                            fields.get(2).and_then(|s| s.parse::<f64>().ok()).unwrap_or(-999.0),
                            fields.get(3).and_then(|s| s.parse::<f64>().ok()).unwrap_or(-999.0),
                            fields.get(4).and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0),
                        )
                    } else {
                        (-999.0, -999.0, 0.0)
                    };

                    let band_raw = caps.get(4).map(|m| m.as_str().trim()).unwrap_or("");
                    let band = normalize_band_name(band_raw, band_prefix_mappings);
                    return Some(SignalInfo {
                        rsrp,
                        rsrq,
                        rssi: -999.0,
                        sinr,
                        band,
                        cell_id: String::new(),
                        technology: None,
                    });
                }
            }
        }
    }

    None
}

/// Parse network type response, returning just the access technology string.
/// Format: +QNWINFO: "tech","operator","band_desc",channel
#[allow(dead_code)] // used by real-hardware feature and tests
fn parse_qnwinfo_type(response: &str, re: Option<&regex::Regex>) -> Option<String> {
    let re = re?;
    let caps = re.captures(response)?;
    Some(caps.get(1)?.as_str().to_string())
}

// =============================================================================
// Telit CA Parser (AT#CAINFO?)
// =============================================================================

/// Convert a Telit band_class value to a band name string and technology.
///
/// LTE: band_class 120=B1, 121=B2, 122=B3... (subtract 119).
/// NR:  band_class 250=n1, 251=n2, 252=n3... (subtract 249).
/// Returns `(band_name, Option<Technology>)`.
#[allow(dead_code)] // used by real-hardware feature and tests
fn telit_band_class_to_name(band_class: u32) -> (String, Option<Technology>) {
    if band_class >= 250 {
        let nr_band = band_class - 249;
        (format!("n{nr_band}"), Some(Technology::Gen5))
    } else if band_class >= 120 {
        let lte_band = band_class - 119;
        (format!("B{lte_band}"), Some(Technology::Gen4))
    } else {
        (format!("?{band_class}"), None)
    }
}

/// Convert Telit SINR encoded value to dB.
///
/// Telit encoding: value 0 = -20 dB, value 250 = +30 dB (linear: dB = value * 0.2 - 20).
#[allow(dead_code)] // used by real-hardware feature and tests
fn telit_sinr_to_db(raw: i32) -> f64 {
    raw as f64 * 0.2 - 20.0
}

/// Parse Telit AT#CAINFO? response, returning PCC and SCC carriers.
///
/// Format (CSV, no quotes around band names):
/// `#CAINFO: <band_class>,<earfcn>,<dl_bw>,<pci>,<rsrp>,<rssi>,<rsrq>,<sinr>,<tac>,<tx_power>,...`
///
/// PCC line is the first #CAINFO line. SCC lines follow with similar format.
/// ENDC lines are prefixed with `ENDC:` and use NR band_class encoding.
///
/// Returns `(pcc, secondaries)`.
#[allow(dead_code)] // used by real-hardware feature and tests
fn parse_telit_cainfo(
    response: &str,
) -> (Option<SignalInfo>, Vec<SignalInfo>) {
    let mut pcc: Option<SignalInfo> = None;
    let mut secondaries = Vec::new();

    for line in response.lines() {
        let line = line.trim();

        // Check for ENDC prefix (NSA 5G secondary)
        let (data_str, is_endc) = if let Some(rest) = line.strip_prefix("ENDC:") {
            (rest.trim(), true)
        } else if line.contains("#CAINFO:") {
            let data = line.split(':').nth(1).unwrap_or("");
            (data.trim(), false)
        } else {
            continue;
        };

        let parts: Vec<&str> = data_str.split(',').map(|s| s.trim()).collect();
        if parts.len() < 8 {
            continue;
        }

        let band_class: u32 = match parts[0].parse() {
            Ok(v) => v,
            Err(_) => continue,
        };
        let earfcn: u64 = parts[1].parse().unwrap_or(0);
        let pci: u32 = parts[3].parse().unwrap_or(0);
        let rsrp: f64 = parts[4].parse().unwrap_or(-999.0);
        let rssi: f64 = parts[5].parse().unwrap_or(-999.0);
        let rsrq: f64 = parts[6].parse().unwrap_or(-999.0);
        let sinr_raw: i32 = parts[7].parse().unwrap_or(0);
        let sinr = telit_sinr_to_db(sinr_raw);

        let (band_name, tech) = if is_endc {
            // ENDC lines always use NR band encoding
            telit_band_class_to_name(band_class.max(250))
        } else {
            telit_band_class_to_name(band_class)
        };

        let info = SignalInfo {
            rsrp,
            rsrq,
            rssi,
            sinr,
            band: band_name,
            cell_id: format!("{pci}"),
            technology: tech,
        };

        // Suppress default EARFCN=0, PCI=0 entries that are clearly placeholder SCCs.
        // Telit pads #CAINFO with zero-filled SCC slots; ignore them.
        let _ = earfcn; // earfcn is used for the zero-check below
        if pcc.is_none() && !is_endc {
            pcc = Some(info);
        } else if earfcn == 0 && pci == 0 && rsrp == 0.0 && rssi == 0.0 {
            // Skip zero-filled placeholder SCC entries
            continue;
        } else {
            secondaries.push(info);
        }
    }

    (pcc, secondaries)
}

// =============================================================================
// GPS Coordinate Conversion
// =============================================================================

/// Convert NMEA coordinate format (DDMM.MMMM) to decimal degrees.
/// Formula: DD + (MM.MMMM / 60.0)
///
/// Example: 3723.2475 → 37 + (23.2475 / 60.0) = 37.387458
#[allow(dead_code)] // used by real-hardware feature and tests
fn nmea_to_decimal(nmea: f64) -> f64 {
    let degrees = (nmea / 100.0).floor();
    let minutes = nmea - (degrees * 100.0);
    degrees + (minutes / 60.0)
}

// =============================================================================
// QICSGP Password Redaction
//
// AT+QICSGP write commands (TX) and +QICSGP: query responses (RX) carry the
// PDP password in cleartext.  These must never appear verbatim in debug! tracing
// output or the debug_trace UI ring buffer.
//
// `redact_qicsgp` is NOT gated behind `#[cfg(feature = "real-hardware")]` so
// that its unit tests compile and run under the default (mock-hardware) feature
// set used by CI.  The call sites inside `send_command` are behind the gate.
// Same pattern as `nmea_to_decimal` and `should_reopen_after_io_error`.
// =============================================================================

/// Regex matching the password field in an `AT+QICSGP=` write command.
///
/// Write format: `AT+QICSGP=<cid>,<type>,"<apn>","<user>","<password>",<auth>`
/// The password is the 5th positional field (4th quoted value).
/// Capture group 1: everything up to and including the opening quote of the
/// password field.  Capture group 2: the closing `",<auth>` suffix.
static QICSGP_WRITE_RE: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| {
        regex::Regex::new(
            r#"(?i)(AT\+QICSGP=\d+,\d+,"[^"]*","[^"]*",)"[^"]*"(,\d+)"#,
        )
        .unwrap()
    });

/// Regex matching the password field in a `+QICSGP:` query response line.
///
/// Response format: `+QICSGP: <type>,"<apn>","<user>","<password>",<auth>`
/// The password is the 4th positional field (3rd quoted value).
/// Capture group 1: everything up to and including the opening quote of the
/// password field.  Capture group 2: the closing `",<auth>` suffix.
static QICSGP_RESP_RE: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| {
        regex::Regex::new(
            r#"(?i)(\+QICSGP:\s*\d+,"[^"]*","[^"]*",)"[^"]*"(,\d+)"#,
        )
        .unwrap()
    });

/// Redact the PDP password from an `AT+QICSGP` write command or `+QICSGP:`
/// response string before it is passed to `debug!` / `debug_trace`.
///
/// - The password field's value is replaced with `"<redacted>"`.
/// - All other fields (cid, type, apn, username, auth) are left intact.
/// - Non-QICSGP lines pass through completely unchanged.
/// - Multi-line strings are processed line-by-line; only matching lines are
///   altered.
/// - An empty password (`""`) is also replaced (preserves debug context).
#[allow(dead_code)] // call sites are inside `#[cfg(feature = "real-hardware")]`
pub(crate) fn redact_qicsgp(input: &str) -> String {
    // Fast-path: if neither marker is present there is nothing to redact.
    let upper = input.to_ascii_uppercase();
    if !upper.contains("AT+QICSGP") && !upper.contains("+QICSGP:") {
        return input.to_string();
    }

    // Process line-by-line so that trailing "OK" and other response lines in a
    // multi-line AT exchange are left completely unchanged.
    let lines: Vec<&str> = input.split('\n').collect();
    let mut out = Vec::with_capacity(lines.len());
    for line in lines {
        let stripped = line.trim_end_matches('\r');
        let upper_line = stripped.to_ascii_uppercase();
        if upper_line.contains("AT+QICSGP=") {
            // Write command: redact the password (5th field, 4th quoted value).
            let redacted = QICSGP_WRITE_RE
                .replace(stripped, r#"${1}"<redacted>"${2}"#)
                .into_owned();
            // Re-attach the \r if it was present.
            if line.ends_with('\r') {
                out.push(redacted + "\r");
            } else {
                out.push(redacted);
            }
        } else if upper_line.contains("+QICSGP:") {
            // Response line: redact the password (4th field, 3rd quoted value).
            let redacted = QICSGP_RESP_RE
                .replace(stripped, r#"${1}"<redacted>"${2}"#)
                .into_owned();
            if line.ends_with('\r') {
                out.push(redacted + "\r");
            } else {
                out.push(redacted);
            }
        } else {
            out.push(line.to_string());
        }
    }
    out.join("\n")
}

// =============================================================================
// UTF-8-safe diagnostic truncation
//
// AT responses are usually ASCII, but a `+COPS` operator name or garbage bytes
// during a USB re-enumeration can contain multibyte UTF-8.  Slicing a `&str` at
// a raw byte index that lands mid-codepoint panics ("byte index N is not a char
// boundary").  These are the self-heal / parse-failure diagnostic paths, so a
// panic there is doubly bad.  `truncate_on_char_boundary` backs the cut down to
// the nearest valid char boundary instead.
//
// NOT gated behind `#[cfg(feature = "real-hardware")]` (same rationale as
// `redact_qicsgp` above) so its unit tests run under CI's default mock-hardware
// build.  Its only call sites are inside the gated impl, hence `dead_code`.
// =============================================================================

/// Return the longest prefix of `s` that is at most `max_bytes` long and ends
/// on a UTF-8 char boundary.
///
/// - If `s.len() <= max_bytes`, the whole string is returned unchanged.
/// - Otherwise the cut point is the largest char boundary `<= max_bytes`,
///   so the result is always a valid `&str` and never panics.
#[allow(dead_code)] // call sites are inside `#[cfg(feature = "real-hardware")]`
fn truncate_on_char_boundary(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    // Walk down from max_bytes to the nearest char boundary. `is_char_boundary`
    // is true at 0 and at s.len(), so this terminates (worst case at 0).
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

// =============================================================================
// Real Hardware Implementation
// =============================================================================

#[cfg(feature = "real-hardware")]
pub mod real_hardware {
    use super::*;
    use super::super::profiles::{GpsCoordinateFormat, ModemProfile, SignalFormatVariant};
    use glob::glob;
    use std::collections::{HashMap, HashSet};
    use std::fs;
    use std::path::Path;
    use std::sync::LazyLock;
    use std::time::Duration;
    use tokio::sync::Mutex;
    use tracing::{debug, info, warn, error};

    // 3GPP-standard regex patterns compiled once (modem-agnostic, never change)
    static CSQ_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"\+CSQ:\s*(\d+),").unwrap()
    });
    static COPS_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r#"\+COPS:\s*\d+(?:,\d+,"([^"]*)"(?:,(\d+))?)?"#).unwrap()
    });
    static CEREG_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"\+C[E5G]?REG:\s*\d+,(\d+)").unwrap()
    });
    static CGPADDR_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r#"\+CGPADDR:\s*\d+,"([^"]+)""#).unwrap()
    });

    /// Default timeout for AT commands
    const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);
    const LONG_TIMEOUT: Duration = Duration::from_secs(60);

    /// Convert a 3GPP dotted-decimal IPv6 address (16 octets) to standard colon-hex notation.
    /// If the input is not 16 octets (e.g. it's already IPv4), return it unchanged.
    fn normalize_cgpaddr_ip(raw: &str) -> String {
        let parts: Vec<&str> = raw.split('.').collect();
        if parts.len() != 16 {
            return raw.to_string();
        }
        // Try to parse all 16 parts as u8
        let octets: Option<Vec<u8>> = parts.iter().map(|p| p.parse::<u8>().ok()).collect();
        match octets {
            Some(bytes) => {
                // Combine pairs of octets into 8 hex groups
                (0..8)
                    .map(|i| format!("{:02x}{:02x}", bytes[i * 2], bytes[i * 2 + 1]))
                    .collect::<Vec<_>>()
                    .join(":")
            }
            None => raw.to_string(),
        }
    }

    // The real serial port satisfies the module-scope `SerialIo` seam used by
    // the reopen-once retry driver (`run_at_command_with_reopen`).
    impl super::SerialIo for serialport::TTYPort {}

    /// Real `PortOpener` for the reopen-once driver: delegates to
    /// `AtHandler::reopen_port`, which re-detects the AT port for the handler's
    /// stable bus-port and returns a verified fresh `TTYPort`.
    struct HandlerPortOpener<'a> {
        handler: &'a AtHandler,
    }

    impl super::PortOpener for HandlerPortOpener<'_> {
        type Port = serialport::TTYPort;
        fn open(&mut self) -> HardwareResult<serialport::TTYPort> {
            self.handler.reopen_port()
        }
    }

    /// AT command handler using serial port communication.
    ///
    /// Driven by a `ModemProfile` that specifies which AT commands to use,
    /// how to parse responses, and which serial port to prefer.
    pub struct AtHandler {
        _device_path: String,
        port: Mutex<serialport::TTYPort>,
        profile: ModemProfile,
        /// Pre-compiled signal format variants from profile
        signal_variants: Vec<(SignalFormatVariant, regex::Regex)>,
        /// Compiled regex for vendor-specific operator name parsing
        operator_regex: Option<regex::Regex>,
        /// Compiled regex for ICCID parsing
        iccid_regex: Option<regex::Regex>,
        /// Compiled regex for LTE secondary carrier parsing (from profile ca_config)
        ca_lte_regex: Option<regex::Regex>,
        /// Compiled regex for NR5G secondary carrier parsing (from profile ca_config)
        ca_nr5g_regex: Option<regex::Regex>,
        /// Compiled regex for network type parsing (from profile ca_config)
        ca_nwinfo_regex: Option<regex::Regex>,
        /// Compiled regex for firmware version parsing (from profile firmware_config)
        firmware_regex: Option<regex::Regex>,
        /// Compiled regex for GPS position parsing (from profile gps_config)
        gps_regex: Option<regex::Regex>,
        /// USB bus-port path for this modem (e.g. "4-1"), used for network interface detection
        bus_port: Option<String>,
        /// Tracks when the modem was first seen connected (for uptime calculation)
        connect_time: std::sync::Mutex<Option<std::time::Instant>>,
        /// Live AT-port path this handler is actually using. Initialised to the
        /// port `new` opened; rewritten by `reopen_port` on every self-heal. A
        /// SEPARATE handle from `port` (not behind the `port` mutex) so state-side
        /// reads never contend with an in-flight AT exchange. Cloned out via
        /// `live_device_path_handle` for the cache-task reconcile.
        live_device_path: std::sync::Arc<std::sync::Mutex<String>>,
    }

    impl AtHandler {
        /// Create a new AT handler for the given serial port with a modem profile.
        ///
        /// `bus_port` is the USB bus-port path (e.g. "4-1") used for dynamic
        /// network interface detection. Pass `None` to fall back to hardcoded
        /// interface names.
        pub fn new(device_path: &str, profile: ModemProfile, bus_port: Option<String>) -> HardwareResult<Self> {
            let port = serialport::new(device_path, profile.port_mapping.baud_rate)
                .timeout(Duration::from_millis(100))
                .open_native()
                .map_err(|e| HardwareError::DeviceNotFound(format!("{device_path}: {e}")))?;

            // Pre-compile signal format variants from profile
            let signal_variants: Vec<(SignalFormatVariant, regex::Regex)> = profile
                .signal_parse_config
                .variants
                .iter()
                .filter_map(|v| {
                    match regex::Regex::new(&v.regex) {
                        Ok(re) => Some((v.clone(), re)),
                        Err(e) => {
                            warn!("Signal variant '{}' regex failed to compile: {}", v.label, e);
                            None
                        }
                    }
                })
                .collect();

            // Pre-compile other profile regexes once
            let operator_regex = profile.commands.operator_name_regex.as_ref()
                .and_then(|p| regex::Regex::new(p).ok());
            let iccid_regex = profile.commands.iccid_regex.as_ref()
                .and_then(|p| regex::Regex::new(p).ok());
            let ca_lte_regex = profile.ca_config.lte_scc_regex.as_ref()
                .and_then(|p| regex::Regex::new(p).ok());
            let ca_nr5g_regex = profile.ca_config.nr5g_scc_regex.as_ref()
                .and_then(|p| regex::Regex::new(p).ok());
            let ca_nwinfo_regex = profile.ca_config.network_type_regex.as_ref()
                .and_then(|p| regex::Regex::new(p).ok());
            let firmware_regex = profile.firmware_config.firmware_regex.as_ref()
                .and_then(|p| regex::Regex::new(p).ok());
            let gps_regex = profile.gps_config.query_regex.as_ref()
                .and_then(|p| regex::Regex::new(p).ok());

            Ok(Self {
                _device_path: device_path.to_string(),
                port: Mutex::new(port),
                profile,
                signal_variants,
                operator_regex,
                iccid_regex,
                ca_lte_regex,
                ca_nr5g_regex,
                ca_nwinfo_regex,
                firmware_regex,
                gps_regex,
                bus_port,
                connect_time: std::sync::Mutex::new(None),
                live_device_path: std::sync::Arc::new(std::sync::Mutex::new(device_path.to_string())),
            })
        }

        /// Synchronous AT verification — sends "AT" and checks for "OK".
        ///
        /// Uses `try_lock()` so it can run from a sync context (e.g. inside
        /// `spawn_blocking`). Must be called right after creation when no one
        /// else holds the port lock.
        fn verify_sync(&self) -> HardwareResult<()> {
            use std::io::{BufRead, BufReader, Write};

            let mut port = self.port.try_lock()
                .map_err(|_| HardwareError::Internal("Port lock contended during verify".into()))?;

            port.write_all(b"AT\r")
                .map_err(|e| HardwareError::Io(format!("Verify write failed: {e}")))?;
            port.flush()
                .map_err(|e| HardwareError::Io(format!("Verify flush failed: {e}")))?;

            let mut reader = BufReader::new(&mut *port);
            let start = std::time::Instant::now();
            let verify_timeout = Duration::from_secs(2);

            while start.elapsed() < verify_timeout {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) => break,
                    Ok(_) => {
                        let trimmed = line.trim();
                        if trimmed == "OK" {
                            return Ok(());
                        }
                        if trimmed == "ERROR"
                            || trimmed.starts_with("+CME ERROR")
                            || trimmed.starts_with("+CMS ERROR")
                        {
                            return Err(HardwareError::NotReady(
                                format!("AT verification returned: {trimmed}"),
                            ));
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::TimedOut => continue,
                    Err(e) => {
                        return Err(HardwareError::Io(format!("Verify read failed: {e}")));
                    }
                }
            }

            Err(HardwareError::Timeout)
        }

        /// Verify a freshly opened serial port answers "AT" with "OK".
        ///
        /// Distinct from `verify_sync` (which locks `self.port`): this verifies a
        /// candidate `TTYPort` that has NOT yet been swapped into the handler, so
        /// `reopen_port` can probe re-enumerated ports without touching the dead
        /// live fd. Bounded by a 2s read window (matches `verify_sync`).
        fn verify_fresh_port(port: &mut serialport::TTYPort) -> HardwareResult<()> {
            use std::io::{BufRead, BufReader, Write};

            port.write_all(b"AT\r")
                .map_err(|e| HardwareError::Io(format!("Reopen verify write failed: {e}")))?;
            port.flush()
                .map_err(|e| HardwareError::Io(format!("Reopen verify flush failed: {e}")))?;

            let mut reader = BufReader::new(&mut *port);
            let start = std::time::Instant::now();
            let verify_timeout = Duration::from_secs(2);

            while start.elapsed() < verify_timeout {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) => break,
                    Ok(_) => {
                        let trimmed = line.trim();
                        if trimmed == "OK" {
                            return Ok(());
                        }
                        if trimmed == "ERROR"
                            || trimmed.starts_with("+CME ERROR")
                            || trimmed.starts_with("+CMS ERROR")
                        {
                            return Err(HardwareError::NotReady(
                                format!("Reopen verification returned: {trimmed}"),
                            ));
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::TimedOut => continue,
                    Err(e) => {
                        return Err(HardwareError::Io(format!("Reopen verify read failed: {e}")));
                    }
                }
            }

            Err(HardwareError::Timeout)
        }

        /// Re-detect the AT port for this handler's stable USB bus-port, open it,
        /// and verify it answers AT/OK — returning the fresh `TTYPort`.
        ///
        /// WHY: a CFUN cycle / reboot re-enumerates the USB AT port
        /// (ttyUSB2↔ttyUSB3), killing the cached fd. The stable `bus_port` (set at
        /// construction) still identifies the same physical modem, so we rescan
        /// `at_ports_for_bus_port` and probe candidates in
        /// `at_interface_preference` order (falling back to discovery order),
        /// returning the first that answers OK. Used by the inline reopen-once
        /// path in `send_command` via `HandlerPortOpener`.
        fn reopen_port(&self) -> HardwareResult<serialport::TTYPort> {
            let Some(bus_port) = self.bus_port.as_deref() else {
                return Err(HardwareError::DeviceNotFound(
                    "Cannot reopen: handler has no bus_port".into(),
                ));
            };

            let candidates = at_ports_for_bus_port(bus_port);
            if candidates.is_empty() {
                return Err(HardwareError::DeviceNotFound(format!(
                    "Reopen: no ttyUSB ports for bus-port {bus_port}"
                )));
            }

            // Probe in interface-preference order first (stable across multi-modem
            // ttyUSB renumbering), then any remaining candidates as a fallback.
            let prefs = &self.profile.port_mapping.at_interface_preference;
            let mut ordered: Vec<String> = Vec::with_capacity(candidates.len());
            for &iface in prefs {
                if let Some(p) = candidates.iter().find(|p| {
                    let name = Path::new(p.as_str())
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("");
                    get_usb_interface_number(name) == Some(iface)
                }) {
                    if !ordered.contains(p) {
                        ordered.push(p.clone());
                    }
                }
            }
            for p in &candidates {
                if !ordered.contains(p) {
                    ordered.push(p.clone());
                }
            }

            let baud = self.profile.port_mapping.baud_rate;
            let mut last_err = HardwareError::DeviceNotFound(format!(
                "Reopen: no candidate port answered OK on bus-port {bus_port}"
            ));
            for path in ordered {
                match serialport::new(&path, baud)
                    .timeout(Duration::from_millis(100))
                    .open_native()
                {
                    Ok(mut fresh) => match Self::verify_fresh_port(&mut fresh) {
                        Ok(()) => {
                            info!("reopen_port: re-detected AT port at {path} (bus-port {bus_port})");
                            crate::state::debug_trace_with_source(
                                format!("reopen → recovered AT port at {path}"),
                                "reconnect",
                            );
                            // Record the live port so the state layer's 60s cache
                            // reconcile can refresh the reported device_path. Brief
                            // lock; never held across an await (this fn is sync).
                            // On lock poison we skip the update (best-effort cosmetic
                            // cell; the next successful reopen re-records it) rather
                            // than unwrap-panic the self-heal path.
                            if let Ok(mut cell) = self.live_device_path.lock() {
                                *cell = path.clone();
                            }
                            return Ok(fresh);
                        }
                        Err(e) => {
                            warn!("reopen_port: {path} opened but verify failed: {e}");
                            last_err = e;
                        }
                    },
                    Err(e) => {
                        warn!("reopen_port: failed to open {path}: {e}");
                        last_err = HardwareError::DeviceNotFound(format!("{path}: {e}"));
                    }
                }
            }
            Err(last_err)
        }

        /// Send an AT command and wait for response.
        ///
        /// Self-heals a dead serial fd once: if the write/read fails with an
        /// fd-dead `io::ErrorKind` (re-enumeration), `run_at_command_with_reopen`
        /// re-detects + reopens the port via `HandlerPortOpener`, swaps it into the
        /// held lock, and retries the exchange exactly once. Read-loop `TimedOut`
        /// stays a `continue` (never a reopen); persistent failure propagates.
        async fn send_command(&self, cmd: &str, cmd_timeout: Duration) -> HardwareResult<String> {
            let cmd_safe = super::redact_qicsgp(cmd);
            debug!("AT TX: {}", cmd_safe);
            crate::state::debug_trace(format!("AT TX → {}", cmd_safe));

            let mut port = self.port.lock().await;

            let mut opener = HandlerPortOpener { handler: self };
            let result =
                super::run_at_command_with_reopen(&mut *port, &mut opener, cmd, cmd_timeout);

            match &result {
                Ok(response) => {
                    let response_preview = if response.len() > 200 {
                        format!(
                            "{}... ({} bytes)",
                            super::truncate_on_char_boundary(response, 200),
                            response.len()
                        )
                    } else {
                        response.trim().to_string()
                    };
                    let preview_safe = super::redact_qicsgp(&response_preview);
                    debug!("AT RX: {}", preview_safe.replace('\n', " | "));
                    crate::state::debug_trace(format!("AT RX ← {}", preview_safe.replace('\n', " | ")));
                }
                Err(HardwareError::Timeout) => {
                    warn!("AT command timed out: {}", cmd_safe);
                    crate::state::debug_trace(format!(
                        "AT RX ✗ TIMEOUT ({}s): {}",
                        cmd_timeout.as_secs(),
                        cmd_safe
                    ));
                }
                Err(e) => {
                    warn!("AT command failed: {} ({})", cmd_safe, e);
                }
            }

            result
        }

        fn parse_csq(response: &str) -> Option<i32> {
            // +CSQ: <rssi>,<ber>
            let caps = CSQ_RE.captures(response)?;
            let rssi_raw: i32 = caps.get(1)?.as_str().parse().ok()?;
            if rssi_raw == 99 {
                Some(-999)
            } else {
                Some(-113 + (rssi_raw * 2))
            }
        }

        fn parse_cpin(response: &str) -> SimState {
            if response.contains("READY") {
                SimState::Ready
            } else if response.contains("SIM PIN") {
                SimState::PinRequired
            } else if response.contains("SIM PUK") {
                SimState::PukRequired
            } else if response.contains("ERROR") {
                SimState::NotInserted
            } else {
                SimState::Error
            }
        }

        fn parse_cops(response: &str) -> (Option<String>, Option<Technology>) {
            // +COPS: <mode>[,<format>,<oper>[,<act>]]
            if let Some(caps) = COPS_RE.captures(response) {
                let operator = caps.get(1).map(|m| m.as_str().to_string());
                let tech = caps.get(2).and_then(|m| {
                    match m.as_str().parse::<u32>().ok()? {
                        0 | 1 | 3 => Some(Technology::Gen2),
                        2 | 4 | 5 | 6 => Some(Technology::Gen3),
                        7 | 8 | 9 => Some(Technology::Gen4),
                        10 | 11 | 12 | 13 => Some(Technology::Gen5),
                        _ => None,
                    }
                });
                return (operator, tech);
            }
            (None, None)
        }

        fn parse_cereg(response: &str) -> RegistrationState {
            // +CEREG: <n>,<stat>
            if let Some(caps) = CEREG_RE.captures(response) {
                if let Some(stat) = caps.get(1).and_then(|m| m.as_str().parse::<u32>().ok()) {
                    return match stat {
                        0 => RegistrationState::NotRegistered,
                        1 => RegistrationState::Registered { home: true },
                        2 => RegistrationState::Searching,
                        3 => RegistrationState::Denied,
                        5 => RegistrationState::Registered { home: false },
                        _ => RegistrationState::Unknown,
                    };
                }
            }
            RegistrationState::Unknown
        }
        
        /// Check if network interface is up (for QMI-based connections).
        ///
        /// If `bus_port` is known, dynamically finds the correct network interface
        /// via sysfs. Falls back to checking hardcoded interface names when bus_port
        /// is unavailable.
        fn check_interface_status(&self) -> bool {
            // Try dynamic detection via bus_port first
            if let Some(ref bp) = self.bus_port {
                if let Some(iface) = find_net_device_for_bus_port(bp) {
                    let path = format!("/sys/class/net/{iface}/operstate");
                    if let Ok(state) = fs::read_to_string(&path) {
                        let state = state.trim();
                        debug!("Interface {} (bus-port {}) state: {}", iface, bp, state);
                        return state == "up";
                    }
                }
                debug!("No net device found for bus-port {}, falling back to hardcoded list", bp);
            }

            // Fallback: check common QMI/wwan interface names
            for iface in &["wwan0", "rmnet_data0", "usb0"] {
                let path = format!("/sys/class/net/{iface}/operstate");
                if let Ok(state) = fs::read_to_string(&path) {
                    let state = state.trim();
                    debug!("Interface {} state: {}", iface, state);
                    if state == "up" {
                        return true;
                    }
                }
            }
            false
        }

        /// Parse signal response using the profile's pre-compiled variant list.
        /// Tries each variant in order; first match wins.
        /// Returns None if no variant matched (caller falls through to CSQ).
        fn parse_signal_with_profile(&self, response: &str) -> Option<SignalInfo> {
            Self::parse_signal_response(response, &self.signal_variants)
        }

        /// Generic variant-matching signal parser (testable without serial port).
        ///
        /// Iterates through pre-compiled signal format variants in order.
        /// Each variant can require a substring check before trying its regex.
        /// First regex match wins and produces a SignalInfo.
        fn parse_signal_response(
            response: &str,
            signal_variants: &[(SignalFormatVariant, regex::Regex)],
        ) -> Option<SignalInfo> {
            for (variant, re) in signal_variants {
                // Skip if requires_substring is set and not found in response
                if !variant.requires_substring.is_empty()
                    && !response.contains(&variant.requires_substring)
                {
                    continue;
                }
                if let Some(caps) = re.captures(response) {
                    let technology = match variant.technology.as_str() {
                        "5G" => Some(Technology::Gen5),
                        _ => Some(Technology::Gen4),
                    };
                    return Some(SignalInfo {
                        rsrp: caps.name("rsrp").and_then(|m| m.as_str().parse::<f64>().ok()).unwrap_or(-999.0),
                        rsrq: caps.name("rsrq").and_then(|m| m.as_str().parse::<f64>().ok()).unwrap_or(-999.0),
                        rssi: caps.name("rssi").and_then(|m| m.as_str().parse::<f64>().ok()).unwrap_or(-999.0),
                        sinr: caps.name("sinr").and_then(|m| m.as_str().parse::<f64>().ok()).unwrap_or(0.0),
                        band: caps.name("band")
                            .map(|m| format!("{}{}", variant.band_prefix, m.as_str()))
                            .unwrap_or_default(),
                        cell_id: caps.name("cellid")
                            .map(|m| m.as_str().to_string())
                            .unwrap_or_default(),
                        technology,
                    });
                }
            }
            None // no variant matched — caller falls through to CSQ
        }

        /// Extract a numeric error code from a `+CME ERROR: <code>` response.
        /// Returns `None` if the response doesn't contain a CME ERROR with a numeric code.
        fn extract_cme_error_code(response: &str) -> Option<u32> {
            for line in response.lines() {
                let trimmed = line.trim();
                if let Some(rest) = trimmed.strip_prefix("+CME ERROR:") {
                    if let Ok(code) = rest.trim().parse::<u32>() {
                        return Some(code);
                    }
                }
            }
            None
        }

        /// Parse a GPS position response using a pre-compiled regex with named capture groups.
        /// Named groups: time, lat, lon, alt, speed, heading, fix, date, satellites.
        /// For NMEA format, also expects: ns (N/S), ew (E/W).
        /// Missing groups produce default values.
        ///
        /// When `coord_format` is `Nmea`, coordinates are converted from DDMM.MMMM to
        /// decimal degrees and sign is applied based on hemisphere (S → negative lat,
        /// W → negative lon).
        pub(crate) fn parse_gps_response(response: &str, re: &regex::Regex, coord_format: &GpsCoordinateFormat) -> GpsInfo {
            if let Some(caps) = re.captures(response) {
                let utc_time = caps.name("time").map(|m| m.as_str().trim()).unwrap_or("");
                let date_str = caps.name("date").map(|m| m.as_str().trim()).unwrap_or("");
                let fix_raw: u32 = caps.name("fix")
                    .and_then(|m| m.as_str().trim().parse().ok())
                    .unwrap_or(0);

                // Build ISO 8601 timestamp from UTC time (HHmmss.ss) and date (DDMMYY)
                let timestamp = if utc_time.len() >= 6 && date_str.len() >= 6 {
                    let hh = &utc_time[0..2];
                    let mm = &utc_time[2..4];
                    let ss = &utc_time[4..6];
                    let day = &date_str[0..2];
                    let mon = &date_str[2..4];
                    let yr = &date_str[4..6];
                    format!("20{yr}-{mon}-{day}T{hh}:{mm}:{ss}Z")
                } else {
                    String::new()
                };

                let raw_lat: f64 = caps.name("lat")
                    .and_then(|m| m.as_str().trim().parse().ok())
                    .unwrap_or(0.0);
                let raw_lon: f64 = caps.name("lon")
                    .and_then(|m| m.as_str().trim().parse().ok())
                    .unwrap_or(0.0);

                let (latitude, longitude) = match coord_format {
                    GpsCoordinateFormat::Nmea => {
                        let lat_dd = nmea_to_decimal(raw_lat);
                        let lon_dd = nmea_to_decimal(raw_lon);
                        // Apply hemisphere sign: S → negative, W → negative
                        let ns = caps.name("ns").map(|m| m.as_str()).unwrap_or("N");
                        let ew = caps.name("ew").map(|m| m.as_str()).unwrap_or("E");
                        let lat_signed = if ns == "S" { -lat_dd } else { lat_dd };
                        let lon_signed = if ew == "W" { -lon_dd } else { lon_dd };
                        (lat_signed, lon_signed)
                    }
                    GpsCoordinateFormat::Decimal => (raw_lat, raw_lon),
                };

                GpsInfo {
                    latitude,
                    longitude,
                    altitude: caps.name("alt")
                        .and_then(|m| m.as_str().trim().parse().ok()),
                    speed: caps.name("speed")
                        .and_then(|m| m.as_str().trim().parse().ok()),
                    fix_type: match fix_raw {
                        2 => "2D".to_string(),
                        3 => "3D".to_string(),
                        _ => "none".to_string(),
                    },
                    satellites: caps.name("satellites")
                        .and_then(|m| m.as_str().trim().parse().ok())
                        .unwrap_or(0),
                    timestamp,
                }
            } else {
                GpsInfo::default()
            }
        }

        /// Convert NMEA coordinate format (DDMM.MMMM) to decimal degrees.
        /// Formula: DD + (MM.MMMM / 60.0)
        #[allow(dead_code)]
        fn nmea_to_decimal(nmea: f64) -> f64 {
            super::nmea_to_decimal(nmea)
        }

        /// Handle interleaved RSRP/RSRQ antenna metrics (Telit AT#LAPS/AT#NRAPS).
        ///
        /// Sends a single command that returns both RSRP and RSRQ interleaved:
        /// `rsrp_rx0, rsrq_rx0, rsrp_rx1, rsrq_rx1[, ...]`
        /// SINR is not available from this command format.
        async fn get_antenna_metrics_interleaved(
            &self,
            config: &super::super::profiles::AntennaMetricsConfig,
        ) -> HardwareResult<AntennaMetrics> {
            let mut ports = Vec::new();

            if let Some(ref cmd) = config.rsrp_cmd {
                let prefix = super::response_prefix_from_cmd(cmd);
                crate::state::debug_trace_with_source(format!("→ {}", cmd), "antenna_live");
                match self.send_command(cmd, DEFAULT_TIMEOUT).await {
                    Ok(r) => {
                        crate::state::debug_trace_with_source(format!("← {}", r.trim()), "antenna_live");
                        let (rsrp, rsrq) = super::parse_antenna_interleaved(
                            &r, &prefix,
                            config.sentinel_value,
                            config.rsrp_min, config.rsrp_max,
                            config.rsrq_min, config.rsrq_max,
                        );
                        for i in 0..4 {
                            if rsrp[i].is_some() || rsrq[i].is_some() {
                                ports.push(AntennaPort {
                                    port: i as u32,
                                    rsrp: rsrp[i].unwrap_or(-999) as f64,
                                    rsrq: rsrq[i].unwrap_or(-999) as f64,
                                    sinr: 0.0, // SINR not available from interleaved format
                                    technology: None,
                                });
                            }
                        }
                    }
                    Err(e) => { warn!("{cmd} failed: {e}"); }
                }
            }

            Ok(AntennaMetrics { ports })
        }

    }

    #[async_trait]
    impl ModemHardware for AtHandler {
        async fn get_device_info(&self) -> HardwareResult<DeviceInfo> {
            let ati = self.send_command("ATI", DEFAULT_TIMEOUT).await?;
            let gsn = self.send_command("AT+GSN", DEFAULT_TIMEOUT).await?;

            // Parse manufacturer/model from ATI
            let lines: Vec<&str> = ati.lines()
                .map(|l| l.trim())
                .filter(|l| !l.starts_with("ATI") && !l.is_empty() && *l != "OK" && *l != "ERROR")
                .collect();
            let mut manufacturer = lines.first().map(|s| s.to_string()).unwrap_or_else(|| "Unknown".to_string());
            let mut model = lines.get(1).map(|s| s.to_string()).unwrap_or_else(|| "Unknown".to_string());

            // Generic fallback: if ATI didn't yield a proper manufacturer/model
            // (e.g. Telit returns just "332\r\n\r\nOK"), use standard 3GPP commands.
            // A valid manufacturer should contain at least one letter.
            let needs_fallback = !manufacturer.chars().any(|c| c.is_ascii_alphabetic())
                || !model.chars().any(|c| c.is_ascii_alphabetic())
                || model == "Unknown";
            if needs_fallback {
                if let Ok(cgmi_resp) = self.send_command("AT+CGMI", DEFAULT_TIMEOUT).await {
                    if let Some(mfr) = cgmi_resp.lines()
                        .map(|l| l.trim())
                        .find(|l| !l.is_empty() && *l != "OK" && !l.starts_with("AT+") && !l.starts_with("+CME"))
                    {
                        if mfr.chars().any(|c| c.is_ascii_alphabetic()) {
                            manufacturer = mfr.to_string();
                        }
                    }
                }
                if let Ok(cgmm_resp) = self.send_command("AT+CGMM", DEFAULT_TIMEOUT).await {
                    if let Some(mdl) = cgmm_resp.lines()
                        .map(|l| l.trim())
                        .find(|l| !l.is_empty() && *l != "OK" && !l.starts_with("AT+") && !l.starts_with("+CME"))
                    {
                        if mdl.chars().any(|c| c.is_ascii_alphanumeric()) {
                            model = mdl.to_string();
                        }
                    }
                }
            }

            // Try profile-driven firmware command, fall back to ATI Revision line
            let firmware_version = if let Some(ref cmd) = self.profile.firmware_config.firmware_cmd {
                if let Ok(response) = self.send_command(cmd, DEFAULT_TIMEOUT).await {
                    crate::state::debug_trace_with_source(format!("Firmware cmd '{}' response: {}", cmd, response.trim()), "system");
                    if let Some(ref re) = self.firmware_regex {
                        // Use profile regex: capture group 1 = version string
                        re.captures(&response)
                            .and_then(|caps| caps.get(1))
                            .map(|m| m.as_str().trim().to_string())
                            .filter(|s| !s.is_empty())
                    } else {
                        // No regex — extract first non-empty, non-AT, non-OK line
                        response.lines()
                            .find(|l| !l.is_empty() && !l.contains("OK") && !l.starts_with("AT"))
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                    }
                } else {
                    None
                }
            } else {
                None
            }.unwrap_or_else(|| {
                // Fallback: parse "Revision:" from ATI response
                lines.iter()
                    .find(|l| l.contains("Revision:"))
                    .and_then(|l| l.split(':').nth(1))
                    .map(|s| s.trim().to_string())
                    .unwrap_or_else(|| "Unknown".to_string())
            });

            let imei = gsn.lines()
                .find(|l| l.chars().all(|c| c.is_ascii_digit()))
                .unwrap_or("Unknown")
                .to_string();

            Ok(DeviceInfo {
                imei,
                manufacturer,
                model,
                firmware_version,
                supported_protocols: vec!["at".to_string()],
            })
        }

        async fn get_status(&self) -> HardwareResult<ModemStatus> {
            // First check if modem is in flight mode (radio off)
            let cfun = self.send_command("AT+CFUN?", DEFAULT_TIMEOUT).await?;
            let in_flight_mode = cfun.contains("+CFUN: 4") || cfun.contains("+CFUN: 0");
            
            if in_flight_mode {
                debug!("Modem is in flight mode - reporting disconnected");
                return Ok(ModemStatus {
                    connected: false,
                    technology: None,
                    operator: None,
                    signal_strength: 0,
                    ip_address: None,
                });
            }
            
            let csq = self.send_command("AT+CSQ", DEFAULT_TIMEOUT).await?;
            let cops = self.send_command("AT+COPS?", DEFAULT_TIMEOUT).await?;
            let cgact = self.send_command("AT+CGACT?", DEFAULT_TIMEOUT).await?;
            let cgpaddr = self.send_command("AT+CGPADDR=1", DEFAULT_TIMEOUT).await.ok();

            // Normalize signal to 0-100: RSSI range -113 (worst) to -51 (best)
            let rssi_dbm = Self::parse_csq(&csq).unwrap_or(0);
            let signal_strength = ((rssi_dbm + 113) * 100 / 62).clamp(0, 100);
            let (operator, technology) = Self::parse_cops(&cops);
            
            // Check PDP context status
            let pdp_active = cgact.contains("+CGACT: 1,1");
            
            // Also check if network interface is up (more reliable for QMI)
            let interface_up = self.check_interface_status();
            
            // Consider connected if either PDP is active OR interface is up
            let connected = pdp_active || interface_up;
            
            debug!("Connection status: pdp_active={}, interface_up={}, connected={}", 
                   pdp_active, interface_up, connected);

            let ip_address = cgpaddr.and_then(|r| {
                CGPADDR_RE.captures(&r)?.get(1).map(|m| normalize_cgpaddr_ip(m.as_str()))
            });

            Ok(ModemStatus {
                connected,
                technology,
                operator,
                signal_strength,
                ip_address,
            })
        }

        async fn get_signal(&self) -> HardwareResult<SignalInfo> {
            // Try vendor-specific signal command from profile
            if let Some(ref cmd) = self.profile.commands.signal_cmd {
                match self.send_command(cmd, DEFAULT_TIMEOUT).await {
                    Ok(response) => {
                        if let Some(parsed) = self.parse_signal_with_profile(&response) {
                            return Ok(parsed);
                        }
                        warn!("Signal parse failed, falling back to CSQ. Response: {:?}",
                            super::truncate_on_char_boundary(&response, 300));
                    }
                    Err(e) => {
                        warn!("Vendor signal command failed ({}), falling back to CSQ", e);
                    }
                }
            }

            // Fall back to generic signal command (AT+CSQ)
            let csq = self.send_command(&self.profile.commands.generic_signal_cmd, DEFAULT_TIMEOUT).await?;
            let rssi = Self::parse_csq(&csq).unwrap_or(-999) as f64;

            Ok(SignalInfo {
                rssi,
                rsrp: -999.0,
                rsrq: -999.0,
                sinr: 0.0,
                band: String::new(),
                cell_id: String::new(),
                technology: None,
            })
        }

        async fn get_data_stats(&self) -> HardwareResult<DataStats> {
            // AT commands don't provide data stats directly
            Ok(DataStats {
                bytes_tx: 0,
                bytes_rx: 0,
                session_uptime_secs: 0,
            })
        }

        async fn connect(&self, config: &ConnectionConfig) -> HardwareResult<()> {
            use crate::state::debug_trace;
            info!("Connecting with APN: {}", config.apn);

            let pdp_type = match config.ip_type {
                IpType::Ipv4 => "IP",
                IpType::Ipv6 => "IPV6",
                IpType::Ipv4v6 => "IPV4V6",
            };
            let cid = config.cid;

            // In ECM mode the modem manages the data bearer internally.
            // AT+CGACT cannot activate/deactivate the ECM connection.
            // The only reliable way to (re)connect with a new APN is a
            // full CFUN cycle: radio OFF → set APN → radio ON.

            // Step 1: Turn radio OFF to tear down current ECM bearer
            debug_trace("[CONNECT] Step 1: Radio OFF (AT+CFUN=0)");
            let response = self.send_command("AT+CFUN=0", DEFAULT_TIMEOUT).await?;
            if response.contains("ERROR") {
                debug_trace(format!("[CONNECT] CFUN=0 failed: {}", response.trim().replace('\n', " | ")));
                return Err(HardwareError::Protocol("Failed to turn radio off".to_string()));
            }

            // Let radio fully quiesce
            tokio::time::sleep(Duration::from_secs(1)).await;

            // Step 2: Set PDP context with APN while radio is off.
            // Defense-in-depth (same root cause as the serial-write guard): the APN
            // is operator-supplied and gets interpolated into a quoted AT argument.
            // Reject a `"` (breaks out of the quoted arg) or any control char (CR/LF
            // smuggles a second command) BEFORE building the command. If we rejected
            // here after turning the radio off, leave it off — the caller decides
            // recovery; a malformed APN is a hard configuration error, not a transient.
            super::validate_at_quoted_arg(&config.apn)?;
            debug_trace(format!("[CONNECT] Step 2: AT+CGDCONT={cid},\"{pdp_type}\",\"{}\"", config.apn));
            let cmd = format!("AT+CGDCONT={cid},\"{pdp_type}\",\"{}\"", config.apn);
            let response = self.send_command(&cmd, DEFAULT_TIMEOUT).await?;
            if response.contains("ERROR") {
                error!("Failed to set PDP context: {}", response);
                debug_trace("[CONNECT] CGDCONT failed, turning radio back on");
                let _ = self.send_command("AT+CFUN=1", DEFAULT_TIMEOUT).await;
                return Err(HardwareError::Protocol("Failed to set PDP context".to_string()));
            }

            // Step 3: Turn radio ON — ECM will auto-connect with the new APN
            debug_trace("[CONNECT] Step 3: Radio ON (AT+CFUN=1)");
            let response = self.send_command("AT+CFUN=1", DEFAULT_TIMEOUT).await?;
            if response.contains("ERROR") {
                debug_trace(format!("[CONNECT] CFUN=1 failed: {}", response.trim().replace('\n', " | ")));
                return Err(HardwareError::Protocol("Failed to turn radio on".to_string()));
            }

            // Wait for modem to re-register and ECM to establish the bearer.
            // Typically takes 3-5s on AT&T.
            debug_trace("[CONNECT] Waiting 5s for network registration + ECM...");
            tokio::time::sleep(Duration::from_secs(5)).await;

            debug_trace("[CONNECT] Connect sequence complete");
            info!("CFUN cycle complete, ECM should auto-connect");
            *self.connect_time.lock().unwrap() = Some(std::time::Instant::now());
            Ok(())
        }

        async fn disconnect(&self) -> HardwareResult<()> {
            info!("Disconnect requested - putting modem in flight mode");
            
            // For QMI-managed connections, AT+CGACT can't control the data session.
            // AT+CFUN=4 puts the modem in "flight mode" (radio off), which disconnects everything.
            // The connection will stay down until AT+CFUN=1 is called (via connect).
            let response = self.send_command("AT+CFUN=4", DEFAULT_TIMEOUT).await?;
            info!("AT+CFUN=4 response: {}", response.trim().replace('\n', " | "));
            
            if response.contains("ERROR") {
                error!("Failed to enter flight mode: {}", response);
                return Err(HardwareError::Protocol("Failed to disconnect (flight mode failed)".to_string()));
            }
            
            // Brief delay to let radio state change
            tokio::time::sleep(Duration::from_millis(300)).await;
            
            // Verify interface is down
            let interface_up = self.check_interface_status();
            info!("Interface status after flight mode: {}", if interface_up { "UP (unexpected)" } else { "DOWN (good)" });
            
            info!("Modem in flight mode - disconnected");
            *self.connect_time.lock().unwrap() = None;
            Ok(())
        }

        async fn reconnect(&self) -> HardwareResult<()> {
            use crate::state::debug_trace_with_source;
            info!("Reconnect requested - pure CFUN cycle (saved APN, no CGDCONT)");

            // In ECM mode the modem manages the data bearer internally and
            // AT+CGACT cannot (re)activate it. A pure radio cycle — radio OFF
            // then radio ON — re-establishes the bearer using whatever APN is
            // already saved on the modem. Unlike connect(), we deliberately do
            // NOT write a PDP context (AT+CGDCONT) here.

            // Step 1: Turn radio OFF to tear down the current ECM bearer
            debug_trace_with_source("[RECONNECT] Step 1: Radio OFF (AT+CFUN=0)", "reconnect");
            let response = self.send_command("AT+CFUN=0", DEFAULT_TIMEOUT).await?;
            if response.contains("ERROR") {
                debug_trace_with_source(format!("[RECONNECT] CFUN=0 failed: {}", response.trim().replace('\n', " | ")), "reconnect");
                return Err(HardwareError::Protocol("Failed to turn radio off".to_string()));
            }

            // Let radio fully quiesce
            tokio::time::sleep(Duration::from_secs(1)).await;

            // Step 2: Turn radio ON — ECM will auto-connect with the saved APN
            debug_trace_with_source("[RECONNECT] Step 2: Radio ON (AT+CFUN=1)", "reconnect");
            let response = self.send_command("AT+CFUN=1", DEFAULT_TIMEOUT).await?;
            if response.contains("ERROR") {
                debug_trace_with_source(format!("[RECONNECT] CFUN=1 failed: {}", response.trim().replace('\n', " | ")), "reconnect");
                return Err(HardwareError::Protocol("Failed to turn radio on".to_string()));
            }

            // Wait for modem to re-register and ECM to establish the bearer.
            // Typically takes 3-5s on AT&T.
            debug_trace_with_source("[RECONNECT] Waiting 5s for network registration + ECM...", "reconnect");
            tokio::time::sleep(Duration::from_secs(5)).await;

            debug_trace_with_source("[RECONNECT] Reconnect sequence complete", "reconnect");
            info!("CFUN cycle complete, ECM should auto-connect with saved APN");
            *self.connect_time.lock().unwrap() = Some(std::time::Instant::now());
            Ok(())
        }

        async fn get_sim_status(&self) -> HardwareResult<SimStatus> {
            let cpin = self.send_command("AT+CPIN?", DEFAULT_TIMEOUT).await?;
            let state = Self::parse_cpin(&cpin);

            let present = state != SimState::NotInserted;

            let iccid = if state == SimState::Ready {
                self.send_command(&self.profile.commands.iccid_cmd, DEFAULT_TIMEOUT).await.ok()
                    .and_then(|r| {
                        // Try profile regex first
                        if let Some(ref re) = self.iccid_regex {
                            if let Some(caps) = re.captures(&r) {
                                return caps.get(1).map(|m| m.as_str().to_string());
                            }
                        }
                        // Fallback: line with only digits and F (some modems return raw ICCID)
                        r.lines()
                            .find(|l| !l.is_empty() && l.chars().all(|c| c.is_ascii_digit() || c == 'F' || c == 'f'))
                            .map(|s| s.to_string())
                    })
            } else {
                None
            };

            let imsi = if state == SimState::Ready {
                self.send_command("AT+CIMI", DEFAULT_TIMEOUT).await.ok()
                    .and_then(|r| r.lines()
                        .find(|l| !l.is_empty() && l.chars().all(|c| c.is_ascii_digit()))
                        .map(|s| s.to_string()))
            } else {
                None
            };

            // Try vendor-specific operator name command from profile
            let operator_name = if state == SimState::Ready {
                let mut name: Option<String> = None;

                if let Some(ref cmd) = self.profile.commands.operator_name_cmd {
                    if let Ok(ref r) = self.send_command(cmd, DEFAULT_TIMEOUT).await {
                        if let Some(ref re) = self.operator_regex {
                            if let Some(caps) = re.captures(r) {
                                name = caps.get(1).map(|m| m.as_str().to_string())
                                    .filter(|s| !s.is_empty());
                            }
                        }
                    }
                }

                // Fallback to standard operator name command
                if name.is_none() {
                    name = self.send_command(&self.profile.commands.generic_operator_cmd, DEFAULT_TIMEOUT).await.ok()
                        .and_then(|r| {
                            let re = regex::Regex::new(&self.profile.commands.generic_operator_regex).ok()?;
                            re.captures(&r)?.get(1).map(|m| m.as_str().to_string())
                        })
                        .filter(|s| !s.is_empty());
                }

                name
            } else {
                None
            };

            Ok(SimStatus {
                present,
                state,
                iccid,
                imsi,
                operator_name,
            })
        }

        async fn verify_pin(&self, pin: &str) -> HardwareResult<()> {
            let cmd = format!("AT+CPIN={pin}");
            let response = self.send_command(&cmd, DEFAULT_TIMEOUT).await?;

            if response.contains("ERROR") {
                Err(HardwareError::SimError("Invalid PIN".to_string()))
            } else {
                Ok(())
            }
        }

        async fn change_pin(&self, old_pin: &str, new_pin: &str) -> HardwareResult<()> {
            let cmd = format!("AT+CPWD=\"SC\",\"{old_pin}\",\"{new_pin}\"");
            let response = self.send_command(&cmd, DEFAULT_TIMEOUT).await?;

            if response.contains("ERROR") {
                Err(HardwareError::SimError("PIN change failed".to_string()))
            } else {
                Ok(())
            }
        }

        async fn enable_pin(&self, pin: &str) -> HardwareResult<()> {
            let cmd = format!("AT+CLCK=\"SC\",1,\"{pin}\"");
            let response = self.send_command(&cmd, DEFAULT_TIMEOUT).await?;

            if response.contains("ERROR") {
                Err(HardwareError::SimError("Enable PIN failed".to_string()))
            } else {
                Ok(())
            }
        }

        async fn disable_pin(&self, pin: &str) -> HardwareResult<()> {
            let cmd = format!("AT+CLCK=\"SC\",0,\"{pin}\"");
            let response = self.send_command(&cmd, DEFAULT_TIMEOUT).await?;

            if response.contains("ERROR") {
                Err(HardwareError::SimError("Disable PIN failed".to_string()))
            } else {
                Ok(())
            }
        }

        async fn get_registration(&self) -> HardwareResult<RegistrationState> {
            let cereg = self.send_command(&self.profile.commands.registration_cmd, DEFAULT_TIMEOUT).await?;
            Ok(Self::parse_cereg(&cereg))
        }

        async fn scan_networks(&self) -> HardwareResult<Vec<AvailableNetwork>> {
            let response = self.send_command("AT+COPS=?", LONG_TIMEOUT).await?;

            let mut networks = Vec::new();
            let re = regex::Regex::new(r#"\((\d+),"([^"]*)","([^"]*)","(\d+)"(?:,(\d+))?\)"#).ok();

            if let Some(re) = re {
                for cap in re.captures_iter(&response) {
                    let status = cap.get(1).and_then(|m| m.as_str().parse::<u32>().ok());
                    let long_name = cap.get(2).map(|m| m.as_str().to_string());
                    let numeric = cap.get(4).map(|m| m.as_str().to_string());
                    let act = cap.get(5).and_then(|m| m.as_str().parse::<u32>().ok());

                    if let (Some(status), Some(name), Some(code)) = (status, long_name, numeric) {
                        let network_status = match status {
                            1 => NetworkStatus::Available,
                            2 => NetworkStatus::Current,
                            3 => NetworkStatus::Forbidden,
                            _ => continue,
                        };

                        let technology = match act {
                            Some(0) => Technology::Gen2,
                            Some(2) => Technology::Gen3,
                            Some(7) => Technology::Gen4,
                            Some(12) => Technology::Gen5,
                            _ => Technology::Gen4,
                        };

                        networks.push(AvailableNetwork {
                            operator_name: name,
                            operator_code: code,
                            technology,
                            status: network_status,
                        });
                    }
                }
            }

            Ok(networks)
        }

        async fn select_network(&self, operator_code: Option<&str>) -> HardwareResult<()> {
            let cmd = if let Some(code) = operator_code {
                format!("AT+COPS=1,2,\"{code}\"")
            } else {
                "AT+COPS=0".to_string()
            };

            let response = self.send_command(&cmd, LONG_TIMEOUT).await?;

            if response.contains("ERROR") {
                Err(HardwareError::Protocol("Network selection failed".to_string()))
            } else {
                Ok(())
            }
        }

        async fn execute_at(&self, command: &str) -> HardwareResult<String> {
            self.send_command(command, DEFAULT_TIMEOUT).await
        }

        async fn get_gps_position(&self) -> HardwareResult<GpsInfo> {
            let gps = &self.profile.gps_config;
            if !gps.supported {
                return Err(HardwareError::Internal("GPS not supported by this modem".to_string()));
            }

            // Start GPS engine if profile defines a start command
            if let Some(ref start_cmd) = gps.start_cmd {
                crate::state::debug_trace_with_source(format!("GPS start: {}", start_cmd), "system");
                let start_resp = self.send_command(start_cmd, DEFAULT_TIMEOUT).await?;
                if start_resp.contains("ERROR") {
                    let code = Self::extract_cme_error_code(&start_resp);
                    if let Some(c) = code {
                        if !gps.start_already_running_codes.contains(&c) {
                            return Err(HardwareError::Protocol(format!("Failed to enable GPS: {}", start_resp.trim())));
                        }
                        // Error code means "already running" — not a real error
                    } else if gps.start_tolerates_bare_error {
                        // Bare ERROR with no CME code — profile says treat as "already running"
                        crate::state::debug_trace_with_source(
                            format!("GPS start bare ERROR tolerated (assumed already running): {}", start_resp.trim()),
                            "system",
                        );
                    } else {
                        // Generic ERROR with no numeric code
                        return Err(HardwareError::Protocol(format!("Failed to enable GPS: {}", start_resp.trim())));
                    }
                }
            }

            // Query GPS position
            let query_cmd = gps.query_cmd.as_deref().ok_or_else(|| {
                HardwareError::Internal("GPS profile has no query_cmd configured".to_string())
            })?;
            crate::state::debug_trace_with_source(format!("GPS query: {}", query_cmd), "system");
            let response = self.send_command(query_cmd, DEFAULT_TIMEOUT).await?;

            // Check for "no fix" error codes
            if response.contains("ERROR") {
                let code = Self::extract_cme_error_code(&response);
                if let Some(c) = code {
                    if gps.no_fix_error_codes.contains(&c) {
                        return Ok(GpsInfo::default());
                    }
                }
                // Other error — return default rather than failing
                return Ok(GpsInfo::default());
            }

            // Parse using profile regex
            if let Some(ref re) = self.gps_regex {
                Ok(Self::parse_gps_response(&response, re, &gps.coordinate_format))
            } else {
                Ok(GpsInfo::default())
            }
        }

        async fn stop_gps(&self) -> HardwareResult<()> {
            let gps = &self.profile.gps_config;
            if !gps.supported {
                return Err(HardwareError::Internal("GPS not supported by this modem".to_string()));
            }

            // If no stop command, GPS engine is always-on — no-op
            let stop_cmd = match gps.stop_cmd.as_deref() {
                Some(cmd) => cmd,
                None => return Ok(()),
            };

            crate::state::debug_trace_with_source(format!("GPS stop: {}", stop_cmd), "system");
            let response = self.send_command(stop_cmd, DEFAULT_TIMEOUT).await?;
            if response.contains("ERROR") {
                let code = Self::extract_cme_error_code(&response);
                if let Some(c) = code {
                    if !gps.stop_already_stopped_codes.contains(&c) {
                        return Err(HardwareError::Protocol(format!("Failed to stop GPS: {}", response.trim())));
                    }
                    // Error code means "already stopped" — not a real error
                } else {
                    // Generic ERROR with no numeric code
                    return Err(HardwareError::Protocol(format!("Failed to stop GPS: {}", response.trim())));
                }
            }

            Ok(())
        }

        async fn get_extended_signal(&self) -> HardwareResult<ExtendedSignalInfo> {
            let primary = self.get_signal().await.unwrap_or_default();

            // Skip CA query if not supported by this profile
            if !self.profile.ca_config.supported {
                return Ok(ExtendedSignalInfo {
                    primary,
                    secondary_cells: vec![],
                    carrier_aggregation: false,
                    network_type: String::new(),
                });
            }

            // CA info command from profile — parse both PCC and SCC
            let mut secondary_cells = vec![];
            if let Some(ref cmd) = self.profile.ca_config.ca_info_cmd {
                if let Ok(response) = self.send_command(cmd, DEFAULT_TIMEOUT).await {
                    if self.profile.ca_config.ca_parser_variant == "telit_cainfo" {
                        // Telit AT#CAINFO? format: band_class-encoded CSV
                        let (_pcc, sccs) = parse_telit_cainfo(&response);
                        secondary_cells = sccs;
                    } else {
                        // Quectel AT+QCAINFO format: regex-based parsing (default)
                        // PCC comes from get_signal() (same source as Signal Info panel).
                        // QCAINFO PCC is not used for primary — avoids value mismatch between
                        // Signal Info and CA sections due to different AT command timing.
                        // parse_qcainfo_pcc() is still available for future use if needed.
                        secondary_cells = parse_qcainfo_secondary(
                            &response,
                            self.ca_lte_regex.as_ref(),
                            self.ca_nr5g_regex.as_ref(),
                            &self.profile.ca_config.band_prefix_mappings,
                        );
                    }
                }
            }

            let carrier_aggregation = !secondary_cells.is_empty();

            // Network type command from profile
            let network_type = if let Some(ref cmd) = self.profile.ca_config.network_type_cmd {
                if let Ok(response) = self.send_command(cmd, DEFAULT_TIMEOUT).await {
                    parse_qnwinfo_type(&response, self.ca_nwinfo_regex.as_ref())
                        .unwrap_or_default()
                } else {
                    String::new()
                }
            } else {
                String::new()
            };

            Ok(ExtendedSignalInfo {
                primary,
                secondary_cells,
                carrier_aggregation,
                network_type,
            })
        }

        async fn get_antenna_metrics(&self) -> HardwareResult<AntennaMetrics> {
            let config = &self.profile.antenna_metrics_config;
            if !config.supported {
                return Ok(AntennaMetrics::default());
            }

            // Interleaved mode (Telit AT#LAPS/AT#NRAPS): RSRP and RSRQ in one response
            if config.interleaved_rsrp_rsrq {
                return self.get_antenna_metrics_interleaved(config).await;
            }

            // Standard mode (Quectel): separate commands for RSRP, SINR, RSRQ

            // Fetch RSRP
            let rsrp_rows = if let Some(ref cmd) = config.rsrp_cmd {
                let prefix = super::response_prefix_from_cmd(cmd);
                crate::state::debug_trace_with_source(format!("→ {}", cmd), "antenna_live");
                match self.send_command(cmd, DEFAULT_TIMEOUT).await {
                    Ok(r) => {
                        crate::state::debug_trace_with_source(format!("← {}", r.trim()), "antenna_live");
                        super::parse_antenna_metric_multi(&r, &prefix, config.sentinel_value, config.rsrp_min, config.rsrp_max)
                    }
                    Err(e) => { warn!("{cmd} failed: {e}"); vec![] }
                }
            } else {
                vec![]
            };

            // Fetch SINR
            let sinr_rows = if let Some(ref cmd) = config.sinr_cmd {
                let prefix = super::response_prefix_from_cmd(cmd);
                crate::state::debug_trace_with_source(format!("→ {}", cmd), "antenna_live");
                match self.send_command(cmd, DEFAULT_TIMEOUT).await {
                    Ok(r) => {
                        crate::state::debug_trace_with_source(format!("← {}", r.trim()), "antenna_live");
                        super::parse_antenna_metric_multi(&r, &prefix, config.sentinel_value, config.sinr_min, config.sinr_max)
                    }
                    Err(e) => { warn!("{cmd} failed: {e}"); vec![] }
                }
            } else {
                vec![]
            };

            // Fetch RSRQ
            let rsrq_rows = if let Some(ref cmd) = config.rsrq_cmd {
                let prefix = super::response_prefix_from_cmd(cmd);
                crate::state::debug_trace_with_source(format!("→ {}", cmd), "antenna_live");
                match self.send_command(cmd, DEFAULT_TIMEOUT).await {
                    Ok(r) => {
                        crate::state::debug_trace_with_source(format!("← {}", r.trim()), "antenna_live");
                        super::parse_antenna_metric_multi(&r, &prefix, config.sentinel_value, config.rsrq_min, config.rsrq_max)
                    }
                    Err(e) => { warn!("{cmd} failed: {e}"); vec![] }
                }
            } else {
                vec![]
            };

            // Build per-port metrics, resetting port numbering per technology
            let mut ports = Vec::new();

            // Collect all technology names seen across the three commands
            let mut techs: Vec<String> = Vec::new();
            for (tech, _) in rsrp_rows.iter().chain(sinr_rows.iter()).chain(rsrq_rows.iter()) {
                if !tech.is_empty() && !techs.contains(tech) {
                    techs.push(tech.clone());
                }
            }

            if techs.is_empty() {
                // Single-row legacy: no technology tag
                let rsrp = rsrp_rows.first().map(|(_, v)| *v).unwrap_or([None; 4]);
                let sinr = sinr_rows.first().map(|(_, v)| *v).unwrap_or([None; 4]);
                let rsrq = rsrq_rows.first().map(|(_, v)| *v).unwrap_or([None; 4]);
                let mut port_idx: u32 = 0;
                for i in 0..4 {
                    if rsrp[i].is_some() || rsrq[i].is_some() || sinr[i].is_some() {
                        ports.push(AntennaPort {
                            port: port_idx,
                            rsrp: rsrp[i].unwrap_or(-999) as f64,
                            rsrq: rsrq[i].unwrap_or(-999) as f64,
                            sinr: sinr[i].unwrap_or(0) as f64,
                            technology: None,
                        });
                    }
                    port_idx += 1;
                }
            } else {
                for tech in &techs {
                    let rsrp = rsrp_rows.iter().find(|(t, _)| t == tech).map(|(_, v)| *v).unwrap_or([None; 4]);
                    let sinr = sinr_rows.iter().find(|(t, _)| t == tech).map(|(_, v)| *v).unwrap_or([None; 4]);
                    let rsrq = rsrq_rows.iter().find(|(t, _)| t == tech).map(|(_, v)| *v).unwrap_or([None; 4]);
                    let mut port_idx: u32 = 0;
                    for i in 0..4 {
                        if rsrp[i].is_some() || rsrq[i].is_some() || sinr[i].is_some() {
                            ports.push(AntennaPort {
                                port: port_idx,
                                rsrp: rsrp[i].unwrap_or(-999) as f64,
                                rsrq: rsrq[i].unwrap_or(-999) as f64,
                                sinr: sinr[i].unwrap_or(0) as f64,
                                technology: Some(tech.clone()),
                            });
                        }
                        port_idx += 1;
                    }
                }
            }

            Ok(AntennaMetrics { ports })
        }

        async fn get_connection_status(&self) -> HardwareResult<ConnectionStatus> {
            // Check flight mode first
            let cfun = self.send_command("AT+CFUN?", DEFAULT_TIMEOUT).await?;
            let in_flight_mode = cfun.contains("+CFUN: 4") || cfun.contains("+CFUN: 0");

            if in_flight_mode {
                return Ok(ConnectionStatus {
                    connected: false,
                    technology: None,
                    operator: None,
                    ip_address: None,
                });
            }

            // No AT+CSQ — signal_strength is derived from SignalInfo by the cache task
            let cops = self.send_command("AT+COPS?", DEFAULT_TIMEOUT).await?;
            let cgact = self.send_command("AT+CGACT?", DEFAULT_TIMEOUT).await?;
            let cgpaddr = self.send_command("AT+CGPADDR=1", DEFAULT_TIMEOUT).await.ok();

            let (operator, technology) = Self::parse_cops(&cops);

            let pdp_active = cgact.contains("+CGACT: 1,1");
            let interface_up = self.check_interface_status();
            let connected = pdp_active || interface_up;

            let ip_address = cgpaddr.and_then(|r| {
                CGPADDR_RE.captures(&r)?.get(1).map(|m| normalize_cgpaddr_ip(m.as_str()))
            });

            Ok(ConnectionStatus {
                connected,
                technology,
                operator,
                ip_address,
            })
        }

        fn live_device_path_handle(&self) -> Option<std::sync::Arc<std::sync::Mutex<String>>> {
            Some(std::sync::Arc::clone(&self.live_device_path))
        }
    }

    // Log at INFO when Verbose, debug! when Quiet — gates detect_modems_impl /
    // find_at_port narration so the periodic 30s rescan stays out of logread.
    macro_rules! narrate {
        ($verbosity:expr, $($arg:tt)*) => {
            match $verbosity {
                DetectionVerbosity::Verbose => tracing::info!($($arg)*),
                DetectionVerbosity::Quiet => tracing::debug!($($arg)*),
            }
        };
    }

    /// Detect QMI devices and find associated AT ports.
    ///
    /// Uses the profile registry to identify modems by USB vendor/product ID
    /// and to determine port preferences for each modem model.
    pub fn detect_modems_impl(registry: &super::super::profiles::ProfileRegistry, verbosity: DetectionVerbosity) -> Vec<DetectedModem> {
        let mut modems = Vec::new();
        let generic = registry.generic();

        // Track USB bus-ports already discovered via QMI to avoid duplicates
        let mut discovered_bus_ports: HashSet<String> = HashSet::new();

        narrate!(verbosity, "Starting modem detection...");

        // ── Pass 1: QMI devices (/dev/cdc-wdm*) ──
        match glob("/dev/cdc-wdm*") {
            Ok(entries) => {
                for entry in entries.flatten() {
                    let qmi_path = entry.to_string_lossy().to_string();
                    debug!("Found QMI device candidate: {}", qmi_path);

                    let device_name = Path::new(&qmi_path).file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("");

                    let driver_link = format!("/sys/class/usbmisc/{device_name}/device/driver");
                    debug!("Checking driver link: {}", driver_link);

                    let driver = fs::read_link(&driver_link).ok()
                        .and_then(|p| p.file_name().and_then(|n| n.to_str()).map(|s| s.to_string()));

                    debug!("Driver detected: {:?}", driver);

                    if driver.as_deref() == Some("qmi_wwan") {
                        // Get USB identity and match to profile
                        let identity = get_modem_identity(&qmi_path);
                        let (vid, pid) = identity.clone().unwrap_or_default();

                        let profile = if !vid.is_empty() {
                            registry.match_profile(&vid, &pid)
                        } else {
                            generic
                        };

                        let qmi_bus_port = get_usb_bus_port(device_name);
                        debug!("QMI device {} identity: {}:{}, bus-port: {:?}",
                               device_name, vid, pid, qmi_bus_port);

                        // Find associated AT port using profile port preferences
                        if let Some(at_port) = find_at_port(&qmi_path, &profile.port_mapping.at_port_preference, &profile.port_mapping.at_interface_preference, verbosity) {
                            let description = if !profile.is_generic() {
                                format!("{} {}", profile.identity.manufacturer, profile.identity.model)
                            } else if !vid.is_empty() {
                                format!("Unknown Modem ({vid}:{pid})")
                            } else {
                                "Unknown Modem".to_string()
                            };

                            narrate!(verbosity, "Found modem via QMI: {} at {} (profile: {})", description, at_port, profile.profile_id());
                            modems.push(DetectedModem {
                                device_path: at_port,
                                protocol: ModemProtocol::At,
                                description,
                                vendor_id: if vid.is_empty() { None } else { Some(vid) },
                                product_id: if pid.is_empty() { None } else { Some(pid) },
                                profile_id: Some(profile.profile_id()),
                                has_profile: !profile.is_generic(),
                                bus_port: qmi_bus_port.clone(),
                                all_ports: Vec::new(),
                            });

                            // Only track bus-port AFTER successfully adding the modem.
                            // If find_at_port fails, we don't track it, so the ttyUSB
                            // scan can still discover this modem's ports.
                            if let Some(bp) = qmi_bus_port {
                                debug!("Tracking bus-port {} (discovered via QMI)", bp);
                                discovered_bus_ports.insert(bp);
                            }
                        } else {
                            warn!("QMI device {} found but no AT port located (bus-port {:?} NOT tracked, ttyUSB scan will retry)",
                                  qmi_path, qmi_bus_port);
                        }
                    }
                }
            }
            Err(e) => {
                warn!("Failed to glob /dev/cdc-wdm*: {}", e);
            }
        }

        // ── Pass 2: ttyUSB devices (always runs, finds modems not detected via QMI) ──
        narrate!(verbosity, "Scanning ttyUSB devices for additional modems...");

        match glob("/dev/ttyUSB*") {
            Ok(entries) => {
                // Group all ttyUSB ports by their parent USB bus-port
                let mut bus_port_groups: HashMap<String, Vec<String>> = HashMap::new();
                let mut ungrouped: Vec<String> = Vec::new();

                for entry in entries.flatten() {
                    let path = entry.to_string_lossy().to_string();
                    let device_name = Path::new(&path)
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("");

                    if let Some(bus_port) = get_usb_bus_port(device_name) {
                        bus_port_groups.entry(bus_port).or_default().push(path);
                    } else {
                        debug!("Could not determine bus-port for {}", path);
                        ungrouped.push(path);
                    }
                }

                narrate!(verbosity, "ttyUSB scan: {} bus-port group(s), {} ungrouped port(s)",
                      bus_port_groups.len(), ungrouped.len());
                for (bp, ports) in &bus_port_groups {
                    narrate!(verbosity, "  bus-port {}: {} port(s) {:?}", bp, ports.len(), ports);
                }

                // Process each bus-port group as a potential modem
                let mut sorted_groups: Vec<_> = bus_port_groups.into_iter().collect();
                sorted_groups.sort_by(|a, b| a.0.cmp(&b.0));

                for (bus_port, mut ports) in sorted_groups {
                    // Skip if this bus-port was already discovered via QMI
                    if discovered_bus_ports.contains(&bus_port) {
                        narrate!(verbosity, "  bus-port {} already discovered via QMI, skipping {} port(s)", bus_port, ports.len());
                        continue;
                    }

                    ports.sort();

                    // Get identity from any port in this group
                    let (vid, pid) = ports.iter()
                        .find_map(|p| get_modem_identity(p))
                        .unwrap_or_default();

                    let profile = if !vid.is_empty() {
                        registry.match_profile(&vid, &pid)
                    } else {
                        generic
                    };

                    // Pick best AT port:
                    // 1. Try name-based preference (works for single-modem setups)
                    // 2. Try USB interface-based preference (works for multi-modem setups)
                    // 3. Fall back to first port in the group
                    let at_port = profile.port_mapping.at_port_preference.iter()
                        .find_map(|pref| {
                            ports.iter().find(|p| p.ends_with(pref.as_str())).cloned()
                        })
                        .or_else(|| {
                            // Interface-based: match by USB interface number
                            profile.port_mapping.at_interface_preference.iter()
                                .find_map(|&iface| {
                                    ports.iter().find(|p| {
                                        let name = Path::new(p.as_str())
                                            .file_name()
                                            .and_then(|n| n.to_str())
                                            .unwrap_or("");
                                        get_usb_interface_number(name) == Some(iface)
                                    }).cloned()
                                })
                        })
                        .or_else(|| ports.first().cloned());

                    if let Some(at_port) = at_port {
                        let description = if !profile.is_generic() {
                            format!("{} {}", profile.identity.manufacturer, profile.identity.model)
                        } else if !vid.is_empty() {
                            format!("Unknown Modem ({vid}:{pid})")
                        } else {
                            format!("Modem (bus {bus_port})")
                        };

                        narrate!(verbosity, "Found modem via ttyUSB: {} at {} (profile: {}, bus-port: {})",
                              description, at_port, profile.profile_id(), bus_port);

                        modems.push(DetectedModem {
                            device_path: at_port,
                            protocol: ModemProtocol::At,
                            description,
                            vendor_id: if vid.is_empty() { None } else { Some(vid) },
                            product_id: if pid.is_empty() { None } else { Some(pid) },
                            profile_id: Some(profile.profile_id()),
                            has_profile: !profile.is_generic(),
                            bus_port: Some(bus_port.clone()),
                            all_ports: ports.clone(),
                        });
                    } else {
                        warn!("ttyUSB ports on bus {} but no suitable AT port", bus_port);
                    }
                }
            }
            Err(e) => {
                warn!("Failed to glob /dev/ttyUSB*: {}", e);
            }
        }

        // Last resort: just try ttyUSB2 directly
        if modems.is_empty() {
            let fallback = "/dev/ttyUSB2";
            if Path::new(fallback).exists() {
                let identity = get_modem_identity(fallback);
                let (vid, pid) = identity.unwrap_or_default();
                let profile = if !vid.is_empty() {
                    registry.match_profile(&vid, &pid)
                } else {
                    generic
                };

                let description = if !profile.is_generic() {
                    format!("{} {}", profile.identity.manufacturer, profile.identity.model)
                } else {
                    "Modem (ttyUSB2)".to_string()
                };

                narrate!(verbosity, "Using direct fallback: {} (profile: {})", fallback, profile.profile_id());
                modems.push(DetectedModem {
                    device_path: fallback.to_string(),
                    protocol: ModemProtocol::At,
                    description,
                    vendor_id: if vid.is_empty() { None } else { Some(vid) },
                    product_id: if pid.is_empty() { None } else { Some(pid) },
                    profile_id: Some(profile.profile_id()),
                    has_profile: !profile.is_generic(),
                    bus_port: None,
                    all_ports: Vec::new(),
                });
            } else {
                warn!("No modem devices found!");
            }
        }

        narrate!(verbosity, "Detection complete: {} modem(s) found", modems.len());
        for (i, m) in modems.iter().enumerate() {
            narrate!(verbosity, "  [{}] {} at {} (vid={:?} pid={:?} profile={:?} has_profile={})",
                  i, m.description, m.device_path, m.vendor_id, m.product_id,
                  m.profile_id, m.has_profile);
        }
        modems
    }

    /// Enumerate `/sys/class/tty/ttyUSB*`, keep those whose USB bus-port matches
    /// `bus_port`, and return them sorted (deterministic ordering for preference
    /// matching). Extracted from `find_at_port` so `reopen_port` can re-detect
    /// the AT port for a handler's stable bus-port after a re-enumeration.
    fn at_ports_for_bus_port(bus_port: &str) -> Vec<String> {
        let mut ports: Vec<String> = Vec::new();
        if let Ok(entries) = glob("/sys/class/tty/ttyUSB*") {
            for entry in entries.flatten() {
                if let Some(name) = entry.file_name().and_then(|n| n.to_str()) {
                    if let Some(port_bus) = get_usb_bus_port(name) {
                        if port_bus == bus_port {
                            ports.push(format!("/dev/{name}"));
                        }
                    }
                }
            }
        }
        ports.sort();
        ports
    }

    fn find_at_port(
        qmi_path: &str,
        port_preference: &[String],
        interface_preference: &[u32],
        verbosity: DetectionVerbosity,
    ) -> Option<String> {
        let device_name = Path::new(qmi_path).file_name()?.to_str()?;

        // Use get_usb_bus_port (which uses canonicalize) to resolve the bus-port
        let bus_port = get_usb_bus_port(device_name)?;
        narrate!(verbosity, "find_at_port: QMI device {} on bus-port {}", device_name, bus_port);

        // Find ttyUSB ports on the same bus-port using canonicalize
        let ports = at_ports_for_bus_port(&bus_port);
        narrate!(verbosity, "find_at_port: {} ttyUSB port(s) on bus-port {}: {:?}", ports.len(), bus_port, ports);

        // 1. Try name-based preference
        for port_name in port_preference {
            let port_path = format!("/dev/{port_name}");
            if ports.contains(&port_path) {
                return Some(port_path);
            }
        }

        // 2. Try USB interface-based preference
        for &iface in interface_preference {
            if let Some(port) = ports.iter().find(|p| {
                let name = Path::new(p.as_str())
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("");
                get_usb_interface_number(name) == Some(iface)
            }) {
                return Some(port.clone());
            }
        }

        // 3. Fall back to first port
        ports.into_iter().next()
    }

    /// Extract USB vendor/product IDs from sysfs uevent.
    /// Returns (vendor_id, product_id).
    ///
    /// Checks multiple sysfs paths to support different device types:
    /// - /sys/class/usbmisc/<name>/device/uevent (QMI devices like cdc-wdm0)
    /// - /sys/class/tty/<name>/device/uevent (serial devices like ttyUSB2)
    /// Then walks up the sysfs tree to find the USB device-level PRODUCT= line.
    pub fn get_modem_identity(device_path: &str) -> Option<(String, String)> {
        let device_name = Path::new(device_path).file_name()?.to_str()?;

        // Try multiple sysfs class paths
        let sysfs_bases = [
            format!("/sys/class/usbmisc/{device_name}/device"),
            format!("/sys/class/tty/{device_name}/device"),
        ];

        for base in &sysfs_bases {
            let base_path = Path::new(base);
            if !base_path.exists() {
                continue;
            }

            // Resolve symlinks to get the real filesystem path.
            // /sys/class/tty/ttyUSB2/device is a symlink to something like
            // /sys/devices/platform/.../usb4/4-1/4-1:1.2 (USB interface level).
            // PRODUCT= is at the USB device level (one directory up from interface).
            // Without canonicalize, PathBuf::pop() does lexical manipulation and
            // would go to /sys/class/tty/ttyUSB2 instead of the real parent.
            let real_path = match fs::canonicalize(base_path) {
                Ok(p) => p,
                Err(_) => continue,
            };

            // Walk up from the real path checking uevent files at each level
            let mut current = real_path;
            for _ in 0..5 {
                let uevent = current.join("uevent");
                if let Ok(content) = fs::read_to_string(&uevent) {
                    if let Some(ids) = parse_product_from_uevent(&content) {
                        return Some(ids);
                    }
                }
                // Walk up to parent
                if !current.pop() {
                    break;
                }
            }
        }
        None
    }

    /// Parse PRODUCT= line from a sysfs uevent file.
    /// Format is PRODUCT=vid/pid/bcdDevice (hex values separated by /).
    fn parse_product_from_uevent(content: &str) -> Option<(String, String)> {
        for line in content.lines() {
            if line.starts_with("PRODUCT=") {
                let product = line.trim_start_matches("PRODUCT=");
                let parts: Vec<&str> = product.split('/').collect();
                if parts.len() >= 2 {
                    // Zero-pad to 4 hex digits (sysfs strips leading zeros: "800" → "0800")
                    let vid = format!("{:0>4}", parts[0].to_lowercase());
                    let pid = format!("{:0>4}", parts[1].to_lowercase());
                    return Some((vid, pid));
                }
            }
        }
        None
    }

    /// Extract USB bus-port identifier from a device name.
    ///
    /// Given "ttyUSB2" or "cdc-wdm0", resolves the sysfs symlink to its real
    /// absolute path and extracts the USB bus-port string (e.g., "4-1" or "4-1.2"
    /// from path like "/sys/devices/.../usb4/4-1/4-1.2/4-1.2:1.2/ttyUSB5").
    /// This uniquely identifies which physical USB device the port belongs to.
    ///
    /// Uses `fs::canonicalize` (not `fs::read_link`) because sysfs symlinks are
    /// relative on OpenWRT (e.g., `../../../ttyUSB0`), and the raw relative path
    /// doesn't contain bus-port information.
    fn get_usb_bus_port(device_name: &str) -> Option<String> {
        let sysfs_bases = [
            format!("/sys/class/tty/{device_name}/device"),
            format!("/sys/class/usbmisc/{device_name}/device"),
        ];

        for base in &sysfs_bases {
            let base_path = Path::new(base);
            if !base_path.exists() {
                continue;
            }
            // canonicalize resolves symlinks to the real absolute sysfs path, e.g.:
            // /sys/devices/platform/.../usb4/4-1/4-1.2/4-1.2:1.2
            if let Ok(real_path) = fs::canonicalize(base_path) {
                let path_str = real_path.to_string_lossy();
                let mut best: Option<String> = None;
                for component in path_str.split('/') {
                    // Match bus-port patterns like "4-1" or "4-1.2" (no colon = not an interface)
                    if component.contains('-') && !component.contains(':') {
                        let parts: Vec<&str> = component.split('-').collect();
                        if parts.len() == 2 && parts[0].parse::<u32>().is_ok() {
                            // Keep the longest match (deepest hub level, e.g., "4-1.2" over "4-1")
                            if best.as_ref().is_none_or(|b| component.len() > b.len()) {
                                best = Some(component.to_string());
                            }
                        }
                    }
                }
                if best.is_some() {
                    return best;
                }
            }
        }
        None
    }

    /// Generate a stable, unique identifier for a modem.
    ///
    /// Uses a fallback chain for maximum reliability:
    /// 1. USB Serial Number (hardware-level, most stable)
    /// 2. MAC Address (network interface)
    ///
    /// Returns format: `{VID}:{PID}:{USB_SERIAL}` or `{VID}:{PID}:MAC-{MAC}`
    ///
    /// # Examples
    /// - `2c7c:0122:e3183572` (USB serial)
    /// - `1bc7:1073:MAC-0012d1234567` (MAC fallback)
    pub fn generate_modem_id_impl(detected: &DetectedModem) -> HardwareResult<String> {
        let vid = detected.vendor_id.as_deref().unwrap_or("0000");
        let pid = detected.product_id.as_deref().unwrap_or("0000");

        // Priority 1: Try USB serial from sysfs
        if let Some(bus_port) = detected.bus_port.as_deref() {
            if let Some(serial) = read_usb_serial(bus_port) {
                if !serial.is_empty() && serial != "0000" {
                    return Ok(format!("{vid}:{pid}:{serial}"));
                }
            }

            // Priority 2: Try MAC address from network interface
            if let Some(mac) = get_modem_mac_address(bus_port) {
                tracing::warn!(
                    "USB serial unavailable for bus-port {bus_port}, using MAC address as fallback"
                );
                // Strip colons from MAC for cleaner ID (AA:BB:CC:DD:EE:FF → AABBCCDDEEFF)
                let mac_clean = mac.replace(":", "");
                return Ok(format!("{vid}:{pid}:MAC-{mac_clean}"));
            }
        }

        // No stable identifier available
        let bus_port_str = detected.bus_port.as_deref().unwrap_or("unknown");
        Err(HardwareError::Internal(format!(
            "No stable identifier available for modem at bus-port {bus_port_str}"
        )))
    }

    /// Read USB serial number from sysfs.
    ///
    /// Reads from `/sys/bus/usb/devices/{bus-port}/serial`.
    /// Returns `None` if file doesn't exist or is empty.
    fn read_usb_serial(bus_port: &str) -> Option<String> {
        let serial_path = format!("/sys/bus/usb/devices/{bus_port}/serial");
        fs::read_to_string(&serial_path)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    /// Get MAC address for a modem's network interface.
    ///
    /// Finds the network device for the given bus-port, then reads its MAC
    /// address from `/sys/class/net/{device}/address`.
    fn get_modem_mac_address(bus_port: &str) -> Option<String> {
        let net_device = find_net_device_for_bus_port(bus_port)?;
        let mac_path = format!("/sys/class/net/{net_device}/address");
        fs::read_to_string(&mac_path)
            .ok()
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty() && s != "00:00:00:00:00:00")
    }

    /// Find the `/dev/cdc-wdmN` control device path for a given USB bus-port.
    ///
    /// Scans `/sys/class/usbmisc/cdc-wdm*` entries; for each, walks the
    /// canonicalized `device` symlink to extract the bus-port (mirrors the
    /// existing `get_usb_bus_port` algorithm); returns `Some("/dev/<basename>")`
    /// for the first match.
    ///
    /// Per Item #37 sub-task 2b spec §4 Q-E (E2), the driver-name filter is
    /// permissive — any driver bound to a cdc-wdm device under
    /// `/sys/class/usbmisc/` is accepted, because the caller already decided
    /// proto=qmi/mbim before invoking this helper. Future drivers (cdc_eem,
    /// vendor-specific) work without this helper learning about them.
    ///
    /// Returns `None` if no match. One-shot — no retry on USB enumeration race;
    /// the next reconcile cycle retries naturally.
    pub fn find_qmi_control_device_for_bus_port(target_bus_port: &str) -> Option<String> {
        find_qmi_control_device_in(Path::new("/sys/class/usbmisc"), target_bus_port)
    }

    /// Test-injectable variant of `find_qmi_control_device_for_bus_port` —
    /// reads from an arbitrary `usbmisc_root` (production passes
    /// `/sys/class/usbmisc`).
    pub fn find_qmi_control_device_in(
        usbmisc_root: &Path,
        target_bus_port: &str,
    ) -> Option<String> {
        let pattern = format!("{}/cdc-wdm*", usbmisc_root.display());
        if let Ok(entries) = glob(&pattern) {
            for entry in entries.flatten() {
                let Some(device_name) = entry.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                let device_name = device_name.to_string();
                let device_link = entry.join("device");
                if !device_link.exists() {
                    continue;
                }
                if let Ok(real_path) = fs::canonicalize(&device_link) {
                    let path_str = real_path.to_string_lossy();
                    let mut best: Option<String> = None;
                    for component in path_str.split('/') {
                        if component.contains('-') && !component.contains(':') {
                            let parts: Vec<&str> = component.split('-').collect();
                            if parts.len() == 2 && parts[0].parse::<u32>().is_ok() {
                                if best.as_ref().is_none_or(|b| component.len() > b.len()) {
                                    best = Some(component.to_string());
                                }
                            }
                        }
                    }
                    if let Some(bp) = best {
                        if bp == target_bus_port {
                            tracing::debug!(
                                "find_qmi_control_device: matched {device_name} for bus-port {target_bus_port}"
                            );
                            return Some(format!("/dev/{}", device_name));
                        }
                    }
                }
            }
        }
        tracing::debug!(
            "find_qmi_control_device: no match for bus-port {target_bus_port}"
        );
        None
    }

    /// Find the Linux network device for a given USB bus-port.
    ///
    /// Scans all `/sys/class/net/*` entries that have a `device` symlink (which
    /// filters out virtual interfaces like lo, br-lan) and resolves each to
    /// extract its bus-port. Matches ECM (usb*), QMI (wwan*), and any other
    /// USB-backed network interface naming convention.
    pub fn find_net_device_for_bus_port(target_bus_port: &str) -> Option<String> {
        // Scan all net interfaces — the `device` symlink check filters out virtuals
        let pattern = "/sys/class/net/*";
        if let Ok(entries) = glob(pattern) {
            for entry in entries.flatten() {
                if let Some(iface_name) = entry.file_name().and_then(|n| n.to_str()) {
                    // Skip known non-modem interfaces
                    if matches!(iface_name, "lo" | "sit0")
                        || iface_name.starts_with("br-")
                        || iface_name.starts_with("docker")
                    {
                        continue;
                    }
                    // Only consider interfaces with a device symlink (USB-backed)
                    let device_path = entry.join("device");
                    if !device_path.exists() {
                        continue;
                    }
                    if let Ok(real_path) = fs::canonicalize(&device_path) {
                        let path_str = real_path.to_string_lossy();
                        // Extract bus-port from the resolved path using same logic as get_usb_bus_port
                        let mut best: Option<String> = None;
                        for component in path_str.split('/') {
                            if component.contains('-') && !component.contains(':') {
                                let parts: Vec<&str> = component.split('-').collect();
                                if parts.len() == 2 && parts[0].parse::<u32>().is_ok() {
                                    if best.as_ref().is_none_or(|b| component.len() > b.len()) {
                                        best = Some(component.to_string());
                                    }
                                }
                            }
                        }
                        if let Some(bp) = best {
                            if bp == target_bus_port {
                                tracing::debug!(
                                    "find_net_device: matched {iface_name} for bus-port {target_bus_port}"
                                );
                                return Some(iface_name.to_string());
                            }
                        }
                    }
                }
            }
        }
        tracing::debug!("find_net_device: no match for bus-port {target_bus_port}");
        None
    }

    /// Extract USB interface number from a ttyUSB device's sysfs path.
    ///
    /// Given "ttyUSB5", resolves the sysfs path and finds the USB interface
    /// component (e.g., "4-1.2:1.2" → interface 2). Returns the interface number
    /// (the part after ":1.").
    fn get_usb_interface_number(device_name: &str) -> Option<u32> {
        let sysfs_path = format!("/sys/class/tty/{device_name}/device");
        let base_path = Path::new(&sysfs_path);
        if !base_path.exists() {
            return None;
        }
        let real_path = fs::canonicalize(base_path).ok()?;
        let path_str = real_path.to_string_lossy();
        // Look for component like "4-1.2:1.2" — the interface descriptor
        for component in path_str.split('/') {
            if let Some(colon_pos) = component.find(':') {
                let after_colon = &component[colon_pos + 1..];
                // USB interface format is "config.interface", e.g., "1.2"
                if let Some(dot_pos) = after_colon.find('.') {
                    if let Ok(iface_num) = after_colon[dot_pos + 1..].parse::<u32>() {
                        return Some(iface_num);
                    }
                }
            }
        }
        None
    }

    pub fn create_modem_handler_impl(
        modem: &DetectedModem,
        profile: ModemProfile,
    ) -> HardwareResult<Box<dyn ModemHardware + Send>> {
        match modem.protocol {
            ModemProtocol::At => {
                // Build ordered list of ports to try: primary first, then remaining all_ports
                let mut ports_to_try: Vec<&str> = vec![&modem.device_path];
                for p in &modem.all_ports {
                    if p != &modem.device_path {
                        ports_to_try.push(p);
                    }
                }

                let mut last_error = None;
                for port_path in &ports_to_try {
                    match AtHandler::new(port_path, profile.clone(), modem.bus_port.clone()) {
                        Ok(handler) => match handler.verify_sync() {
                            Ok(()) => {
                                if *port_path != modem.device_path {
                                    info!(
                                        "Handler verified on fallback port {} (primary {} failed)",
                                        port_path, modem.device_path
                                    );
                                }
                                return Ok(Box::new(handler));
                            }
                            Err(e) => {
                                debug!("Port {} opened but AT verify failed: {}", port_path, e);
                                last_error = Some(e);
                            }
                        },
                        Err(e) => {
                            debug!("Port {} open failed: {}", port_path, e);
                            last_error = Some(e);
                        }
                    }
                }

                Err(last_error.unwrap_or_else(|| {
                    HardwareError::DeviceNotFound("No responsive AT port found".into())
                }))
            }
            _ => Err(HardwareError::Internal(
                format!("{:?} protocol not yet implemented", modem.protocol),
            )),
        }
    }

}

// =============================================================================
// Public API
// =============================================================================

/// Controls how chatty `detect_modems` is. Boot and operator-initiated rescans
/// use `Verbose` (full INFO narration — the useful first-run / troubleshooting
/// record). The periodic 30s hot-plug rescan uses `Quiet` (narration drops to
/// `debug!`), so a healthy router does not flood `logread` twice a minute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectionVerbosity {
    Verbose,
    Quiet,
}

/// Detect available modems on the system.
///
/// Scans /dev for modem devices and returns a list that can be used
/// with `create_modem_handler`. Uses the profile registry to identify
/// modem models and determine port preferences.
pub fn detect_modems(registry: &super::profiles::ProfileRegistry, verbosity: DetectionVerbosity) -> Vec<DetectedModem> {
    #[cfg(feature = "real-hardware")]
    {
        match verbosity {
            DetectionVerbosity::Verbose => tracing::info!("detect_modems: using real-hardware implementation"),
            DetectionVerbosity::Quiet => tracing::debug!("detect_modems: using real-hardware implementation"),
        }
        real_hardware::detect_modems_impl(registry, verbosity)
    }
    #[cfg(not(feature = "real-hardware"))]
    {
        let _ = registry;
        match verbosity {
            DetectionVerbosity::Verbose => tracing::info!("detect_modems: real-hardware not enabled, returning empty"),
            DetectionVerbosity::Quiet => tracing::debug!("detect_modems: real-hardware not enabled, returning empty"),
        }
        vec![]
    }
}

/// Generate a stable, unique identifier for a modem.
///
/// Uses a fallback chain:
/// 1. USB Serial Number (hardware-level, most stable)
/// 2. MAC Address (network interface)
///
/// Format: {VID}:{PID}:{USB_SERIAL} or {VID}:{PID}:MAC-{MAC}
///
/// Example: "2c7c:0122:e3183572"
pub fn generate_modem_id(detected: &DetectedModem) -> HardwareResult<String> {
    #[cfg(feature = "real-hardware")]
    {
        real_hardware::generate_modem_id_impl(detected)
    }
    #[cfg(not(feature = "real-hardware"))]
    {
        // Mock hardware: use VID:PID:bus-port
        let vid = detected.vendor_id.as_deref().unwrap_or("0000");
        let pid = detected.product_id.as_deref().unwrap_or("0000");
        let bus_port = detected.bus_port.as_deref().unwrap_or("unknown")
            .replace("-", "").replace(".", "");
        Ok(format!("{vid}:{pid}:MOCK-{bus_port}"))
    }
}

/// Create a modem handler for the specified device.
///
/// Returns a boxed trait object that implements ModemHardware.
/// The profile drives AT command selection, signal parsing, and port config.
pub fn create_modem_handler(
    modem: &DetectedModem,
    profile: super::profiles::ModemProfile,
) -> HardwareResult<Box<dyn ModemHardware + Send>> {
    #[cfg(feature = "real-hardware")]
    {
        real_hardware::create_modem_handler_impl(modem, profile)
    }
    #[cfg(not(feature = "real-hardware"))]
    {
        let _ = (modem, profile);
        Err(HardwareError::Internal(
            "Real hardware not enabled - use MockHardware or enable real-hardware feature".to_string(),
        ))
    }
}

/// Find the Linux network device for a given USB bus-port.
///
/// Scans all USB-backed network interfaces under `/sys/class/net/` and
/// matches by bus-port. Supports ECM (usb*), QMI (wwan*), and other naming.
pub fn find_net_device_for_bus_port(target_bus_port: &str) -> Option<String> {
    #[cfg(feature = "real-hardware")]
    {
        real_hardware::find_net_device_for_bus_port(target_bus_port)
    }
    #[cfg(not(feature = "real-hardware"))]
    {
        let _ = target_bus_port;
        None
    }
}

/// Find the QMI/MBIM control device path (`/dev/cdc-wdmN`) for a given USB bus-port.
///
/// Real-hardware builds scan `/sys/class/usbmisc/cdc-wdm*`; mock builds always
/// return `None`. See `find_net_device_for_bus_port` for the netif counterpart.
///
/// Used by `reconcile_uci_section` (Item #37 sub-task 2b) to write the correct
/// UCI `option device` value for proto=qmi/mbim sections.
pub fn find_qmi_control_device_for_bus_port(target_bus_port: &str) -> Option<String> {
    #[cfg(feature = "real-hardware")]
    {
        real_hardware::find_qmi_control_device_for_bus_port(target_bus_port)
    }
    #[cfg(not(feature = "real-hardware"))]
    {
        let _ = target_bus_port;
        None
    }
}

// =============================================================================
// F1 — stale serial fd self-heal (Layer 1)
//
// These items live at module scope (NOT behind the `real-hardware` gate) so the
// fd-dead classifier and the reopen-once retry state machine compile and run
// under default (mock-hardware) features in CI. The real `serialport::TTYPort`
// supplies the `SerialIo` impl and `AtHandler::reopen_port` supplies the
// `PortOpener`, both behind the gate; the loop logic itself is shared and tested
// here over in-memory fakes.
//
// `#[allow(dead_code)]`: in a non-test default-features build these are reached
// only through the `real-hardware`-gated `send_command`, so the dead-code lint
// fires. They ARE exercised by the CI `mod tests` (default features) and by the
// real build. Same pattern as `nmea_to_decimal` above.
// =============================================================================

/// Classify an I/O error kind as "the serial fd is dead, re-detect + reopen" vs
/// "stay on the current fd".
///
/// WHY a tight allow-set: a CFUN cycle or modem reboot re-enumerates the USB AT
/// port (ttyUSB2↔ttyUSB3), invalidating the cached fd. Writes/reads then surface
/// as `BrokenPipe`/`NotConnected`/`UnexpectedEof`. Only those warrant a reopen.
/// `TimedOut` is the read loop's ordinary "no data yet" tick (must remain a
/// `continue`, never a reopen); `Interrupted` (EINTR) is transient with a live
/// fd. Everything else (protocol/permission/data errors) leaves the fd intact —
/// reopening would mask a real fault and risk a needless re-detect storm.
#[allow(dead_code)] // used by real-hardware send_command and tests
pub fn should_reopen_after_io_error(kind: std::io::ErrorKind) -> bool {
    matches!(
        kind,
        std::io::ErrorKind::BrokenPipe
            | std::io::ErrorKind::NotConnected
            | std::io::ErrorKind::UnexpectedEof
    )
}

/// Linux errno values meaning "the device is physically gone" — a write/read to a
/// USB-serial that has been removed (re-enumeration / unplug) returns one of these.
/// Hardcoded (not `libc::`) so this pure classifier compiles under default features
/// for CI tests with fakes; the daemon only runs on Linux/musl where these hold.
const ERRNO_EIO: i32 = 5;
const ERRNO_ENXIO: i32 = 6;
const ERRNO_ENODEV: i32 = 19;

/// True when an I/O error's raw OS errno indicates the device is gone (reopen-worthy).
/// Complements `should_reopen_after_io_error` (which keys on `io::ErrorKind`): a
/// disconnected-device write/read typically yields EIO/ENXIO/ENODEV, which map to
/// `ErrorKind::Other`/`Uncategorized` (not in that allowlist). Matched narrowly —
/// NOT all `Other` — so genuine faults are not masked into a reopen storm.
#[allow(dead_code)] // called by classify_io (itself dead-code-allowed) and by tests
pub fn is_device_gone_errno(raw: Option<i32>) -> bool {
    matches!(raw, Some(ERRNO_EIO) | Some(ERRNO_ENXIO) | Some(ERRNO_ENODEV))
}

/// Minimal serial byte-stream seam: the real `serialport::TTYPort` and the test
/// fakes both satisfy this, so the retry state machine is generic over it.
#[allow(dead_code)] // used by real-hardware send_command and tests
pub trait SerialIo: std::io::Read + std::io::Write + Send {}

/// Re-detects the AT port for the handler's stable bus-port and opens a fresh
/// `SerialIo`. The real opener performs `at_ports_for_bus_port` + a fresh-port
/// AT/OK verify; test fakes hand out scripted ports. Returns an `Err` when
/// re-detect/open fails so the caller can propagate (and let Layer 2 + the cache
/// path engage).
#[allow(dead_code)] // used by real-hardware send_command and tests
pub trait PortOpener {
    type Port: SerialIo;
    fn open(&mut self) -> HardwareResult<Self::Port>;
}

/// Run one AT command over `port`, self-healing a dead fd **once**.
///
/// State machine: write+read-loop on the current port → on an I/O error
/// whose kind is fd-dead (`should_reopen_after_io_error`) and we have not yet
/// reopened, ask `opener` for a fresh port, swap it into `*port`, and retry the
/// full exchange exactly once → otherwise propagate. The read loop preserves the
/// existing `TimedOut ⇒ continue` semantics and is bounded by `cmd_timeout`, so
/// there is no infinite loop and at most one reopen per call.
#[allow(dead_code)] // used by real-hardware send_command and tests
pub fn run_at_command_with_reopen<S, O>(
    port: &mut S,
    opener: &mut O,
    cmd: &str,
    cmd_timeout: std::time::Duration,
) -> HardwareResult<String>
where
    S: SerialIo,
    O: PortOpener<Port = S>,
{
    let mut reopened = false;
    loop {
        match at_exchange(port, cmd, cmd_timeout) {
            Ok(resp) => return Ok(resp),
            Err(AtExchangeError::Timeout) => return Err(HardwareError::Timeout),
            Err(AtExchangeError::Rejected(msg)) => {
                // Fail closed: a control char in the command body is never an I/O
                // fault, so never reopen/retry — reject the whole command.
                return Err(HardwareError::CommandRejected(msg));
            }
            Err(AtExchangeError::Fatal(msg)) => return Err(HardwareError::Io(msg)),
            Err(AtExchangeError::Reopenable(msg)) => {
                if reopened {
                    // Already healed once this call — the fresh fd is dead too.
                    // Propagate rather than spin (Layer 2 / cache recover next).
                    return Err(HardwareError::Io(msg));
                }
                reopened = true;
                let fresh = opener.open()?;
                *port = fresh;
                // loop: retry the exchange once on the fresh fd
            }
        }
    }
}

/// Outcome of a single AT exchange that the reopen driver acts on. Splitting
/// `Reopenable` from `Fatal`/`Timeout` lets the driver decide reopen vs propagate
/// while keeping the `io::ErrorKind` decision local (before it's stringified).
#[allow(dead_code)] // used by real-hardware send_command and tests
#[derive(Debug)]
enum AtExchangeError {
    /// Read loop ran to `cmd_timeout` with no terminal response.
    Timeout,
    /// I/O failure whose kind is fd-dead class — worth one reopen+retry.
    Reopenable(String),
    /// I/O failure on a live fd (or non-fd-dead kind) — propagate as-is.
    Fatal(String),
    /// The caller-supplied command body contained a control character (CR/LF or
    /// any other ASCII control). Rejected at the serial choke point BEFORE any
    /// byte is written — fail closed, never reopen, never retry. This is the
    /// authoritative backstop against multi-line AT-command injection (a
    /// whitelist-validated first token smuggling a second, blocked command via an
    /// embedded `\r`).
    Rejected(String),
}

/// Scan a caller-supplied AT command body for any ASCII control character.
///
/// The serial writer appends its OWN single `\r` terminator; this guard rejects
/// control chars *within* `cmd`, so a validated first token (`AT+CFUN=1`) can
/// never carry an embedded `\r`/`\n` that puts a SECOND command on the wire.
/// Returns the offending byte for the rejection message; `None` means clean.
#[allow(dead_code)] // used by real-hardware send_command and tests
fn find_control_char(cmd: &str) -> Option<u8> {
    cmd.bytes().find(|b| b.is_ascii_control())
}

/// Validate an operator-supplied string that will be interpolated into a quoted
/// AT argument (e.g. the APN in `AT+CGDCONT=<cid>,"IP","<apn>"`).
///
/// Defense-in-depth for the same root cause as the serial-write guard: reject any
/// value containing a double-quote `"` (would break out of the quoted argument)
/// or any ASCII control character (CR/LF would smuggle a second command). The
/// serial choke point is the authoritative backstop, but rejecting here gives a
/// clearer error at the call site and stops a malformed command being built.
#[allow(dead_code)] // used by connect()/CGDCONT builder and tests
fn validate_at_quoted_arg(value: &str) -> HardwareResult<()> {
    if let Some(b) = value.bytes().find(|b| b.is_ascii_control() || *b == b'"') {
        return Err(HardwareError::CommandRejected(format!(
            "AT quoted argument contains forbidden byte 0x{b:02x}"
        )));
    }
    Ok(())
}

/// One write+read-until-terminal exchange over a `SerialIo`. Mirrors the
/// body of `AtHandler::send_command` so the real and test paths share semantics:
/// `TimedOut` reads are skipped (`continue`); a fd-dead I/O error short-circuits
/// to `Reopenable`; other I/O errors are `Fatal`; running out the clock is
/// `Timeout`.
/// Known unsolicited result code (URC) line prefixes that must be filtered out
/// of a command's response (R3, 2026-06-18 AT-channel read-framing spec).
///
/// Universal 3GPP family + Quectel family. The daemon consumes NO URCs anywhere,
/// so stripping these from command output loses nothing the app uses; it just
/// keeps downstream parsers from substring-matching a URC as if it were the
/// command's response (the dev.56 MBN-false-reboot root cause).
///
/// **Mode-agnostic:** this is a universal constant set, not a per-modem branch
/// (`feedback_modem_mode_agnostic.md`). Modem-specific additions, if ever
/// required, belong in the profile struct (`profiles.rs`) — noted as a future
/// extension, intentionally NOT implemented this pass; the universal set covers
/// the bench RM520N-GL / FN990.
///
/// **Dual-use prefixes** (`+CPIN:`, `+CGREG:`, `+CEREG:`, `+CREG:`) are both URC
/// and query-response. They are listed here but the dual-use guard in
/// `at_exchange` keeps any line whose prefix matches the *issued command's own
/// response family* (e.g. `AT+CPIN?` → keep `+CPIN:`). When in doubt, keep.
#[allow(dead_code)] // used by real-hardware send_command and tests
const URC_PREFIXES: &[&str] = &[
    // 3GPP registration / network state
    "+CREG:",
    "+CGREG:",
    "+CEREG:",
    "+CGEV:",
    // 3GPP SMS / messaging
    "+CMTI:",
    "+CMT:",
    "+CMGS:",
    "+CDS:",
    "+CBM:",
    // 3GPP supplementary / call
    "+CUSD:",
    "+CRING:",
    "+CLIP:",
    "+CCWA:",
    // Power / SIM state URCs
    "+CPIN:",
    "+QUSIM:",
    // Quectel indications
    "+QIND:",
    "+QUSIM",
    "+QMBN:",
    "+QNETDEVSTATUS:",
];

/// Bare (non-`+`-prefixed) URC lines that arrive on their own line.
#[allow(dead_code)] // used by real-hardware send_command and tests
const URC_BARE_LINES: &[&str] = &["RDY", "POWERED DOWN", "NORMAL POWER DOWN", "+CPIN: READY"];

/// Derive the expected response-line prefix for an issued AT command, used by
/// the R3 dual-use guard so a command's own response family is never filtered.
///
/// `AT+CPIN?` → `+CPIN:`; `AT+QMBNCFG="AutoSel"` → `+QMBNCFG:`;
/// `AT+CREG?` → `+CREG:`. Returns `None` for commands with no `+`-prefixed
/// query form (bare `AT`, `ATI`, etc.).
#[allow(dead_code)] // used by real-hardware send_command and tests
fn expected_response_prefix(cmd: &str) -> Option<String> {
    // Strip a leading "AT" (case-insensitive), then take up to the first of
    // '=', '?', or end — the command name. Only "+"-prefixed names have a
    // "+NAME:" response family.
    let c = cmd.trim();
    let body = c
        .strip_prefix("AT")
        .or_else(|| c.strip_prefix("at"))
        .unwrap_or(c);
    let body = body.trim_start_matches(['\r', '\n']);
    if !body.starts_with('+') {
        return None;
    }
    let name: String = body
        .chars()
        .take_while(|&ch| ch != '=' && ch != '?')
        .collect();
    let name = name.trim();
    if name.len() <= 1 {
        return None;
    }
    Some(format!("{name}:"))
}

/// Whether `trimmed_line` is a URC that should be filtered from a response,
/// given the issued command's expected response prefix (the dual-use guard).
///
/// A line is filtered iff it matches a known URC prefix/bare form AND does NOT
/// match the command's own response family. When uncertain → keep (return
/// false): over-filtering a real response is worse than leaking a URC.
#[allow(dead_code)] // used by real-hardware send_command and tests
fn is_urc_line(trimmed_line: &str, expected_prefix: Option<&str>) -> bool {
    // Dual-use guard: never filter the command's own response family.
    if let Some(exp) = expected_prefix {
        if trimmed_line.starts_with(exp) {
            return false;
        }
    }
    if URC_PREFIXES.iter().any(|p| trimmed_line.starts_with(p)) {
        return true;
    }
    if URC_BARE_LINES.contains(&trimmed_line) {
        return true;
    }
    false
}

#[allow(dead_code)] // used by real-hardware send_command and tests
fn at_exchange<S: SerialIo>(
    port: &mut S,
    cmd: &str,
    cmd_timeout: std::time::Duration,
) -> Result<String, AtExchangeError> {
    // `write_all`/`flush` resolve through the `SerialIo: Write` supertrait bound;
    // only `BufRead`/`BufReader` need importing here.
    use std::io::{BufRead, BufReader};

    // CRITICAL serial-write control-char guard. The AT whitelist (security layer)
    // validates only the first token of a command; the serial path otherwise
    // transmits every byte verbatim. An embedded CR/LF inside a validated command
    // (`AT+CFUN=1\rAT+QFASTBOOT`) would put a SECOND, permanently-blocked command
    // on the wire — defeating the whitelist. Reject ANY caller-supplied control
    // char BEFORE writing anything (fail closed; no partial/sanitized write). The
    // single trailing `\r` we append below is added AFTER this check, so it is
    // never affected.
    if let Some(b) = find_control_char(cmd) {
        return Err(AtExchangeError::Rejected(format!(
            "AT command contains forbidden control byte 0x{b:02x} (possible multi-command injection)"
        )));
    }

    // R1 — Pre-command input drain. Discard any bytes already pending in the
    // serial input buffer (the stale tail of a prior command's late response, or
    // accumulated URCs) so cross-command carryover can never contaminate this
    // exchange. Bounded + non-blocking: read-and-discard whatever is immediately
    // available; the FIRST TimedOut/WouldBlock/EOF means "nothing more pending"
    // and ends the drain — we never block waiting for more bytes. The iteration
    // cap is a belt-and-braces guard against a pathological always-ready fd.
    {
        let mut scratch = [0u8; 512];
        for _ in 0..256 {
            match port.read(&mut scratch) {
                Ok(0) => break, // EOF — nothing pending
                Ok(_) => continue, // discarded a chunk; check for more
                Err(e)
                    if e.kind() == std::io::ErrorKind::TimedOut
                        || e.kind() == std::io::ErrorKind::WouldBlock =>
                {
                    break // buffer drained — no data immediately available
                }
                // A real I/O error here is the same fd-dead class the write below
                // would hit; surface it through the same classifier.
                Err(e) => return Err(classify_io(e, "Drain read failed")),
            }
        }
    }

    let cmd_bytes = format!("{cmd}\r");

    if let Err(e) = port.write_all(cmd_bytes.as_bytes()) {
        return Err(classify_io(e, "Write failed"));
    }
    // No explicit flush(): serialport's TTYPort::flush() calls tcdrain(), which
    // blocks unbounded in tty_wait_until_sent on a removed device (ignores the
    // port timeout) and is uncancellable by tokio::timeout — the wedge this fix
    // exists to kill. The kernel transmits the queued write bytes without an
    // explicit drain; the bounded read loop below waits for the response.

    // R2/R3 framing state. We anchor capture on the command echo: deployment
    // modems echo by default (echo ON; the daemon never issues ATE0).
    let echo_target = cmd.trim();
    let expected_prefix = expected_response_prefix(cmd);
    let mut echo_seen = false;
    // Tracks pre-echo carryover. Set when a non-blank, non-echo line is seen
    // before our echo — i.e. stale content from a prior exchange is in flight
    // (the `AT\r` of a crossing `AT\r\r\nOK`). While carryover is present, a
    // pre-echo terminal is stale and must be discarded (R2b). With NO carryover
    // and no echo (ATE0), a terminal is accepted as ours (R2 degrade clause).
    let mut pre_echo_noise = false;

    let mut response = String::new();
    let start = std::time::Instant::now();
    let mut reader = BufReader::new(&mut *port);

    while start.elapsed() < cmd_timeout {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {
                let trimmed = line.trim();

                // R2(a) — recognize and strip the command echo, anchoring capture.
                if !echo_seen && trimmed == echo_target {
                    echo_seen = true;
                    continue;
                }

                let is_terminal = trimmed == "OK"
                    || trimmed == "ERROR"
                    || trimmed.starts_with("+CME ERROR")
                    || trimmed.starts_with("+CMS ERROR");

                // R2(b) — pre-echo handling. We have not yet seen our echo, so
                // this line is either (i) stale carryover from a prior exchange
                // (a stale command echo `AT...`, a stale terminal, or a URC) or
                // (ii) genuine response content from an ATE0 (echo-off) modem.
                if !echo_seen {
                    // A stale command-echo line (`AT`/`at...`) — e.g. the `AT\r`
                    // of a crossing `AT\r\r\nOK`. Always carryover; discard and
                    // flag that a prior exchange's tail is in flight.
                    let looks_like_echo =
                        trimmed.len() >= 2 && trimmed[..2].eq_ignore_ascii_case("AT");
                    if looks_like_echo {
                        pre_echo_noise = true;
                        continue;
                    }
                    // A pre-echo URC is always discarded regardless (R3 final
                    // sentence) — it is never command output.
                    if is_urc_line(trimmed, expected_prefix.as_deref()) {
                        continue;
                    }
                    if is_terminal {
                        if pre_echo_noise {
                            // Stale terminal (the crossing `...\r\nOK`). Discard;
                            // keep reading for our real echo + response. Clear the
                            // flag so a later bodyless reply isn't blocked.
                            pre_echo_noise = false;
                            continue;
                        }
                        // No carryover seen and no echo (ATE0): after R1's drain
                        // the buffer holds only our exchange, so this terminal is
                        // ours — accept it (R2 degrade-safely clause).
                        response.push_str(&line);
                        return Ok(response);
                    }
                    // Genuine pre-echo response content (ATE0 modem). Blank lines
                    // are dropped; real content is captured.
                    if !trimmed.is_empty() {
                        response.push_str(&line);
                    }
                    continue;
                }

                // --- Post-echo: this is our exchange's output. ---

                // R3 — filter known URCs (with the dual-use guard) so downstream
                // parsers receive clean command output.
                if is_urc_line(trimmed, expected_prefix.as_deref()) {
                    continue;
                }

                response.push_str(&line);
                if is_terminal {
                    return Ok(response);
                }
            }
            // Existing semantics: a serial read timeout means "no data yet" —
            // keep polling until the command timeout, never reopen.
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => continue,
            Err(e) => return Err(classify_io(e, "Read failed")),
        }
    }

    Err(AtExchangeError::Timeout)
}

/// Map an `io::Error` to a reopen-or-propagate verdict, reading `kind()` BEFORE
/// it is flattened into a string (`HardwareError::Io(String)` loses the kind).
#[allow(dead_code)] // used by real-hardware send_command and tests
fn classify_io(e: std::io::Error, ctx: &str) -> AtExchangeError {
    let msg = format!("{ctx}: {e}");
    if should_reopen_after_io_error(e.kind()) || is_device_gone_errno(e.raw_os_error()) {
        AtExchangeError::Reopenable(msg)
    } else {
        AtExchangeError::Fatal(msg)
    }
}

#[cfg(test)]
mod tests {
    /// The RM551E-GL profile regex for single-line LTE QENG responses.
    fn rm551e_signal_regex() -> regex::Regex {
        regex::Regex::new(
            r#"\+QENG:\s*"servingcell","[^"]*","LTE","[^"]*",\d+,\d+,(?P<cellid>[0-9A-Fa-f]+),\d+,(?P<earfcn>\d+),(?P<band>\d+),\d+,\d+,[0-9A-Fa-f]+,(?P<rsrp>-?\d+),(?P<rsrq>-?\d+),(?P<rssi>-?\d+),(?P<sinr>-?\d+)"#
        ).unwrap()
    }

    /// The 2-line LTE regex (matches LTE data line without "servingcell" prefix).
    fn lte_line_regex() -> regex::Regex {
        regex::Regex::new(
            r#"\+QENG:\s*"LTE","[^"]*",\d+,\d+,(?P<cellid>[0-9A-Fa-f]+),\d+,(?P<earfcn>\d+),(?P<band>\d+),\d+,\d+,[0-9A-Fa-f]+,(?P<rsrp>-?\d+),(?P<rsrq>-?\d+),(?P<rssi>-?\d+),(?P<sinr>-?\d+)"#
        ).unwrap()
    }

    /// Helper to extract f64 from a named capture group.
    fn cap_f64(caps: &regex::Captures, name: &str) -> f64 {
        caps.name(name).and_then(|m| m.as_str().parse::<f64>().ok()).unwrap_or(-999.0)
    }

    /// Helper to extract string from a named capture group.
    fn cap_str<'a>(caps: &'a regex::Captures<'a>, name: &str) -> &'a str {
        caps.name(name).map(|m| m.as_str()).unwrap_or("")
    }

    // =========================================================================
    // Test 1: Single-line LTE (profile regex)
    // =========================================================================

    #[test]
    fn single_line_lte_matches_profile_regex() {
        let re = rm551e_signal_regex();
        let response = r#"+QENG: "servingcell","NOCONN","LTE","FDD",310,410,29401C0,325,5330,14,3,3,2294,-84,-12,-54,18,12,30,-"#;

        let caps = re.captures(response).expect("profile regex should match single-line LTE");
        assert_eq!(cap_str(&caps, "cellid"), "29401C0");
        assert_eq!(cap_str(&caps, "earfcn"), "5330");
        assert_eq!(cap_str(&caps, "band"), "14");
        assert_eq!(cap_f64(&caps, "rsrp"), -84.0);
        assert_eq!(cap_f64(&caps, "rsrq"), -12.0);
        assert_eq!(cap_f64(&caps, "rssi"), -54.0);
        assert_eq!(cap_f64(&caps, "sinr"), 18.0);
    }

    #[test]
    fn single_line_lte_in_full_at_response() {
        let re = rm551e_signal_regex();
        // Simulates send_command() output with echo + OK
        let response = "AT+QENG=\"servingcell\"\r\n\
            +QENG: \"servingcell\",\"NOCONN\",\"LTE\",\"FDD\",310,410,29401C0,325,5330,14,3,3,2294,-84,-12,-54,18,12,30,-\r\n\
            OK\r\n";

        let caps = re.captures(response).expect("should match inside full AT response");
        assert_eq!(cap_f64(&caps, "rsrp"), -84.0);
        assert_eq!(cap_str(&caps, "band"), "14");
    }

    // =========================================================================
    // Test 2: 2-line LTE (LTE_LINE_RE fallback)
    // =========================================================================

    #[test]
    fn two_line_lte_matches_lte_line_regex() {
        let re = lte_line_regex();
        // 2-line format: servingcell header on line 1, LTE data on line 2
        let response = "+QENG: \"servingcell\",\"NOCONN\"\r\n\
            +QENG: \"LTE\",\"FDD\",310,410,29401C0,325,5330,14,3,3,2294,-84,-12,-54,18,12,30,-\r\n\
            OK\r\n";

        let caps = re.captures(response).expect("LTE line regex should match 2-line format");
        assert_eq!(cap_str(&caps, "cellid"), "29401C0");
        assert_eq!(cap_str(&caps, "earfcn"), "5330");
        assert_eq!(cap_str(&caps, "band"), "14");
        assert_eq!(cap_f64(&caps, "rsrp"), -84.0);
        assert_eq!(cap_f64(&caps, "rsrq"), -12.0);
        assert_eq!(cap_f64(&caps, "rssi"), -54.0);
        assert_eq!(cap_f64(&caps, "sinr"), 18.0);
    }

    #[test]
    fn two_line_lte_not_matched_by_profile_regex() {
        let re = rm551e_signal_regex();
        // Profile regex requires "servingcell"..."LTE" on same line — should NOT match 2-line
        let response = "+QENG: \"servingcell\",\"NOCONN\"\r\n\
            +QENG: \"LTE\",\"FDD\",310,410,29401C0,325,5330,14,3,3,2294,-84,-12,-54,18,12,30,-\r\n\
            OK\r\n";

        assert!(re.captures(response).is_none(),
            "profile regex must NOT match 2-line LTE (servingcell and LTE on different lines)");
    }

    // =========================================================================
    // Test 3: LTE line regex does not match single-line format
    // =========================================================================

    #[test]
    fn lte_line_regex_does_not_match_single_line_servingcell() {
        let re = lte_line_regex();
        // Single-line: +QENG: "servingcell","NOCONN","LTE"... — the "LTE" follows "servingcell",
        // not directly after +QENG:, so LTE_LINE_RE should NOT match
        let response = r#"+QENG: "servingcell","NOCONN","LTE","FDD",310,410,29401C0,325,5330,14,3,3,2294,-84,-12,-54,18"#;

        // The regex starts with \+QENG:\s*"LTE" which expects "LTE" right after +QENG:
        // In single-line, after +QENG: comes "servingcell", not "LTE"
        // However, captures() searches the entire string — let's verify there's no false match.
        // The single-line has +QENG: "servingcell"..."LTE" — the "LTE" is not directly after +QENG:\s*
        assert!(re.captures(response).is_none(),
            "LTE line regex must NOT match single-line format where LTE follows servingcell");
    }

    // =========================================================================
    // Test 4: 3-line NR5G-NSA
    // =========================================================================

    #[test]
    fn nr5g_nsa_three_line_response() {
        // In NSA mode, LTE is always the primary anchor (PCC).
        // NR5G is a secondary carrier — its metrics belong in the CA section.
        let lte_re = regex::Regex::new(
            r#"\+QENG:\s*"LTE","[^"]*",\d+,\d+,(?P<cellid>[0-9A-Fa-f]+),\d+,\d+,(?P<band_lte>\d+),\d+,\d+,[0-9A-Fa-f]+,(?P<rsrp_lte>-?\d+),(?P<rsrq_lte>-?\d+),(?P<rssi>-?\d+),(?P<sinr_lte>-?\d+)"#
        ).unwrap();
        let nsa_re = regex::Regex::new(
            r#"\+QENG:\s*"NR5G-NSA",\d+,\d+,\d+,(?P<rsrp>-?\d+),(?P<sinr>-?\d+),(?P<rsrq>-?\d+),\d+,(?P<band>\d+)"#
        ).unwrap();

        let response = "+QENG: \"servingcell\",\"NOCONN\"\r\n\
            +QENG: \"LTE\",\"FDD\",310,410,29401C0,325,5330,14,3,3,2294,-84,-12,-54,18\r\n\
            +QENG: \"NR5G-NSA\",310,410,500,-95,12,-10,627264,77,100,1\r\n\
            OK\r\n";

        assert!(response.contains("NR5G-NSA"), "response should contain NR5G-NSA");

        // LTE anchor is the primary signal in NSA mode
        let lte_caps = lte_re.captures(response).expect("LTE anchor should match");
        assert_eq!(cap_str(&lte_caps, "cellid"), "29401C0");
        assert_eq!(cap_str(&lte_caps, "band_lte"), "14");
        assert_eq!(cap_f64(&lte_caps, "rsrp_lte"), -84.0);
        assert_eq!(cap_f64(&lte_caps, "rsrq_lte"), -12.0);
        assert_eq!(cap_f64(&lte_caps, "rssi"), -54.0);
        assert_eq!(cap_f64(&lte_caps, "sinr_lte"), 18.0);

        // NR5G-NSA line still parseable (for CA/antenna metrics section)
        let nsa_caps = nsa_re.captures(response).expect("NR5G-NSA line should match");
        assert_eq!(cap_f64(&nsa_caps, "rsrp"), -95.0);
        assert_eq!(cap_f64(&nsa_caps, "sinr"), 12.0);
        assert_eq!(cap_f64(&nsa_caps, "rsrq"), -10.0);
        assert_eq!(cap_str(&nsa_caps, "band"), "77");
    }

    // =========================================================================
    // Test 5: NR5G-SA single-line
    // =========================================================================

    #[test]
    fn nr5g_sa_single_line_response() {
        let nr_re = regex::Regex::new(
            r#"\+QENG:\s*"servingcell","[^"]*","NR5G-SA","[^"]*",\d+,\d+,(?P<cellid>[0-9A-Fa-f]+),[0-9A-Fa-f]+,(?:[0-9A-Fa-f]+,)*(?P<band>\d+),\d+,(?P<rsrp>-?\d+),(?P<rsrq>-?\d+),(?P<sinr>-?\d+)"#
        ).unwrap();

        let response = r#"+QENG: "servingcell","NOCONN","NR5G-SA","TDD",310,410,1A2B3C4,100,627264,77,100,-88,-9,15"#;

        let caps = nr_re.captures(response).expect("NR5G-SA regex should match");
        assert_eq!(cap_str(&caps, "cellid"), "1A2B3C4");
        assert_eq!(cap_str(&caps, "band"), "77");
        assert_eq!(cap_f64(&caps, "rsrp"), -88.0);
        assert_eq!(cap_f64(&caps, "rsrq"), -9.0);
        assert_eq!(cap_f64(&caps, "sinr"), 15.0);
    }

    /// Regression: RM520N-GL NR5G-SA response has hex ARFCN and extra fields.
    #[test]
    fn nr5g_sa_hex_arfcn_response() {
        let nr_re = regex::Regex::new(
            r#"\+QENG:\s*"servingcell","[^"]*","NR5G-SA","[^"]*",\d+,\d+,(?P<cellid>[0-9A-Fa-f]+),[0-9A-Fa-f]+,(?:[0-9A-Fa-f]+,)*(?P<band>\d+),\d+,(?P<rsrp>-?\d+),(?P<rsrq>-?\d+),(?P<sinr>-?\d+)"#
        ).unwrap();

        let response = r#"+QENG: "servingcell","NOCONN","NR5G-SA","FDD",310,260,10244F016,823,ABFB00,387410,25,2,-106,-12,3,0,11"#;

        let caps = nr_re.captures(response).expect("NR5G-SA regex should match hex ARFCN response");
        assert_eq!(cap_str(&caps, "cellid"), "10244F016");
        assert_eq!(cap_str(&caps, "band"), "25");
        assert_eq!(cap_f64(&caps, "rsrp"), -106.0);
        assert_eq!(cap_f64(&caps, "rsrq"), -12.0);
        assert_eq!(cap_f64(&caps, "sinr"), 3.0);
    }

    // =========================================================================
    // Test 6: Antenna metrics — per-metric range validation (profile-driven)
    // =========================================================================

    // Quectel-equivalent range constants for tests
    const SENTINEL: i32 = -32768;
    const RSRP_MIN: i32 = -140;
    const RSRP_MAX: i32 = -44;
    const RSRQ_MIN: i32 = -20;
    const RSRQ_MAX: i32 = -3;
    const SINR_MIN: i32 = -20;
    const SINR_MAX: i32 = 30;

    // --- RSRP (3GPP TS 36.133: -140 to -44 dBm) ---

    #[test]
    fn rsrp_valid_lower_boundary() {
        assert_eq!(super::parse_antenna_val("-140", SENTINEL, RSRP_MIN, RSRP_MAX), Some(-140));
    }

    #[test]
    fn rsrp_valid_upper_boundary() {
        assert_eq!(super::parse_antenna_val("-44", SENTINEL, RSRP_MIN, RSRP_MAX), Some(-44));
    }

    #[test]
    fn rsrp_valid_mid_range() {
        assert_eq!(super::parse_antenna_val("-84", SENTINEL, RSRP_MIN, RSRP_MAX), Some(-84));
    }

    #[test]
    fn rsrp_below_range() {
        assert_eq!(super::parse_antenna_val("-141", SENTINEL, RSRP_MIN, RSRP_MAX), None);
    }

    #[test]
    fn rsrp_above_range() {
        assert_eq!(super::parse_antenna_val("-43", SENTINEL, RSRP_MIN, RSRP_MAX), None);
    }

    #[test]
    fn rsrp_sentinel() {
        assert_eq!(super::parse_antenna_val("-32768", SENTINEL, RSRP_MIN, RSRP_MAX), None);
    }

    // --- RSRQ (3GPP TS 36.133: -20 to -3 dB) ---

    #[test]
    fn rsrq_valid_lower_boundary() {
        assert_eq!(super::parse_antenna_val("-20", SENTINEL, RSRQ_MIN, RSRQ_MAX), Some(-20));
    }

    #[test]
    fn rsrq_valid_upper_boundary() {
        assert_eq!(super::parse_antenna_val("-3", SENTINEL, RSRQ_MIN, RSRQ_MAX), Some(-3));
    }

    #[test]
    fn rsrq_valid_mid_range() {
        assert_eq!(super::parse_antenna_val("-10", SENTINEL, RSRQ_MIN, RSRQ_MAX), Some(-10));
    }

    #[test]
    fn rsrq_below_range() {
        assert_eq!(super::parse_antenna_val("-21", SENTINEL, RSRQ_MIN, RSRQ_MAX), None);
    }

    #[test]
    fn rsrq_above_range() {
        assert_eq!(super::parse_antenna_val("-2", SENTINEL, RSRQ_MIN, RSRQ_MAX), None);
    }

    #[test]
    fn rsrq_sentinel() {
        assert_eq!(super::parse_antenna_val("-32768", SENTINEL, RSRQ_MIN, RSRQ_MAX), None);
    }

    // --- SINR (3GPP TS 36.214: -20 to +30 dB) ---

    #[test]
    fn sinr_valid_lower_boundary() {
        assert_eq!(super::parse_antenna_val("-20", SENTINEL, SINR_MIN, SINR_MAX), Some(-20));
    }

    #[test]
    fn sinr_valid_upper_boundary() {
        assert_eq!(super::parse_antenna_val("30", SENTINEL, SINR_MIN, SINR_MAX), Some(30));
    }

    #[test]
    fn sinr_valid_mid_range() {
        assert_eq!(super::parse_antenna_val("12", SENTINEL, SINR_MIN, SINR_MAX), Some(12));
    }

    #[test]
    fn sinr_below_range() {
        assert_eq!(super::parse_antenna_val("-21", SENTINEL, SINR_MIN, SINR_MAX), None);
    }

    #[test]
    fn sinr_above_range() {
        assert_eq!(super::parse_antenna_val("31", SENTINEL, SINR_MIN, SINR_MAX), None);
    }

    #[test]
    fn sinr_sentinel() {
        assert_eq!(super::parse_antenna_val("-32768", SENTINEL, SINR_MIN, SINR_MAX), None);
    }

    // --- Whitespace and non-numeric handling ---

    #[test]
    fn parse_with_whitespace() {
        assert_eq!(super::parse_antenna_val(" -84 ", SENTINEL, RSRP_MIN, RSRP_MAX), Some(-84));
        assert_eq!(super::parse_antenna_val(" 15 ", SENTINEL, SINR_MIN, SINR_MAX), Some(15));
        assert_eq!(super::parse_antenna_val(" -10 ", SENTINEL, RSRQ_MIN, RSRQ_MAX), Some(-10));
    }

    #[test]
    fn parse_non_numeric() {
        assert_eq!(super::parse_antenna_val("abc", SENTINEL, RSRP_MIN, RSRP_MAX), None);
        assert_eq!(super::parse_antenna_val("", SENTINEL, SINR_MIN, SINR_MAX), None);
        assert_eq!(super::parse_antenna_val("--5", SENTINEL, RSRQ_MIN, RSRQ_MAX), None);
    }

    // --- Response prefix derivation ---

    #[test]
    fn response_prefix_from_at_command() {
        assert_eq!(super::response_prefix_from_cmd("AT+QRSRP"), "+QRSRP:");
        assert_eq!(super::response_prefix_from_cmd("AT+QSINR"), "+QSINR:");
        assert_eq!(super::response_prefix_from_cmd("AT+QRSRQ"), "+QRSRQ:");
    }

    // --- Integration: full AT response parsing (unified parser) ---

    #[test]
    fn parse_rsrp_multi_with_sentinel() {
        let response = "+QRSRP: -84,-90,-32768,-32768,LTE\r\nOK\r\n";
        let rows = super::parse_antenna_metric_multi(response, "+QRSRP:", SENTINEL, RSRP_MIN, RSRP_MAX);
        assert_eq!(rows.len(), 1);
        let (tech, vals) = &rows[0];
        assert_eq!(tech, "LTE");
        assert_eq!(vals[0], Some(-84));
        assert_eq!(vals[1], Some(-90));
        assert_eq!(vals[2], None); // sentinel filtered
        assert_eq!(vals[3], None); // sentinel filtered
    }

    #[test]
    fn parse_sinr_multi_valid_range() {
        let response = "+QSINR: -20,15,30,-21,NR5G-NSA\r\nOK\r\n";
        let rows = super::parse_antenna_metric_multi(response, "+QSINR:", SENTINEL, SINR_MIN, SINR_MAX);
        assert_eq!(rows.len(), 1);
        let (tech, vals) = &rows[0];
        assert_eq!(tech, "NR5G-NSA");
        assert_eq!(vals[0], Some(-20)); // lower boundary valid
        assert_eq!(vals[1], Some(15));  // mid-range valid
        assert_eq!(vals[2], Some(30));  // upper boundary valid
        assert_eq!(vals[3], None);      // -21 out of range
    }

    #[test]
    fn parse_rsrq_multi_filters_invalid() {
        let response = "+QRSRQ: -10,-3,-50,-2,LTE\r\nOK\r\n";
        let rows = super::parse_antenna_metric_multi(response, "+QRSRQ:", SENTINEL, RSRQ_MIN, RSRQ_MAX);
        assert_eq!(rows.len(), 1);
        let (_, vals) = &rows[0];
        assert_eq!(vals[0], Some(-10)); // valid
        assert_eq!(vals[1], Some(-3));  // upper boundary valid
        assert_eq!(vals[2], None);      // -50 out of range
        assert_eq!(vals[3], None);      // -2 out of range
    }

    #[test]
    fn parse_multi_row_technologies() {
        let response = "+QRSRP: -84,-90,-95,-100,LTE\r\n+QRSRP: -88,-92,-32768,-32768,NR5G-NSA\r\nOK\r\n";
        let rows = super::parse_antenna_metric_multi(response, "+QRSRP:", SENTINEL, RSRP_MIN, RSRP_MAX);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0, "LTE");
        assert_eq!(rows[0].1[0], Some(-84));
        assert_eq!(rows[1].0, "NR5G-NSA");
        assert_eq!(rows[1].1[0], Some(-88));
        assert_eq!(rows[1].1[2], None); // sentinel
    }

    // =========================================================================
    // Carrier Aggregation parser tests
    // =========================================================================

    use super::super::profiles::BandPrefixMapping;

    fn quectel_band_mappings() -> Vec<BandPrefixMapping> {
        vec![
            BandPrefixMapping { prefix: "LTE BAND ".into(), replacement: "B".into() },
            BandPrefixMapping { prefix: "NR5G BAND ".into(), replacement: "n".into() },
        ]
    }

    fn quectel_lte_re() -> regex::Regex {
        regex::Regex::new(
            r#"\+QCAINFO:\s*"(PCC|SCC)",(\d+),(\d+),"(LTE[^"]*)",\d+,(\d+),(-?\d+),(-?\d+),(-?\d+),(-?\d+)"#
        ).unwrap()
    }

    fn quectel_nr5g_re() -> regex::Regex {
        regex::Regex::new(
            r#"\+QCAINFO:\s*"(PCC|SCC)",(\d+),(\d+),"(NR5G[^"]*)"(?:,(.+))?"#
        ).unwrap()
    }

    fn quectel_nwinfo_re() -> regex::Regex {
        regex::Regex::new(r#"\+QNWINFO:\s*"([^"]*)""#).unwrap()
    }

    #[test]
    fn parse_qcainfo_lte_scc() {
        let lte_re = quectel_lte_re();
        let mappings = quectel_band_mappings();
        let response = r#"+QCAINFO: "SCC",1850,50,"LTE BAND 3",2,100,-95,-12,-68,10"#;
        let cells = super::parse_qcainfo_secondary(response, Some(&lte_re), None, &mappings);
        assert_eq!(cells.len(), 1);
        assert_eq!(cells[0].band, "B3");
        assert_eq!(cells[0].rsrp, -95.0);
        assert_eq!(cells[0].rsrq, -12.0);
        assert_eq!(cells[0].rssi, -68.0);
        assert_eq!(cells[0].sinr, 10.0);
    }

    #[test]
    fn parse_qcainfo_nr5g_scc() {
        let nr_re = quectel_nr5g_re();
        let mappings = quectel_band_mappings();
        let response = r#"+QCAINFO: "SCC",627264,100,"NR5G BAND 78",2,500,-88,-9,15"#;
        let cells = super::parse_qcainfo_secondary(response, None, Some(&nr_re), &mappings);
        assert_eq!(cells.len(), 1);
        assert_eq!(cells[0].band, "n78");
        assert_eq!(cells[0].rsrp, -88.0);
        assert_eq!(cells[0].rsrq, -9.0);
        assert_eq!(cells[0].sinr, 15.0);
    }

    #[test]
    fn parse_qcainfo_pcc_lte_parsed() {
        let lte_re = quectel_lte_re();
        let mappings = quectel_band_mappings();
        let response = r#"+QCAINFO: "PCC",5330,50,"LTE BAND 14",4,325,-84,-12,-54,18"#;
        let pcc = super::parse_qcainfo_pcc(response, Some(&lte_re), None, &mappings);
        assert!(pcc.is_some(), "PCC line should be parsed");
        let pcc = pcc.unwrap();
        assert_eq!(pcc.band, "B14");
        assert_eq!(pcc.rsrp, -84.0);
        assert_eq!(pcc.rsrq, -12.0);
        assert_eq!(pcc.rssi, -54.0);
        assert_eq!(pcc.sinr, 18.0);
    }

    #[test]
    fn parse_qcainfo_pcc_still_excluded_from_secondary() {
        // Verify that parse_qcainfo_secondary still ignores PCC lines
        let lte_re = quectel_lte_re();
        let mappings = quectel_band_mappings();
        let response = r#"+QCAINFO: "PCC",5330,50,"LTE BAND 14",4,325,-84,-12,-54,18"#;
        let cells = super::parse_qcainfo_secondary(response, Some(&lte_re), None, &mappings);
        assert_eq!(cells.len(), 0, "PCC lines should still be ignored by secondary parser");
    }

    #[test]
    fn parse_qcainfo_mixed_lte_nr5g() {
        let lte_re = quectel_lte_re();
        let nr_re = quectel_nr5g_re();
        let mappings = quectel_band_mappings();
        let response = "+QCAINFO: \"PCC\",5330,50,\"LTE BAND 14\",4,325,-84,-12,-54,18\r\n\
                        +QCAINFO: \"SCC\",1850,50,\"LTE BAND 3\",2,100,-95,-12,-68,10\r\n\
                        +QCAINFO: \"SCC\",627264,100,\"NR5G BAND 78\",2,500,-88,-9,15\r\n";
        let cells = super::parse_qcainfo_secondary(response, Some(&lte_re), Some(&nr_re), &mappings);
        assert_eq!(cells.len(), 2);
        assert_eq!(cells[0].band, "B3");
        assert_eq!(cells[1].band, "n78");
    }

    #[test]
    fn parse_qnwinfo_type_extracts_technology() {
        let re = quectel_nwinfo_re();
        let response = r#"+QNWINFO: "FDD LTE","310410","LTE BAND 14",5330"#;
        assert_eq!(super::parse_qnwinfo_type(response, Some(&re)), Some("FDD LTE".to_string()));
    }

    #[test]
    fn parse_qnwinfo_type_none_when_no_regex() {
        assert_eq!(super::parse_qnwinfo_type("anything", None), None);
    }

    #[test]
    fn parse_qcainfo_no_regexes_returns_empty() {
        let response = r#"+QCAINFO: "SCC",1850,50,"LTE BAND 3",2,100,-95,-12,-68,10"#;
        let cells = super::parse_qcainfo_secondary(response, None, None, &[]);
        assert_eq!(cells.len(), 0);
    }

    #[test]
    fn normalize_band_name_applies_first_match() {
        let mappings = quectel_band_mappings();
        assert_eq!(super::normalize_band_name("LTE BAND 7", &mappings), "B7");
        assert_eq!(super::normalize_band_name("NR5G BAND 78", &mappings), "n78");
        assert_eq!(super::normalize_band_name("UNKNOWN BAND 1", &mappings), "UNKNOWN BAND 1");
    }

    // =========================================================================
    // PCC parsing tests
    // =========================================================================

    #[test]
    fn parse_qcainfo_pcc_nsa_mode_lte_pcc_nr5g_scc() {
        // NSA mode: QCAINFO has LTE PCC + NR5G SCC
        // PCC should be LTE, secondary should be NR5G
        let lte_re = quectel_lte_re();
        let nr_re = quectel_nr5g_re();
        let mappings = quectel_band_mappings();
        let response = "+QCAINFO: \"PCC\",5330,50,\"LTE BAND 2\",2,325,-85,-10,-54,15\r\n\
                        +QCAINFO: \"SCC\",627264,100,\"NR5G BAND 77\",2,500,-95,-10,12\r\n";

        let pcc = super::parse_qcainfo_pcc(response, Some(&lte_re), Some(&nr_re), &mappings);
        assert!(pcc.is_some(), "PCC should be found");
        let pcc = pcc.unwrap();
        assert_eq!(pcc.band, "B2", "PCC should be LTE Band 2");
        assert_eq!(pcc.rsrp, -85.0);
        assert_eq!(pcc.rsrq, -10.0);
        assert_eq!(pcc.rssi, -54.0);
        assert_eq!(pcc.sinr, 15.0);

        let sccs = super::parse_qcainfo_secondary(response, Some(&lte_re), Some(&nr_re), &mappings);
        assert_eq!(sccs.len(), 1, "Should have 1 SCC");
        assert_eq!(sccs[0].band, "n77", "SCC should be NR5G Band 77");
    }

    #[test]
    fn parse_qcainfo_pcc_sa_mode_nr5g_pcc_only() {
        // SA mode: QCAINFO with NR5G PCC only (no SCCs)
        let lte_re = quectel_lte_re();
        let nr_re = quectel_nr5g_re();
        let mappings = quectel_band_mappings();
        let response = r#"+QCAINFO: "PCC",627264,100,"NR5G BAND 77",2,500,-88,-9,15"#;

        let pcc = super::parse_qcainfo_pcc(response, Some(&lte_re), Some(&nr_re), &mappings);
        assert!(pcc.is_some(), "PCC should be found");
        let pcc = pcc.unwrap();
        assert_eq!(pcc.band, "n77", "PCC should be NR5G Band 77");
        assert_eq!(pcc.rsrp, -88.0);
        assert_eq!(pcc.rsrq, -9.0);
        assert_eq!(pcc.sinr, 15.0);

        let sccs = super::parse_qcainfo_secondary(response, Some(&lte_re), Some(&nr_re), &mappings);
        assert_eq!(sccs.len(), 0, "Should have no SCCs");
    }

    #[test]
    fn parse_qcainfo_pcc_lte_only_ca() {
        // LTE-only CA: QCAINFO with LTE PCC + LTE SCCs
        let lte_re = quectel_lte_re();
        let mappings = quectel_band_mappings();
        let response = "+QCAINFO: \"PCC\",5330,50,\"LTE BAND 2\",2,325,-85,-10,-54,15\r\n\
                        +QCAINFO: \"SCC\",3450,50,\"LTE BAND 7\",2,100,-92,-11,-65,12\r\n\
                        +QCAINFO: \"SCC\",66486,50,\"LTE BAND 66\",2,200,-90,-9,-62,14\r\n";

        let pcc = super::parse_qcainfo_pcc(response, Some(&lte_re), None, &mappings);
        assert!(pcc.is_some(), "PCC should be found");
        let pcc = pcc.unwrap();
        assert_eq!(pcc.band, "B2", "PCC should be LTE Band 2");
        assert_eq!(pcc.rsrp, -85.0);

        let sccs = super::parse_qcainfo_secondary(response, Some(&lte_re), None, &mappings);
        assert_eq!(sccs.len(), 2, "Should have 2 SCCs");
        assert_eq!(sccs[0].band, "B7", "First SCC should be LTE Band 7");
        assert_eq!(sccs[1].band, "B66", "Second SCC should be LTE Band 66");
    }

    #[test]
    fn parse_qcainfo_pcc_no_pcc_line_returns_none() {
        // No PCC line in QCAINFO — fallback to get_signal() should apply
        let lte_re = quectel_lte_re();
        let mappings = quectel_band_mappings();
        let response = "+QCAINFO: \"SCC\",3450,50,\"LTE BAND 7\",2,100,-92,-11,-65,12\r\n";

        let pcc = super::parse_qcainfo_pcc(response, Some(&lte_re), None, &mappings);
        assert!(pcc.is_none(), "Should return None when no PCC line is present");
    }

    #[test]
    fn parse_qcainfo_pcc_no_regexes_returns_none() {
        let response = r#"+QCAINFO: "PCC",5330,50,"LTE BAND 14",4,325,-84,-12,-54,18"#;
        let pcc = super::parse_qcainfo_pcc(response, None, None, &[]);
        assert!(pcc.is_none(), "Should return None when no regexes are provided");
    }

    // =========================================================================
    // Signal variant-matching engine tests
    // =========================================================================

    use super::super::profiles::SignalFormatVariant;

    /// Build the Quectel RM551E-GL signal variant list (compile all 4 variant regexes).
    fn quectel_signal_variants() -> Vec<(SignalFormatVariant, regex::Regex)> {
        let variants = vec![
            SignalFormatVariant {
                label: "NR5G-NSA 3-line".into(),
                requires_substring: "NR5G-NSA".into(),
                regex: r#"\+QENG:\s*"LTE","[^"]*",\d+,\d+,(?P<cellid>[0-9A-Fa-f]+),\d+,\d+,(?P<band>\d+),\d+,\d+,[0-9A-Fa-f]+,(?P<rsrp>-?\d+),(?P<rsrq>-?\d+),(?P<rssi>-?\d+),(?P<sinr>-?\d+)"#.into(),
                band_prefix: "B".into(),
                technology: "4G".into(),
            },
            SignalFormatVariant {
                label: "LTE single-line".into(),
                requires_substring: String::new(),
                regex: r#"\+QENG:\s*"servingcell","[^"]*","LTE","[^"]*",\d+,\d+,(?P<cellid>[0-9A-Fa-f]+),\d+,(?P<earfcn>\d+),(?P<band>\d+),\d+,\d+,[0-9A-Fa-f]+,(?P<rsrp>-?\d+),(?P<rsrq>-?\d+),(?P<rssi>-?\d+),(?P<sinr>-?\d+)"#.into(),
                band_prefix: "B".into(),
                technology: "4G".into(),
            },
            SignalFormatVariant {
                label: "LTE 2-line".into(),
                requires_substring: String::new(),
                regex: r#"\+QENG:\s*"LTE","[^"]*",\d+,\d+,(?P<cellid>[0-9A-Fa-f]+),\d+,(?P<earfcn>\d+),(?P<band>\d+),\d+,\d+,[0-9A-Fa-f]+,(?P<rsrp>-?\d+),(?P<rsrq>-?\d+),(?P<rssi>-?\d+),(?P<sinr>-?\d+)"#.into(),
                band_prefix: "B".into(),
                technology: "4G".into(),
            },
            SignalFormatVariant {
                label: "NR5G-SA single-line".into(),
                requires_substring: "NR5G-SA".into(),
                regex: r#"\+QENG:\s*"servingcell","[^"]*","NR5G-SA","[^"]*",\d+,\d+,(?P<cellid>[0-9A-Fa-f]+),[0-9A-Fa-f]+,(?:[0-9A-Fa-f]+,)*(?P<band>\d+),\d+,(?P<rsrp>-?\d+),(?P<rsrq>-?\d+),(?P<sinr>-?\d+)"#.into(),
                band_prefix: "n".into(),
                technology: "5G".into(),
            },
        ];
        variants
            .into_iter()
            .map(|v| {
                let re = regex::Regex::new(&v.regex).expect("variant regex should compile");
                (v, re)
            })
            .collect()
    }

    #[cfg(feature = "real-hardware")]
    use super::real_hardware::AtHandler;

    #[test]
    fn variant_engine_nsa_returns_lte_anchor() {
        let variants = quectel_signal_variants();
        let response = "+QENG: \"servingcell\",\"NOCONN\"\r\n\
            +QENG: \"LTE\",\"FDD\",310,410,29401C0,325,5330,14,3,3,2294,-84,-12,-54,18\r\n\
            +QENG: \"NR5G-NSA\",310,410,500,-95,12,-10,627264,77,100,1\r\n\
            OK\r\n";

        #[cfg(feature = "real-hardware")]
        let result = AtHandler::parse_signal_response(response, &variants);
        #[cfg(not(feature = "real-hardware"))]
        let result = parse_signal_response_standalone(response, &variants);

        let info = result.expect("NSA response should match variant 1");
        assert_eq!(info.band, "B14", "NSA PCC rule: LTE is primary, band should be B14");
        assert_eq!(info.rsrp, -84.0);
        assert_eq!(info.rsrq, -12.0);
        assert_eq!(info.rssi, -54.0);
        assert_eq!(info.sinr, 18.0);
        assert_eq!(info.cell_id, "29401C0");
        assert!(matches!(info.technology, Some(super::Technology::Gen4)),
            "NSA PCC should report Gen4 (LTE anchor)");
    }

    #[test]
    fn variant_engine_lte_single_line() {
        let variants = quectel_signal_variants();
        let response = r#"+QENG: "servingcell","NOCONN","LTE","FDD",310,410,29401C0,325,5330,14,3,3,2294,-84,-12,-54,18,12,30,-"#;

        #[cfg(feature = "real-hardware")]
        let result = AtHandler::parse_signal_response(response, &variants);
        #[cfg(not(feature = "real-hardware"))]
        let result = parse_signal_response_standalone(response, &variants);

        let info = result.expect("Single-line LTE should match variant 2");
        assert_eq!(info.band, "B14");
        assert_eq!(info.rsrp, -84.0);
        assert_eq!(info.rsrq, -12.0);
        assert_eq!(info.rssi, -54.0);
        assert_eq!(info.sinr, 18.0);
        assert!(matches!(info.technology, Some(super::Technology::Gen4)));
    }

    #[test]
    fn variant_engine_lte_two_line() {
        let variants = quectel_signal_variants();
        let response = "+QENG: \"servingcell\",\"NOCONN\"\r\n\
            +QENG: \"LTE\",\"FDD\",310,410,29401C0,325,5330,14,3,3,2294,-84,-12,-54,18\r\n\
            OK\r\n";

        #[cfg(feature = "real-hardware")]
        let result = AtHandler::parse_signal_response(response, &variants);
        #[cfg(not(feature = "real-hardware"))]
        let result = parse_signal_response_standalone(response, &variants);

        let info = result.expect("2-line LTE should match variant 3");
        assert_eq!(info.band, "B14");
        assert_eq!(info.rsrp, -84.0);
        assert!(matches!(info.technology, Some(super::Technology::Gen4)));
    }

    #[test]
    fn variant_engine_nr5g_sa() {
        let variants = quectel_signal_variants();
        let response = r#"+QENG: "servingcell","NOCONN","NR5G-SA","TDD",310,410,1A2B3C4,100,627264,77,100,-88,-9,15"#;

        #[cfg(feature = "real-hardware")]
        let result = AtHandler::parse_signal_response(response, &variants);
        #[cfg(not(feature = "real-hardware"))]
        let result = parse_signal_response_standalone(response, &variants);

        let info = result.expect("NR5G-SA should match variant 4");
        assert_eq!(info.band, "n77", "SA band should use 'n' prefix");
        assert_eq!(info.rsrp, -88.0);
        assert_eq!(info.rsrq, -9.0);
        assert_eq!(info.sinr, 15.0);
        assert_eq!(info.cell_id, "1A2B3C4");
        assert!(matches!(info.technology, Some(super::Technology::Gen5)),
            "SA should report Gen5");
    }

    /// Regression test: RM520N-GL firmware emits hex ARFCN and extra fields in
    /// NR5G-SA servingcell response. The regex must tolerate hex values and
    /// variable field counts between cell ID and the signal metrics.
    #[test]
    fn variant_engine_nr5g_sa_hex_arfcn() {
        let variants = quectel_signal_variants();
        // Actual RM520N-GL response: field 9 (ABFB00) is hex ARFCN, and there
        // is an extra field (387410) between ARFCN and band compared to the
        // standard format.
        let response = r#"+QENG: "servingcell","NOCONN","NR5G-SA","FDD",310,260,10244F016,823,ABFB00,387410,25,2,-106,-12,3,0,11"#;

        #[cfg(feature = "real-hardware")]
        let result = AtHandler::parse_signal_response(response, &variants);
        #[cfg(not(feature = "real-hardware"))]
        let result = parse_signal_response_standalone(response, &variants);

        let info = result.expect("NR5G-SA with hex ARFCN should match variant 4");
        assert_eq!(info.cell_id, "10244F016", "cell ID should be parsed from hex field");
        assert_eq!(info.band, "n25", "SA band should use 'n' prefix");
        assert_eq!(info.rsrp, -106.0);
        assert_eq!(info.rsrq, -12.0);
        assert_eq!(info.sinr, 3.0);
        assert!(matches!(info.technology, Some(super::Technology::Gen5)),
            "SA should report Gen5");
    }

    #[test]
    fn variant_engine_garbage_returns_none() {
        let variants = quectel_signal_variants();
        let response = "OK\r\nERROR\r\ngarbage text\r\n";

        #[cfg(feature = "real-hardware")]
        let result = AtHandler::parse_signal_response(response, &variants);
        #[cfg(not(feature = "real-hardware"))]
        let result = parse_signal_response_standalone(response, &variants);

        assert!(result.is_none(), "Garbage response should not match any variant");
    }

    #[test]
    fn variant_engine_empty_variants_returns_none() {
        let variants: Vec<(SignalFormatVariant, regex::Regex)> = vec![];
        let response = r#"+QENG: "servingcell","NOCONN","LTE","FDD",310,410,29401C0,325,5330,14,3,3,2294,-84,-12,-54,18"#;

        #[cfg(feature = "real-hardware")]
        let result = AtHandler::parse_signal_response(response, &variants);
        #[cfg(not(feature = "real-hardware"))]
        let result = parse_signal_response_standalone(response, &variants);

        assert!(result.is_none(), "Empty variant list should return None (CSQ fallback)");
    }

    // =========================================================================
    // Telit FN990 AT#RFSTS signal parsing tests
    // =========================================================================

    /// Build Telit FN990 signal variants matching the profile in profiles.rs.
    fn telit_signal_variants() -> Vec<(SignalFormatVariant, regex::Regex)> {
        let variants = vec![
            SignalFormatVariant {
                label: "Telit RFSTS LTE".into(),
                requires_substring: "#RFSTS:".into(),
                regex: concat!(
                    r#"#RFSTS:\s*"[^"]*","#,
                    r"(?P<earfcn>\d+),",
                    r"(?P<rsrp>-?\d+),",
                    r"(?P<rssi>-?\d+),",
                    r"(?P<rsrq>-?\d+),",
                    r"[0-9A-Fa-f]+,",
                    r"[0-9A-Fa-f]+,",
                    r"-?\d*,",
                    r"\d+,",
                    r"\d+,",
                    r"\d+,",
                    r"(?P<cellid>[0-9A-Fa-f]+),",
                    r#""[^"]*","[^"]*","#,
                    r"\d+,",
                    r"(?P<band>\d+)",
                ).into(),
                band_prefix: "B".into(),
                technology: "4G".into(),
            },
            SignalFormatVariant {
                label: "Telit RFSTS NR SA".into(),
                requires_substring: "#RFSTS:".into(),
                regex: concat!(
                    r#"#RFSTS:\s*"[^"]*","#,
                    r"\d+,",
                    r"\d+,",
                    r"(?P<rsrp>-?\d+),",
                    r"(?P<rssi>-?\d+),",
                    r"(?P<rsrq>-?\d+),",
                    r"(?P<band>\d+),",
                    r"\d+,",
                    r"\d+",
                ).into(),
                band_prefix: "n".into(),
                technology: "5G".into(),
            },
        ];
        variants
            .into_iter()
            .map(|v| {
                let re = regex::Regex::new(&v.regex).expect("Telit variant regex should compile");
                (v, re)
            })
            .collect()
    }

    #[test]
    fn telit_rfsts_lte_only() {
        let variants = telit_signal_variants();
        let response = r#"#RFSTS: "310 260",66811,-116,-80,-16,ABFB,FF,,1280,3,0,07BAB02,"310260228115983","T-Mobile",3,66"#;

        #[cfg(feature = "real-hardware")]
        let result = AtHandler::parse_signal_response(response, &variants);
        #[cfg(not(feature = "real-hardware"))]
        let result = parse_signal_response_standalone(response, &variants);

        let info = result.expect("LTE-only RFSTS should match variant 1");
        assert_eq!(info.band, "B66", "Band should be B66 (ABND=66)");
        assert_eq!(info.rsrp, -116.0);
        assert_eq!(info.rssi, -80.0);
        assert_eq!(info.rsrq, -16.0);
        assert_eq!(info.cell_id, "07BAB02");
        assert!(matches!(info.technology, Some(super::Technology::Gen4)),
            "LTE should report Gen4");
    }

    #[test]
    fn telit_rfsts_endc_returns_lte_primary() {
        let variants = telit_signal_variants();
        // ENDC (NSA) response: LTE fields + NR fields appended
        let response = r#"#RFSTS: "310 260",66811,-114,-78,-16,ABFB,255,23,1280,19,2,07BAB02,"310260228115983","T-Mobile",3,66,501390,501390,-111,-96,-15,41,90,90,0"#;

        #[cfg(feature = "real-hardware")]
        let result = AtHandler::parse_signal_response(response, &variants);
        #[cfg(not(feature = "real-hardware"))]
        let result = parse_signal_response_standalone(response, &variants);

        let info = result.expect("ENDC RFSTS should match LTE variant (variant 1)");
        assert_eq!(info.band, "B66", "NSA PCC rule: LTE is primary, band=B66");
        assert_eq!(info.rsrp, -114.0);
        assert_eq!(info.rssi, -78.0);
        assert_eq!(info.rsrq, -16.0);
        assert_eq!(info.cell_id, "07BAB02");
        assert!(matches!(info.technology, Some(super::Technology::Gen4)),
            "NSA PCC should report Gen4 (LTE anchor)");
    }

    #[test]
    fn telit_rfsts_nr_sa() {
        let variants = telit_signal_variants();
        // NR SA response: no LTE fields, just NR
        let response = r#"#RFSTS: "310 260",501390,501390,-95,-80,-10,41,100,100"#;

        #[cfg(feature = "real-hardware")]
        let result = AtHandler::parse_signal_response(response, &variants);
        #[cfg(not(feature = "real-hardware"))]
        let result = parse_signal_response_standalone(response, &variants);

        let info = result.expect("NR SA RFSTS should match variant 2");
        assert_eq!(info.band, "n41", "SA band should use 'n' prefix");
        assert_eq!(info.rsrp, -95.0);
        assert_eq!(info.rssi, -80.0);
        assert_eq!(info.rsrq, -10.0);
        assert!(matches!(info.technology, Some(super::Technology::Gen5)),
            "SA should report Gen5");
    }

    #[test]
    fn telit_rfsts_lte_in_full_at_response() {
        let variants = telit_signal_variants();
        // Simulates send_command() output with echo + OK
        let response = "AT#RFSTS\r\n\
            #RFSTS: \"310 260\",66811,-116,-80,-16,ABFB,FF,,1280,3,0,07BAB02,\"310260228115983\",\"T-Mobile\",3,66\r\n\
            OK\r\n";

        #[cfg(feature = "real-hardware")]
        let result = AtHandler::parse_signal_response(response, &variants);
        #[cfg(not(feature = "real-hardware"))]
        let result = parse_signal_response_standalone(response, &variants);

        let info = result.expect("Should match inside full AT response with echo/OK");
        assert_eq!(info.band, "B66");
        assert_eq!(info.rsrp, -116.0);
    }

    #[test]
    fn telit_rfsts_nr_sa_does_not_match_lte_variant() {
        // NR SA format has no quoted IMSI/NetName, so LTE variant should fail
        let lte_regex = regex::Regex::new(
            concat!(
                r#"#RFSTS:\s*"[^"]*","#,
                r"(?P<earfcn>\d+),",
                r"(?P<rsrp>-?\d+),",
                r"(?P<rssi>-?\d+),",
                r"(?P<rsrq>-?\d+),",
                r"[0-9A-Fa-f]+,",
                r"[0-9A-Fa-f]+,",
                r"-?\d*,",
                r"\d+,",
                r"\d+,",
                r"\d+,",
                r"(?P<cellid>[0-9A-Fa-f]+),",
                r#""[^"]*","[^"]*","#,
                r"\d+,",
                r"(?P<band>\d+)",
            )
        ).unwrap();
        let nr_sa_response = r#"#RFSTS: "310 260",501390,501390,-95,-80,-10,41,100,100"#;

        assert!(lte_regex.captures(nr_sa_response).is_none(),
            "LTE variant must NOT match NR SA response (no quoted IMSI/NetName)");
    }

    /// Standalone variant-matching engine for test use when real-hardware feature is not enabled.
    /// Mirrors the logic of AtHandler::parse_signal_response exactly.
    #[cfg(not(feature = "real-hardware"))]
    fn parse_signal_response_standalone(
        response: &str,
        signal_variants: &[(SignalFormatVariant, regex::Regex)],
    ) -> Option<super::SignalInfo> {
        for (variant, re) in signal_variants {
            if !variant.requires_substring.is_empty()
                && !response.contains(&variant.requires_substring)
            {
                continue;
            }
            if let Some(caps) = re.captures(response) {
                let technology = match variant.technology.as_str() {
                    "5G" => Some(super::Technology::Gen5),
                    _ => Some(super::Technology::Gen4),
                };
                return Some(super::SignalInfo {
                    rsrp: caps.name("rsrp").and_then(|m| m.as_str().parse::<f64>().ok()).unwrap_or(-999.0),
                    rsrq: caps.name("rsrq").and_then(|m| m.as_str().parse::<f64>().ok()).unwrap_or(-999.0),
                    rssi: caps.name("rssi").and_then(|m| m.as_str().parse::<f64>().ok()).unwrap_or(-999.0),
                    sinr: caps.name("sinr").and_then(|m| m.as_str().parse::<f64>().ok()).unwrap_or(0.0),
                    band: caps.name("band")
                        .map(|m| format!("{}{}", variant.band_prefix, m.as_str()))
                        .unwrap_or_default(),
                    cell_id: caps.name("cellid")
                        .map(|m| m.as_str().to_string())
                        .unwrap_or_default(),
                    technology,
                });
            }
        }
        None
    }

    // =========================================================================
    // Telit interleaved antenna metrics (AT#LAPS) tests
    // =========================================================================

    // Telit uses rsrq_max=0 (wider range than Quectel's -3)
    const TELIT_RSRQ_MIN: i32 = -20;
    const TELIT_RSRQ_MAX: i32 = 0;

    #[test]
    fn telit_laps_basic_2rx() {
        // AT#LAPS returns interleaved RSRP/RSRQ for 2 RX ports
        let response = "#LAPS: -114,-12,-109,-11\r\nOK\r\n";
        let (rsrp, rsrq) = super::parse_antenna_interleaved(
            response, "#LAPS:", SENTINEL, RSRP_MIN, RSRP_MAX, TELIT_RSRQ_MIN, TELIT_RSRQ_MAX,
        );
        assert_eq!(rsrp[0], Some(-114));
        assert_eq!(rsrq[0], Some(-12));
        assert_eq!(rsrp[1], Some(-109));
        assert_eq!(rsrq[1], Some(-11));
        assert_eq!(rsrp[2], None); // No 3rd port
        assert_eq!(rsrq[2], None);
        assert_eq!(rsrp[3], None); // No 4th port
        assert_eq!(rsrq[3], None);
    }

    #[test]
    fn telit_laps_4rx() {
        // AT#LAPS with 4 RX antenna ports
        let response = "#LAPS: -100,-8,-105,-10,-110,-14,-115,-18\r\nOK\r\n";
        let (rsrp, rsrq) = super::parse_antenna_interleaved(
            response, "#LAPS:", SENTINEL, RSRP_MIN, RSRP_MAX, TELIT_RSRQ_MIN, TELIT_RSRQ_MAX,
        );
        assert_eq!(rsrp[0], Some(-100));
        assert_eq!(rsrq[0], Some(-8));
        assert_eq!(rsrp[1], Some(-105));
        assert_eq!(rsrq[1], Some(-10));
        assert_eq!(rsrp[2], Some(-110));
        assert_eq!(rsrq[2], Some(-14));
        assert_eq!(rsrp[3], Some(-115));
        assert_eq!(rsrq[3], Some(-18));
    }

    #[test]
    fn telit_laps_with_sentinel_values() {
        // Some ports may report sentinel values
        let response = "#LAPS: -114,-12,-32768,-32768\r\nOK\r\n";
        let (rsrp, rsrq) = super::parse_antenna_interleaved(
            response, "#LAPS:", SENTINEL, RSRP_MIN, RSRP_MAX, TELIT_RSRQ_MIN, TELIT_RSRQ_MAX,
        );
        assert_eq!(rsrp[0], Some(-114));
        assert_eq!(rsrq[0], Some(-12));
        assert_eq!(rsrp[1], None); // Sentinel filtered
        assert_eq!(rsrq[1], None); // Sentinel filtered
    }

    #[test]
    fn telit_laps_out_of_range() {
        // RSRP out of range should be filtered
        let response = "#LAPS: -150,-25,-44,0\r\nOK\r\n";
        let (rsrp, rsrq) = super::parse_antenna_interleaved(
            response, "#LAPS:", SENTINEL, RSRP_MIN, RSRP_MAX, TELIT_RSRQ_MIN, TELIT_RSRQ_MAX,
        );
        assert_eq!(rsrp[0], None); // -150 below RSRP_MIN
        assert_eq!(rsrq[0], None); // -25 below TELIT_RSRQ_MIN (-20)
        assert_eq!(rsrp[1], Some(-44)); // Upper boundary valid
        assert_eq!(rsrq[1], Some(0)); // Upper boundary valid (TELIT_RSRQ_MAX=0)
    }

    #[test]
    fn telit_nraps_prefix() {
        // AT#NRAPS uses #NRAPS: prefix
        let response = "#NRAPS: -95,-9,-100,-12\r\nOK\r\n";
        let (rsrp, rsrq) = super::parse_antenna_interleaved(
            response, "#NRAPS:", SENTINEL, RSRP_MIN, RSRP_MAX, TELIT_RSRQ_MIN, TELIT_RSRQ_MAX,
        );
        assert_eq!(rsrp[0], Some(-95));
        assert_eq!(rsrq[0], Some(-9));
        assert_eq!(rsrp[1], Some(-100));
        assert_eq!(rsrq[1], Some(-12));
    }

    #[test]
    fn telit_laps_empty_response() {
        // Empty response or no matching prefix
        let response = "OK\r\n";
        let (rsrp, rsrq) = super::parse_antenna_interleaved(
            response, "#LAPS:", SENTINEL, RSRP_MIN, RSRP_MAX, TELIT_RSRQ_MIN, TELIT_RSRQ_MAX,
        );
        for i in 0..4 {
            assert_eq!(rsrp[i], None);
            assert_eq!(rsrq[i], None);
        }
    }

    #[test]
    fn response_prefix_from_telit_commands() {
        assert_eq!(super::response_prefix_from_cmd("AT#LAPS"), "#LAPS:");
        assert_eq!(super::response_prefix_from_cmd("AT#NRAPS"), "#NRAPS:");
        assert_eq!(super::response_prefix_from_cmd("AT#CAINFO?"), "#CAINFO?:");
    }

    // =========================================================================
    // Telit CA parser (AT#CAINFO?) tests
    // =========================================================================

    #[test]
    fn telit_band_class_lte() {
        let (name, tech) = super::telit_band_class_to_name(121);
        assert_eq!(name, "B2");
        assert_eq!(tech, Some(super::Technology::Gen4));
    }

    #[test]
    fn telit_band_class_nr() {
        let (name, tech) = super::telit_band_class_to_name(290);
        assert_eq!(name, "n41");
        assert_eq!(tech, Some(super::Technology::Gen5));
    }

    #[test]
    fn telit_band_class_b1() {
        let (name, tech) = super::telit_band_class_to_name(120);
        assert_eq!(name, "B1");
        assert_eq!(tech, Some(super::Technology::Gen4));
    }

    #[test]
    fn telit_band_class_n1() {
        let (name, tech) = super::telit_band_class_to_name(250);
        assert_eq!(name, "n1");
        assert_eq!(tech, Some(super::Technology::Gen5));
    }

    #[test]
    fn telit_sinr_conversion() {
        assert!((super::telit_sinr_to_db(0) - (-20.0)).abs() < 0.01);
        assert!((super::telit_sinr_to_db(100) - 0.0).abs() < 0.01);
        assert!((super::telit_sinr_to_db(250) - 30.0).abs() < 0.01);
        assert!((super::telit_sinr_to_db(105) - 1.0).abs() < 0.01);
    }

    #[test]
    fn telit_cainfo_pcc_only() {
        // Single PCC line from AT#CAINFO?
        let response = "#CAINFO: 121,1079,3,61,-110,-76,-16,105,abfb,,0,0,0\r\nOK\r\n";
        let (pcc, sccs) = super::parse_telit_cainfo(response);
        assert!(pcc.is_some(), "PCC should be found");
        let pcc = pcc.unwrap();
        assert_eq!(pcc.band, "B2"); // 121 - 119 = 2
        assert_eq!(pcc.rsrp, -110.0);
        assert_eq!(pcc.rssi, -76.0);
        assert_eq!(pcc.rsrq, -16.0);
        assert!((pcc.sinr - 1.0).abs() < 0.01); // 105 * 0.2 - 20 = 1.0
        assert_eq!(pcc.cell_id, "61");
        assert_eq!(sccs.len(), 0, "No SCCs");
    }

    #[test]
    fn telit_cainfo_pcc_with_lte_scc() {
        // PCC + one LTE SCC
        let response = "#CAINFO: 121,1079,3,61,-110,-76,-16,105,abfb,,0,0,0\r\n\
                        #CAINFO: 185,66486,3,100,-95,-68,-12,120,1,0,0,0\r\n\
                        OK\r\n";
        let (pcc, sccs) = super::parse_telit_cainfo(response);
        assert!(pcc.is_some());
        let pcc = pcc.unwrap();
        assert_eq!(pcc.band, "B2");

        assert_eq!(sccs.len(), 1);
        assert_eq!(sccs[0].band, "B66"); // 185 - 119 = 66
        assert_eq!(sccs[0].rsrp, -95.0);
        assert_eq!(sccs[0].rsrq, -12.0);
    }

    #[test]
    fn telit_cainfo_zero_filled_sccs_filtered() {
        // Telit pads extra SCC lines with zeros — these should be filtered out
        let response = "#CAINFO: 121,1079,3,61,-110,-76,-16,105,abfb,,0,0,0\r\n\
                        #CAINFO: 0,0,0,0,0,0,0,0,0,0,0,0\r\n\
                        #CAINFO: 0,0,0,0,0,0,0,0,0,0,0,0\r\n\
                        OK\r\n";
        let (pcc, sccs) = super::parse_telit_cainfo(response);
        assert!(pcc.is_some());
        assert_eq!(sccs.len(), 0, "Zero-filled SCC lines should be filtered");
    }

    #[test]
    fn telit_cainfo_with_endc() {
        // PCC (LTE) + ENDC line (NR5G secondary)
        let response = "#CAINFO: 121,1079,3,61,-110,-76,-16,105,abfb,,0,0,0\r\n\
                        ENDC: 327,627264,100,500,-88,-60,-9,150,0,0,0,0\r\n\
                        OK\r\n";
        let (pcc, sccs) = super::parse_telit_cainfo(response);
        assert!(pcc.is_some());
        let pcc = pcc.unwrap();
        assert_eq!(pcc.band, "B2");

        assert_eq!(sccs.len(), 1);
        assert_eq!(sccs[0].band, "n78"); // band_class 327 for ENDC: max(327, 250) = 327, 327-249 = 78
        assert_eq!(sccs[0].rsrp, -88.0);
        assert_eq!(sccs[0].rsrq, -9.0);
        assert_eq!(sccs[0].technology, Some(super::Technology::Gen5));
    }

    #[test]
    fn telit_cainfo_empty_response() {
        let response = "OK\r\n";
        let (pcc, sccs) = super::parse_telit_cainfo(response);
        assert!(pcc.is_none());
        assert_eq!(sccs.len(), 0);
    }

    #[test]
    fn telit_cainfo_multiple_lte_sccs() {
        // PCC + two LTE SCCs
        let response = "#CAINFO: 121,1079,3,61,-110,-76,-16,105,abfb,,0,0,0\r\n\
                        #CAINFO: 122,1500,5,200,-100,-70,-10,110,1,0,0,0\r\n\
                        #CAINFO: 159,5230,3,300,-105,-72,-14,100,1,0,0,0\r\n\
                        OK\r\n";
        let (pcc, sccs) = super::parse_telit_cainfo(response);
        assert!(pcc.is_some());
        assert_eq!(pcc.unwrap().band, "B2"); // 121-119=2
        assert_eq!(sccs.len(), 2);
        assert_eq!(sccs[0].band, "B3"); // 122-119=3
        assert_eq!(sccs[1].band, "B40"); // 159-119=40
    }

    // =========================================================================
    // Telit FN990 GPS ($GPSACP) Tests
    // =========================================================================

    /// The Telit FN990 profile regex for $GPSACP GPS position responses.
    /// Captures N/S and E/W hemisphere indicators as named groups.
    fn telit_gps_regex() -> regex::Regex {
        regex::Regex::new(
            r"\$GPSACP:\s*(?P<time>\d+\.\d+),(?P<lat>\d+\.\d+),(?P<ns>[NS]),(?P<lon>\d+\.\d+),(?P<ew>[EW]),[\d.]*,(?P<alt>[\d.]+),(?P<fix>\d+),[\d.]*,(?P<speed>[\d.]+),[\d.]*,(?P<date>\d+),(?P<satellites>\d+)"
        ).unwrap()
    }

    #[test]
    fn telit_gpsacp_valid_fix_matches() {
        let re = telit_gps_regex();
        // Full $GPSACP: UTC,lat,N/S,lon,E/W,hdop,alt,fix,cog,spkm,spkn,date,nsat
        let response = "$GPSACP: 161229.487,3723.2475,N,12158.3416,W,1.0,161.5,2,227.08,0.78,0.42,130998,12";

        let caps = re.captures(response).expect("regex should match valid $GPSACP with fix");
        assert_eq!(cap_str(&caps, "time"), "161229.487");
        assert_eq!(cap_f64(&caps, "lat"), 3723.2475);  // raw NMEA format
        assert_eq!(cap_str(&caps, "ns"), "N");
        assert_eq!(cap_f64(&caps, "lon"), 12158.3416);  // raw NMEA format
        assert_eq!(cap_str(&caps, "ew"), "W");
        assert_eq!(cap_f64(&caps, "alt"), 161.5);
        assert_eq!(cap_str(&caps, "fix"), "2");
        assert_eq!(cap_f64(&caps, "speed"), 0.78);
        assert_eq!(cap_str(&caps, "date"), "130998");
        assert_eq!(cap_str(&caps, "satellites"), "12");
    }

    #[test]
    fn telit_gpsacp_no_fix_does_not_match() {
        let re = telit_gps_regex();
        // Telit returns empty fields when there is no GPS fix
        let response = "$GPSACP: ,,,,,1,,,,,";

        let caps = re.captures(response);
        assert!(caps.is_none(), "regex should NOT match no-fix $GPSACP response with empty fields");
    }

    #[test]
    fn nmea_to_decimal_latitude() {
        use super::nmea_to_decimal;
        // 3723.2475 → 37 + (23.2475 / 60) = 37.387458...
        let result = nmea_to_decimal(3723.2475);
        assert!((result - 37.387458).abs() < 0.000001,
            "3723.2475 should convert to ~37.387458, got {}", result);
    }

    #[test]
    fn nmea_to_decimal_longitude() {
        use super::nmea_to_decimal;
        // 12158.3416 → 121 + (58.3416 / 60) = 121.972360
        let result = nmea_to_decimal(12158.3416);
        assert!((result - 121.972360).abs() < 0.000001,
            "12158.3416 should convert to ~121.972360, got {}", result);
    }

    #[test]
    fn telit_gpsacp_nmea_regex_captures_hemisphere() {
        let re = telit_gps_regex();
        let response = "$GPSACP: 161229.487,3723.2475,N,12158.3416,W,1.0,161.5,2,227.08,0.78,0.42,130998,12";

        let caps = re.captures(response).expect("regex should match");
        // Verify hemisphere groups are captured
        assert_eq!(cap_str(&caps, "ns"), "N");
        assert_eq!(cap_str(&caps, "ew"), "W");

        // Verify NMEA→decimal conversion with hemisphere applied
        use super::nmea_to_decimal;
        let lat = nmea_to_decimal(cap_f64(&caps, "lat"));
        let lon = nmea_to_decimal(cap_f64(&caps, "lon"));
        let ns = cap_str(&caps, "ns");
        let ew = cap_str(&caps, "ew");
        let lat_signed = if ns == "S" { -lat } else { lat };
        let lon_signed = if ew == "W" { -lon } else { lon };

        // 3723.2475,N → 37.387458
        assert!((lat_signed - 37.387458).abs() < 0.000001,
            "latitude should be ~37.387458, got {}", lat_signed);
        // 12158.3416,W → -121.972360
        assert!((lon_signed - (-121.972360)).abs() < 0.000001,
            "longitude should be ~-121.972360, got {}", lon_signed);
    }

    #[test]
    fn telit_gpsacp_south_east_hemisphere() {
        // Synthetic: coordinates in Southern/Eastern hemisphere
        let re = regex::Regex::new(
            r"\$GPSACP:\s*(?P<time>\d+\.\d+),(?P<lat>\d+\.\d+),(?P<ns>[NS]),(?P<lon>\d+\.\d+),(?P<ew>[EW]),[\d.]*,(?P<alt>[\d.]+),(?P<fix>\d+),[\d.]*,(?P<speed>[\d.]+),[\d.]*,(?P<date>\d+),(?P<satellites>\d+)"
        ).unwrap();
        let response = "$GPSACP: 120000.000,3350.5000,S,15110.2500,E,1.0,50.0,3,0.0,1.5,0.8,010126,8";

        let caps = re.captures(response).expect("regex should match");
        use super::nmea_to_decimal;
        let lat = nmea_to_decimal(cap_f64(&caps, "lat"));
        let lon = nmea_to_decimal(cap_f64(&caps, "lon"));
        let ns = cap_str(&caps, "ns");
        let ew = cap_str(&caps, "ew");
        let lat_signed = if ns == "S" { -lat } else { lat };
        let lon_signed = if ew == "W" { -lon } else { lon };

        // 3350.5000,S → -(33 + 50.5/60) = -33.841667
        assert!((lat_signed - (-33.841667)).abs() < 0.001,
            "S latitude should be negative, got {}", lat_signed);
        // 15110.2500,E → 151 + 10.25/60 = 151.170833
        assert!((lon_signed - 151.170833).abs() < 0.001,
            "E longitude should be positive, got {}", lon_signed);
    }

    // =========================================================================
    // Item #37 sub-task 2b — find_qmi_control_device_for_bus_port tests
    // =========================================================================

    #[cfg(all(unix, feature = "real-hardware"))]
    #[test]
    fn find_qmi_control_device_in_resolves_qmi_wwan() {
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().expect("tempdir");
        let usbmisc = tmp.path();
        // Build:
        //   <tmp>/cdc-wdm0/device → <tmp>/.usbdev0/usb1/4-1.1/4-1.1:1.0
        // The bus-port walk picks the longest matching `<digit>-<port>`
        // component in the canonicalized path, so 4-1.1 wins over (none).
        let device_dir = usbmisc.join(".usbdev0").join("usb1").join("4-1.1").join("4-1.1:1.0");
        std::fs::create_dir_all(&device_dir).unwrap();
        let cdc_wdm = usbmisc.join("cdc-wdm0");
        std::fs::create_dir_all(&cdc_wdm).unwrap();
        symlink(&device_dir, cdc_wdm.join("device")).unwrap();

        let result = super::real_hardware::find_qmi_control_device_in(usbmisc, "4-1.1");
        assert_eq!(result.as_deref(), Some("/dev/cdc-wdm0"));
    }

    #[cfg(all(unix, feature = "real-hardware"))]
    #[test]
    fn find_qmi_control_device_in_resolves_cdc_mbim() {
        // Under spec §4 Q-E (E2), driver type doesn't matter — only
        // bus-port match. Same fixture shape as qmi_wwan test; the
        // `cdc-wdm*` filename is what's globbed, not the driver behind it.
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().expect("tempdir");
        let usbmisc = tmp.path();
        let device_dir = usbmisc.join(".usbdev0").join("usb1").join("3-2").join("3-2:1.0");
        std::fs::create_dir_all(&device_dir).unwrap();
        let cdc_wdm = usbmisc.join("cdc-wdm1");
        std::fs::create_dir_all(&cdc_wdm).unwrap();
        symlink(&device_dir, cdc_wdm.join("device")).unwrap();

        let result = super::real_hardware::find_qmi_control_device_in(usbmisc, "3-2");
        assert_eq!(result.as_deref(), Some("/dev/cdc-wdm1"));
    }

    #[cfg(all(unix, feature = "real-hardware"))]
    #[test]
    fn find_qmi_control_device_in_returns_none_for_no_match() {
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().expect("tempdir");
        let usbmisc = tmp.path();
        let device_dir = usbmisc.join(".usbdev0").join("usb1").join("4-1.1").join("4-1.1:1.0");
        std::fs::create_dir_all(&device_dir).unwrap();
        let cdc_wdm = usbmisc.join("cdc-wdm0");
        std::fs::create_dir_all(&cdc_wdm).unwrap();
        symlink(&device_dir, cdc_wdm.join("device")).unwrap();

        // Looking for a different bus-port that doesn't exist in fixture
        let result = super::real_hardware::find_qmi_control_device_in(usbmisc, "9-9");
        assert_eq!(result, None);
    }

    #[test]
    fn find_qmi_control_device_in_returns_none_for_empty_root() {
        // Empty tempdir → no cdc-wdm* glob matches → None.
        // Cross-platform; no symlink needed.
        let tmp = tempfile::tempdir().expect("tempdir");
        #[cfg(feature = "real-hardware")]
        {
            let result = super::real_hardware::find_qmi_control_device_in(tmp.path(), "4-1");
            assert_eq!(result, None);
        }
        #[cfg(not(feature = "real-hardware"))]
        {
            // Mock build — production wrapper always returns None
            let result = super::find_qmi_control_device_for_bus_port("4-1");
            assert_eq!(result, None);
            let _ = tmp; // silence unused warning
        }
    }

    #[test]
    fn find_qmi_control_device_for_bus_port_returns_none_on_missing_sysfs() {
        // Production wrapper hits /sys/class/usbmisc/cdc-wdm*. On Windows the
        // path doesn't exist; on Linux, "no-such-bus-port-99-99" doesn't match
        // anything real. Either way, None is the contract.
        assert_eq!(
            super::find_qmi_control_device_for_bus_port("no-such-bus-port-99-99"),
            None
        );
    }

    // =========================================================================
    // F1 — stale serial fd self-heal (Layer 1)
    //
    // These tests run in CI under default (mock-hardware) features: the
    // classifier and the reopen-once state machine are module-scope free
    // functions / traits compiled outside the `real-hardware` gate, exercised
    // here over in-memory fakes. The real `serialport::TTYPort` open + bus-port
    // re-detection stay `real-hardware`-gated and bench-verified.
    // =========================================================================

    use std::io::{self, Read, Write};

    #[test]
    fn should_reopen_on_fd_dead_kinds() {
        // The fd-dead set: a re-enumeration (ttyUSB2↔ttyUSB3) drops the old fd,
        // so writes/reads surface as one of these. Each must trigger a reopen.
        assert!(super::should_reopen_after_io_error(io::ErrorKind::BrokenPipe));
        assert!(super::should_reopen_after_io_error(io::ErrorKind::NotConnected));
        assert!(super::should_reopen_after_io_error(io::ErrorKind::UnexpectedEof));
    }

    #[test]
    fn should_not_reopen_on_timed_out() {
        // TimedOut is the read-loop's normal "no data yet" signal — it must stay
        // a retry-in-place `continue`, never a reopen.
        assert!(!super::should_reopen_after_io_error(io::ErrorKind::TimedOut));
    }

    #[test]
    fn should_not_reopen_on_interrupted() {
        // EINTR is transient and the fd is still alive — no reopen.
        assert!(!super::should_reopen_after_io_error(io::ErrorKind::Interrupted));
    }

    #[test]
    fn should_reopen_allow_set_is_exactly_fd_dead() {
        // Pin the allow-set: ONLY the three fd-dead kinds reopen; every other
        // kind we might plausibly see does not. Guards against the set silently
        // widening (which would reopen on recoverable/protocol errors).
        let reopen_kinds = [
            io::ErrorKind::BrokenPipe,
            io::ErrorKind::NotConnected,
            io::ErrorKind::UnexpectedEof,
        ];
        let no_reopen_kinds = [
            io::ErrorKind::TimedOut,
            io::ErrorKind::Interrupted,
            io::ErrorKind::WouldBlock,
            io::ErrorKind::PermissionDenied,
            io::ErrorKind::InvalidData,
            io::ErrorKind::InvalidInput,
            io::ErrorKind::Other,
            io::ErrorKind::ConnectionReset,
            io::ErrorKind::AddrInUse,
            io::ErrorKind::AlreadyExists,
            io::ErrorKind::WriteZero,
        ];
        for k in reopen_kinds {
            assert!(super::should_reopen_after_io_error(k), "{k:?} must reopen");
        }
        for k in no_reopen_kinds {
            assert!(!super::should_reopen_after_io_error(k), "{k:?} must NOT reopen");
        }
    }

    // --- Retry-sequence state machine over fakes ---

    /// A scripted fake serial port. Each call to write/read either succeeds
    /// (echoing back the queued response on read) or fails with a queued
    /// `io::ErrorKind`. Counts reads so the test can assert the response source.
    struct FakeSerial {
        /// Response bytes this port will hand back on a successful read pass.
        response: Vec<u8>,
        /// If set, the next write fails with this kind (consumed once).
        fail_write: Option<io::ErrorKind>,
        read_pos: usize,
        write_log: Vec<u8>,
        /// True once the command has been written. The scripted response models
        /// the modem's reply to that command, so it only becomes readable AFTER
        /// the write — before the write, reads return `TimedOut` (no data yet).
        /// This also makes the fake compatible with `at_exchange`'s R1
        /// pre-command drain, which must find an empty input buffer.
        wrote: bool,
    }

    impl FakeSerial {
        fn ok(response: &str) -> Self {
            FakeSerial {
                response: response.as_bytes().to_vec(),
                fail_write: None,
                read_pos: 0,
                write_log: Vec::new(),
                wrote: false,
            }
        }
        fn failing_write(kind: io::ErrorKind) -> Self {
            FakeSerial {
                response: Vec::new(),
                fail_write: Some(kind),
                read_pos: 0,
                write_log: Vec::new(),
                wrote: false,
            }
        }
    }

    impl Read for FakeSerial {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if !self.wrote || self.read_pos >= self.response.len() {
                // Before the command write the buffer is empty (R1 drain sees
                // nothing); after the response is exhausted there is no more
                // scripted data. Either way emulate the serial timeout the read
                // loop treats as `continue`. The driver bounds total time.
                return Err(io::Error::new(io::ErrorKind::TimedOut, "fake idle"));
            }
            let remaining = &self.response[self.read_pos..];
            let n = remaining.len().min(buf.len());
            buf[..n].copy_from_slice(&remaining[..n]);
            self.read_pos += n;
            Ok(n)
        }
    }

    impl Write for FakeSerial {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if let Some(kind) = self.fail_write.take() {
                return Err(io::Error::new(kind, "fake write failure"));
            }
            self.write_log.extend_from_slice(buf);
            // The command has now been sent; the scripted response becomes
            // readable (the modem's reply arrives only after it receives the cmd).
            self.wrote = true;
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl super::SerialIo for FakeSerial {}

    /// A scripted opener: hands out pre-built ports in order, then fails.
    /// Tracks how many times it was asked to open (reopen-count assertion).
    struct FakeOpener {
        ports: Vec<FakeSerial>,
        open_calls: usize,
    }

    impl super::PortOpener for FakeOpener {
        type Port = FakeSerial;
        fn open(&mut self) -> super::HardwareResult<FakeSerial> {
            self.open_calls += 1;
            if self.ports.is_empty() {
                return Err(super::HardwareError::DeviceNotFound("no fake port".into()));
            }
            Ok(self.ports.remove(0))
        }
    }

    #[test]
    fn reopen_once_recovers_after_broken_pipe() {
        // Current port's write dies with BrokenPipe (fd re-enumerated). The
        // driver must reopen exactly once, retry on the fresh port, and return
        // the fresh port's OK response.
        let mut port = FakeSerial::failing_write(io::ErrorKind::BrokenPipe);
        let mut opener = FakeOpener {
            ports: vec![FakeSerial::ok("OK\r\n")],
            open_calls: 0,
        };

        let result = super::run_at_command_with_reopen(
            &mut port,
            &mut opener,
            "AT+CSQ",
            std::time::Duration::from_secs(1),
        );

        assert_eq!(opener.open_calls, 1, "must reopen exactly once");
        let resp = result.expect("retry on fresh port should succeed");
        assert!(resp.contains("OK"), "should return fresh port response, got {resp:?}");
    }

    #[test]
    fn reopen_attempted_once_then_errors_when_fresh_port_also_dead() {
        // Original port dies with BrokenPipe; the reopened port ALSO dies. The
        // driver must NOT loop — exactly one reopen attempt, then Err.
        let mut port = FakeSerial::failing_write(io::ErrorKind::BrokenPipe);
        let mut opener = FakeOpener {
            ports: vec![FakeSerial::failing_write(io::ErrorKind::BrokenPipe)],
            open_calls: 0,
        };

        let result = super::run_at_command_with_reopen(
            &mut port,
            &mut opener,
            "AT+CSQ",
            std::time::Duration::from_secs(1),
        );

        assert_eq!(opener.open_calls, 1, "exactly one reopen attempt, no second");
        assert!(result.is_err(), "persistent failure must propagate as Err");
    }

    #[test]
    fn timed_out_does_not_trigger_reopen() {
        // A port that writes fine but only ever yields TimedOut reads must NOT
        // reopen — it runs the read loop to the command timeout and returns
        // Timeout (matching existing send_command semantics), never opening.
        let mut port = FakeSerial::ok(""); // empty response → only TimedOut reads
        let mut opener = FakeOpener { ports: vec![], open_calls: 0 };

        let result = super::run_at_command_with_reopen(
            &mut port,
            &mut opener,
            "AT+CSQ",
            std::time::Duration::from_millis(100),
        );

        assert_eq!(opener.open_calls, 0, "TimedOut must never reopen");
        assert!(
            matches!(result, Err(super::HardwareError::Timeout)),
            "read-loop timeout should surface as HardwareError::Timeout, got {result:?}"
        );
    }

    #[test]
    fn healthy_port_returns_response_without_reopen() {
        // No fault at all → no reopen, response returned as-is.
        let mut port = FakeSerial::ok("+CSQ: 20,99\r\nOK\r\n");
        let mut opener = FakeOpener { ports: vec![], open_calls: 0 };

        let result = super::run_at_command_with_reopen(
            &mut port,
            &mut opener,
            "AT+CSQ",
            std::time::Duration::from_secs(1),
        );

        assert_eq!(opener.open_calls, 0, "healthy path must not reopen");
        let resp = result.expect("healthy command should succeed");
        assert!(resp.contains("+CSQ: 20,99"));
        assert!(resp.contains("OK"));
    }

    #[test]
    fn carriage_return_in_cmd_is_rejected_and_not_transmitted() {
        // CRITICAL injection: a whitelist-validated first token (`AT+CFUN=1`) with
        // an embedded CR smuggles a SECOND, permanently-blocked firmware command
        // (`AT+QFASTBOOT`) onto the serial wire. The serial choke point MUST reject
        // the whole command BEFORE any byte is written.
        let mut port = FakeSerial::ok("OK\r\n");
        let mut opener = FakeOpener { ports: vec![], open_calls: 0 };

        let result = super::run_at_command_with_reopen(
            &mut port,
            &mut opener,
            "AT+CFUN=1\rAT+QFASTBOOT",
            std::time::Duration::from_secs(1),
        );

        assert!(
            matches!(result, Err(super::HardwareError::CommandRejected(_))),
            "embedded CR must be rejected as CommandRejected, got {result:?}"
        );
        assert!(
            port.write_log.is_empty(),
            "NOTHING may be written when the command is rejected, wrote {:?}",
            port.write_log
        );
        assert_eq!(opener.open_calls, 0, "rejection must not reopen");
    }

    #[test]
    fn line_feed_in_cmd_is_rejected_and_not_transmitted() {
        // Same class of attack using LF as the embedded line terminator.
        let mut port = FakeSerial::ok("OK\r\n");
        let mut opener = FakeOpener { ports: vec![], open_calls: 0 };

        let result = super::run_at_command_with_reopen(
            &mut port,
            &mut opener,
            "AT+CFUN=1\nAT+QFASTBOOT",
            std::time::Duration::from_secs(1),
        );

        assert!(
            matches!(result, Err(super::HardwareError::CommandRejected(_))),
            "embedded LF must be rejected as CommandRejected, got {result:?}"
        );
        assert!(
            port.write_log.is_empty(),
            "NOTHING may be written when the command is rejected, wrote {:?}",
            port.write_log
        );
    }

    #[test]
    fn normal_multi_arg_command_transmits_unchanged() {
        // A legitimate multi-argument command must pass the guard and reach the
        // wire byte-for-byte (plus the single trailing \r the writer adds itself).
        let mut port = FakeSerial::ok("+QENG: \"servingcell\",\"NOCONN\"\r\nOK\r\n");
        let mut opener = FakeOpener { ports: vec![], open_calls: 0 };

        let cmd = "AT+QENG=\"servingcell\"";
        let result = super::run_at_command_with_reopen(
            &mut port,
            &mut opener,
            cmd,
            std::time::Duration::from_secs(1),
        );

        let resp = result.expect("legitimate command must succeed");
        assert!(resp.contains("OK"));
        assert_eq!(
            port.write_log,
            format!("{cmd}\r").into_bytes(),
            "the legitimate command must reach the wire byte-identical (cmd + \\r)"
        );
    }

    #[test]
    fn cgdcont_build_rejects_quote_or_control_char_in_apn() {
        // Defense-in-depth at the CGDCONT builder: an operator-supplied APN that
        // contains a double-quote (breaks out of the quoted argument) or a control
        // char (smuggles a second command) must be rejected before the command is
        // built.
        assert!(
            super::validate_at_quoted_arg("internet").is_ok(),
            "a clean APN must pass"
        );
        assert!(
            super::validate_at_quoted_arg("apn.with-dots_123").is_ok(),
            "typical APN punctuation must pass"
        );
        assert!(
            super::validate_at_quoted_arg("evil\",\"IP\"\rAT+QFASTBOOT").is_err(),
            "an APN containing a double-quote must be rejected"
        );
        assert!(
            super::validate_at_quoted_arg("evil\rAT+QFASTBOOT").is_err(),
            "an APN containing a CR must be rejected"
        );
        assert!(
            super::validate_at_quoted_arg("evil\nfoo").is_err(),
            "an APN containing a LF must be rejected"
        );
    }

    #[test]
    fn is_device_gone_errno_matches_removed_device_errnos() {
        // EIO(5)/ENXIO(6)/ENODEV(19) = the device is physically gone (a write/read to a
        // removed USB-serial). Reopen-worthy. Other errnos must NOT match (no fault masking).
        assert!(super::is_device_gone_errno(Some(5)));
        assert!(super::is_device_gone_errno(Some(6)));
        assert!(super::is_device_gone_errno(Some(19)));
        assert!(!super::is_device_gone_errno(Some(13))); // EACCES
        assert!(!super::is_device_gone_errno(Some(0)));
        assert!(!super::is_device_gone_errno(None));
    }

    #[test]
    fn classify_io_treats_device_gone_errno_as_reopenable() {
        // A dead-device write surfaces EIO → must be Reopenable so the reopen-once
        // driver self-heals, exactly like the BrokenPipe (read-side) case.
        let eio = std::io::Error::from_raw_os_error(5);
        assert!(matches!(
            super::classify_io(eio, "Write failed"),
            super::AtExchangeError::Reopenable(_)
        ));
        // A non-device-gone errno (EACCES) stays Fatal — no spurious reopen.
        let eacces = std::io::Error::from_raw_os_error(13);
        assert!(matches!(
            super::classify_io(eacces, "Write failed"),
            super::AtExchangeError::Fatal(_)
        ));
    }

    #[test]
    fn mock_handler_has_no_live_device_path_handle() {
        // The mock has no stable serial fd, so it inherits the trait default
        // (None). This is what makes mock-backed ModemContexts a reconcile no-op.
        let mock = crate::hardware::mock::MockHardware::new();
        let handle = super::ModemHardware::live_device_path_handle(&mock);
        assert!(handle.is_none(), "mock must report no live device-path handle");
    }

    // Bench-only real-hardware verification of the actual port re-detection +
    // reopen path. NOT run in CI (no serial hardware, `real-hardware`-gated, and
    // `#[ignore]`d). Run manually on the bench against an RM551E-GL after forcing
    // a re-enumeration:
    //   1. SSH to the router, `AT+CFUN=0` then `AT+CFUN=1` (or reboot the modem)
    //      to flip ttyUSB2↔ttyUSB3 while the daemon holds the old fd.
    //   2. cargo test --features real-hardware -- --ignored reopen_port_recovers_real_hardware
    // Expected: `reopen_port()` re-detects the new ttyUSB on the same bus-port and
    // returns a `TTYPort` that answers AT/OK — no service restart needed.
    #[cfg(all(unix, feature = "real-hardware"))]
    #[test]
    #[ignore = "requires real modem hardware + a forced USB re-enumeration; bench-only"]
    fn reopen_port_recovers_real_hardware() {
        // Intentionally a documented stub: a full assertion needs a constructed
        // AtHandler bound to a live modem's bus-port, which is not available in
        // any automated environment. Bench operator wires the AtHandler for the
        // detected RM551E-GL, forces the CFUN re-enumeration, then asserts:
        //   1. `handler.reopen_port()` is `Ok` and the returned port answers AT/OK.
        //   2. After a successful reopen, the live-device-path cell holds the
        //      recovered port path (non-empty). Template:
        //      let handle = super::super::ModemHardware::live_device_path_handle(&handler)
        //          .expect("AtHandler must expose a live_device_path handle");
        //      let live = handle.lock().expect("cell lock").clone();
        //      assert!(!live.is_empty(), "cell must hold the recovered AT port path");
        // See the module-level F1 design notes and Item #42 Phase 4 acceptance.
        unimplemented!("bench-only: construct AtHandler for live modem, force re-enum, assert reopen_port() Ok + live_device_path_handle cell non-empty");
    }

    // =========================================================================
    // QICSGP redaction — password must never appear in logs or debug traces
    // =========================================================================

    #[test]
    fn qicsgp_write_cmd_password_redacted() {
        // AT+QICSGP write: field order is cid,type,"apn","user","password",auth
        let cmd = r#"AT+QICSGP=1,1,"internet","myuser","s3cr3t!",2"#;
        let result = super::redact_qicsgp(cmd);
        assert!(
            !result.contains("s3cr3t!"),
            "write command must not contain plaintext password, got: {result}"
        );
        assert!(
            result.contains(r#"AT+QICSGP=1,1,"internet","myuser","#),
            "prefix fields must be preserved, got: {result}"
        );
        assert!(
            result.ends_with(",2"),
            "auth field must be preserved, got: {result}"
        );
    }

    #[test]
    fn qicsgp_response_line_password_redacted() {
        // +QICSGP: response: field order is type,"apn","user","password",auth
        let resp = r#"+QICSGP: 1,"internet","myuser","s3cr3t!",2"#;
        let result = super::redact_qicsgp(resp);
        assert!(
            !result.contains("s3cr3t!"),
            "response line must not contain plaintext password, got: {result}"
        );
        assert!(
            result.contains(r#"+QICSGP: 1,"internet","myuser","#),
            "prefix fields must be preserved, got: {result}"
        );
        assert!(
            result.ends_with(",2"),
            "auth field must be preserved, got: {result}"
        );
    }

    #[test]
    fn qicsgp_empty_password_handled_correctly() {
        // Open-APN case: password is ""
        let cmd = r#"AT+QICSGP=1,1,"internet","","",0"#;
        let result = super::redact_qicsgp(cmd);
        // Must not corrupt the line; prefix and auth must survive
        assert!(
            result.contains(r#"AT+QICSGP=1,1,"internet","","#),
            "empty-password write must preserve prefix fields, got: {result}"
        );
        assert!(
            result.ends_with(",0"),
            "auth field must be preserved for empty-password case, got: {result}"
        );
        // The password slot is replaced by the redaction marker (even for "")
        assert!(
            result.contains("<redacted>"),
            "empty password must still emit redaction marker, got: {result}"
        );
    }

    #[test]
    fn non_qicsgp_line_passes_through_unchanged() {
        // Unrelated AT commands must never be altered
        for cmd in &[
            "AT+CGDCONT?",
            "AT+CSQ",
            "AT+CFUN=1",
            "+CSQ: 20,0",
            "+CGDCONT: 1,\"IP\",\"internet\"",
            "OK",
        ] {
            let result = super::redact_qicsgp(cmd);
            assert_eq!(result, *cmd, "non-QICSGP line must pass through unchanged");
        }
    }

    #[test]
    fn qicsgp_multiline_response_redacts_only_qicsgp_line() {
        // Multi-line AT response: +QICSGP: line plus trailing OK
        let response = "+QICSGP: 1,\"internet\",\"myuser\",\"s3cr3t!\",2\r\nOK\r\n";
        let result = super::redact_qicsgp(response);
        assert!(
            !result.contains("s3cr3t!"),
            "multiline: password must be redacted, got: {result}"
        );
        assert!(
            result.contains("OK"),
            "multiline: non-QICSGP lines must be preserved, got: {result}"
        );
    }

    // =========================================================================
    // UTF-8-safe truncation — diagnostic previews must never panic on a
    // multibyte codepoint straddling the byte limit.
    // =========================================================================

    #[test]
    fn truncate_short_string_returned_unchanged() {
        let s = "AT+CSQ OK";
        assert_eq!(super::truncate_on_char_boundary(s, 200), s);
    }

    #[test]
    fn truncate_empty_string_returned_unchanged() {
        assert_eq!(super::truncate_on_char_boundary("", 200), "");
    }

    #[test]
    fn truncate_exactly_at_limit_returned_whole() {
        let s = "abcde"; // 5 bytes, limit 5
        assert_eq!(super::truncate_on_char_boundary(s, 5), s);
    }

    #[test]
    fn truncate_ascii_over_limit_truncates_to_limit() {
        let s = "a".repeat(250);
        let out = super::truncate_on_char_boundary(&s, 200);
        // Pure ASCII: every byte is a char boundary, so it truncates to exactly 200.
        assert_eq!(out.len(), 200);
        assert!(s.starts_with(out));
    }

    #[test]
    fn truncate_mid_multibyte_codepoint_does_not_panic() {
        // 'é' (U+00E9) is 2 bytes in UTF-8. Build a string where the byte at the
        // limit lands in the MIDDLE of one of these codepoints. A naive
        // `&s[..limit]` would panic with "byte index N is not a char boundary".
        //
        // 100 × "é" = 200 bytes. With limit 199, byte 199 is the 2nd byte of the
        // 100th 'é' — not a char boundary.
        let s = "é".repeat(100); // 200 bytes
        assert_eq!(s.len(), 200);
        let out = super::truncate_on_char_boundary(&s, 199);
        // Must back off to the previous boundary (198) — 99 full 'é' chars.
        assert_eq!(out.len(), 198);
        assert_eq!(out.chars().count(), 99);
        // And it must be a valid &str (the assert above proves no panic, this
        // proves we did not corrupt a codepoint).
        assert!(out.chars().all(|c| c == 'é'));
    }

    #[test]
    fn truncate_never_panics_at_any_boundary_in_multibyte_string() {
        // Sweep every possible limit across a mixed multibyte string. If any
        // limit produced a mid-codepoint slice, this would panic.
        let s = "AT+COPS: 0,0,\"Téléphøne 日本語 Ñandú\",7\r\nOK\r\n";
        for limit in 0..=s.len() + 5 {
            let out = super::truncate_on_char_boundary(s, limit);
            // Result is always a valid prefix of s.
            assert!(s.starts_with(out));
            // Result length never exceeds the requested limit.
            assert!(out.len() <= limit || out.len() == s.len());
        }
    }

    // =========================================================================
    // AT-channel URC-filtering / read-framing hardening (2026-06-18 spec)
    // R1 pre-command drain · R2 echo-anchored framing · R3 URC filtering
    // (`io`, `Read`, `Write` are already imported by the F1 test block above.)
    // =========================================================================

    /// A scripted fake serial port for the framing tests. Distinct from the
    /// retry-path `FakeSerial`: it serves a queue of "pre-pending" stale bytes
    /// (already sitting in the OS input buffer at write time — the R1 drain
    /// target) separately from the bytes that arrive *after* the command write
    /// (the real exchange). This lets a test model the "stale tail of a prior
    /// command crosses into the next read" defect precisely.
    struct FramingSerial {
        /// Bytes pending in the input buffer *before* the command is written.
        /// `at_exchange` must drain these (R1) so they never enter the response.
        pending: Vec<u8>,
        pending_pos: usize,
        /// Bytes that become readable only *after* the command write (the modem's
        /// echo + real response + terminal, plus any interleaved URCs).
        after: Vec<u8>,
        after_pos: usize,
        /// One-shot boundary: once `pending` is exhausted, the next `read()`
        /// returns `TimedOut` to model "nothing more *immediately* available"
        /// (the R1 drain's stop condition) before `after` becomes readable.
        boundary_pending: bool,
        write_log: Vec<u8>,
    }

    impl FramingSerial {
        /// `pending`: stale pre-command bytes; `after`: post-write exchange bytes.
        fn new(pending: &str, after: &str) -> Self {
            FramingSerial {
                pending: pending.as_bytes().to_vec(),
                pending_pos: 0,
                after: after.as_bytes().to_vec(),
                after_pos: 0,
                boundary_pending: true,
                write_log: Vec::new(),
            }
        }
    }

    impl Read for FramingSerial {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            // Pending (pre-command) bytes are served first and only while they
            // remain — once exhausted we serve the post-write exchange bytes.
            if self.pending_pos < self.pending.len() {
                let remaining = &self.pending[self.pending_pos..];
                let n = remaining.len().min(buf.len());
                buf[..n].copy_from_slice(&remaining[..n]);
                self.pending_pos += n;
                return Ok(n);
            }
            // Boundary between pending and after: a single TimedOut so the R1
            // drain stops (nothing immediately available) and `after` only
            // becomes readable on the subsequent (post-write) reads.
            if self.boundary_pending {
                self.boundary_pending = false;
                return Err(io::Error::new(io::ErrorKind::TimedOut, "fake drain boundary"));
            }
            if self.after_pos < self.after.len() {
                let remaining = &self.after[self.after_pos..];
                let n = remaining.len().min(buf.len());
                buf[..n].copy_from_slice(&remaining[..n]);
                self.after_pos += n;
                return Ok(n);
            }
            // No more scripted data: emulate the serial read timeout that the
            // read loop treats as `continue`; the driver bounds total time.
            Err(io::Error::new(io::ErrorKind::TimedOut, "fake idle"))
        }
    }

    impl Write for FramingSerial {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.write_log.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl super::SerialIo for FramingSerial {}

    fn exch(port: &mut FramingSerial, cmd: &str) -> Result<String, super::AtExchangeError> {
        super::at_exchange(port, cmd, std::time::Duration::from_secs(1))
    }

    // --- R1: pre-command input drain ----------------------------------------

    #[test]
    fn r1_drains_stale_pending_bytes_before_command() {
        // A prior command's late response is still sitting in the buffer. The
        // drain must discard it so it never contaminates this command's output.
        let mut port = FramingSerial::new(
            "+CSQ: 19,99\r\nOK\r\n",                       // stale tail of a prior AT+CSQ
            "AT+QUIMSLOT?\r\r\n+QUIMSLOT: 1\r\nOK\r\n",    // our echo + real response
        );
        let resp = exch(&mut port, "AT+QUIMSLOT?").expect("exchange should succeed");
        assert!(
            resp.contains("+QUIMSLOT: 1"),
            "real response must be returned, got {resp:?}"
        );
        assert!(
            !resp.contains("+CSQ"),
            "stale pre-command bytes must be drained, got {resp:?}"
        );
    }

    // --- R2: echo-anchored framing ------------------------------------------

    #[test]
    fn r2_strips_command_echo_from_response() {
        // Echo is ON: the modem echoes "AT+CSQ" back. It must not appear in the
        // returned response body (downstream parsers want clean output).
        let mut port = FramingSerial::new("", "AT+CSQ\r\r\n+CSQ: 20,99\r\nOK\r\n");
        let resp = exch(&mut port, "AT+CSQ").expect("exchange should succeed");
        assert!(resp.contains("+CSQ: 20,99"), "response body kept, got {resp:?}");
        assert!(
            !resp.lines().any(|l| l.trim() == "AT+CSQ"),
            "command echo line must be stripped, got {resp:?}"
        );
    }

    #[test]
    fn r2_bare_at_echo_crossing_does_not_terminate_early() {
        // The 2026-06-18 bench defect: a stale `AT\r\r\nOK` (echo+terminal of a
        // prior bare-AT health check) is sitting in the buffer when we issue
        // AT+QUIMSLOT?. The stale OK must NOT terminate the read before our real
        // response arrives; the real +QUIMSLOT response must be returned.
        // The stale `AT\r\r\nOK` lands in the post-write stream (it had not fully
        // arrived at drain time), BEFORE our own echo. R2b: that stale OK must NOT
        // terminate the read; we must wait for our echo and the real response.
        let mut port = FramingSerial::new(
            "",
            "AT\r\r\nOK\r\n\
             AT+QUIMSLOT?\r\r\n+QUIMSLOT: 1\r\nOK\r\n", // stale echo+OK, then our echo + real
        );
        let resp = exch(&mut port, "AT+QUIMSLOT?").expect("exchange should succeed");
        assert!(
            resp.contains("+QUIMSLOT: 1"),
            "must return the real QUIMSLOT response, not stop at the stale OK; got {resp:?}"
        );
    }

    #[test]
    fn r2_degrades_safely_when_echo_absent() {
        // ATE0 modem: no echo. After R1's drain the buffer holds only our
        // exchange, so a terminal arriving with no preceding echo is accepted.
        let mut port = FramingSerial::new("", "+CSQ: 20,99\r\nOK\r\n");
        let resp = exch(&mut port, "AT+CSQ").expect("echo-absent exchange should succeed");
        assert!(resp.contains("+CSQ: 20,99"), "got {resp:?}");
        assert!(resp.contains("OK"), "terminal accepted with no echo, got {resp:?}");
    }

    // --- R3: URC filtering --------------------------------------------------

    #[test]
    fn r3_filters_interleaved_urcs_from_response() {
        // URCs (+CREG/+QIND/+CGEV) interleave before and within the response.
        // They must be filtered out so the parser sees only command output.
        let mut port = FramingSerial::new(
            "",
            "AT+CSQ\r\r\n+CREG: 1,\"1A2B\",\"00C1D2\",7\r\n+CSQ: 20,99\r\n+QIND: act,\"LTE\"\r\nOK\r\n",
        );
        let resp = exch(&mut port, "AT+CSQ").expect("exchange should succeed");
        assert!(resp.contains("+CSQ: 20,99"), "real response kept, got {resp:?}");
        assert!(!resp.contains("+CREG"), "+CREG URC must be filtered, got {resp:?}");
        assert!(!resp.contains("+QIND"), "+QIND URC must be filtered, got {resp:?}");
    }

    #[test]
    fn r3_dual_use_guard_keeps_cpin_for_cpin_query() {
        // +CPIN: is both a URC and the response to AT+CPIN?. For the AT+CPIN?
        // command its own response family MUST be kept, never filtered.
        let mut port = FramingSerial::new(
            "",
            "AT+CPIN?\r\r\n+CPIN: READY\r\nOK\r\n",
        );
        let resp = exch(&mut port, "AT+CPIN?").expect("exchange should succeed");
        assert!(
            resp.contains("+CPIN: READY"),
            "+CPIN: response must be kept for AT+CPIN?, got {resp:?}"
        );
    }

    #[test]
    fn r3_qmbncfg_autosel_read_clean_under_urc_flood() {
        // The dev.56 regression repro at the read layer: a +QMBNCFG AutoSel read
        // arrives interleaved with a URC flood. After filtering, the response
        // must contain a clean +QMBNCFG line and none of the URC noise.
        let mut port = FramingSerial::new(
            "",
            "AT+QMBNCFG=\"AutoSel\"\r\r\n\
             +CREG: 1\r\n\
             +QIND: SMS DONE\r\n\
             +QMBNCFG: \"AutoSel\",1\r\n\
             +CGEV: ME PDN ACT 1\r\n\
             OK\r\n",
        );
        let resp = exch(&mut port, "AT+QMBNCFG=\"AutoSel\"").expect("exchange should succeed");
        assert!(
            resp.contains("+QMBNCFG: \"AutoSel\",1"),
            "clean QMBNCFG line must survive, got {resp:?}"
        );
        assert!(!resp.contains("+CREG"), "URC filtered, got {resp:?}");
        assert!(!resp.contains("+QIND"), "URC filtered, got {resp:?}");
        assert!(!resp.contains("+CGEV"), "URC filtered, got {resp:?}");
    }
}
