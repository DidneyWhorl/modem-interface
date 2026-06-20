//! Modem profile definitions and registry.
//!
//! A modem profile captures everything needed to communicate with a specific
//! modem model: AT command mappings, signal parsing regexes, port preferences,
//! capabilities, and AT whitelist additions.
//!
//! Built-in profiles are compiled into the binary. Additional profiles can be
//! loaded from `/etc/modem-interface/profiles/*.toml` at runtime.

use serde::{Deserialize, Serialize};
use tracing::{info, warn};

// ============================================================================
// Profile Data Structures
// ============================================================================

/// Identifies a modem model by USB vendor/product ID.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct ModemIdentity {
    /// USB vendor ID, e.g. "2c7c" (Quectel)
    pub vendor_id: String,
    /// USB product ID, e.g. "0122" (RM551E-GL)
    pub product_id: String,
    /// Manufacturer name, e.g. "Quectel"
    pub manufacturer: String,
    /// Primary model name, e.g. "RM551E-GL"
    pub model: String,
    /// Alternative model strings that also match this profile
    #[serde(default)]
    pub model_variants: Vec<String>,
}

/// AT command mappings for hardware operations.
///
/// Tells the AT handler which commands to send and how to parse responses.
/// `None` values mean "skip this command and use the fallback".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AtCommandSet {
    /// Primary signal query command (vendor-specific).
    /// e.g. `AT+QENG="servingcell"` for Quectel.
    /// None means skip and use `generic_signal_cmd` directly.
    pub signal_cmd: Option<String>,

    /// Regex to parse the signal response. Uses named capture groups:
    /// `(?P<rsrp>...)`, `(?P<rsrq>...)`, `(?P<rssi>...)`, `(?P<sinr>...)`,
    /// `(?P<band>...)`, `(?P<earfcn>...)`.
    pub signal_parse_regex: Option<String>,

    /// Fallback signal command (3GPP standard). Default: "AT+CSQ"
    #[serde(default = "default_csq")]
    pub generic_signal_cmd: String,

    /// Vendor-specific operator name command, e.g. "AT+QSPN"
    pub operator_name_cmd: Option<String>,

    /// Regex to parse operator name response.
    /// Must have capture group 1 = operator name.
    pub operator_name_regex: Option<String>,

    /// Fallback operator name command (3GPP standard).
    #[serde(default = "default_cspn")]
    pub generic_operator_cmd: String,

    /// Regex to parse fallback operator name response.
    #[serde(default = "default_cspn_regex")]
    pub generic_operator_regex: String,

    /// ICCID query command. Default: "AT+CCID"
    #[serde(default = "default_ccid")]
    pub iccid_cmd: String,

    /// Regex to parse ICCID response (capture group 1 = ICCID).
    pub iccid_regex: Option<String>,

    /// Registration check command. Default: "AT+CEREG?"
    #[serde(default = "default_cereg")]
    pub registration_cmd: String,
}

fn default_csq() -> String { "AT+CSQ".to_string() }
fn default_cspn() -> String { "AT+COPS?".to_string() }
fn default_cspn_regex() -> String { r#"\+COPS:\s*\d+,\d+,"([^"]*)""#.to_string() }
fn default_ccid() -> String { "AT+CCID".to_string() }
fn default_cereg() -> String { "AT+CEREG?".to_string() }

/// Serial port mapping preferences for this modem model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortMapping {
    /// Preferred AT port names in priority order.
    /// e.g. `["ttyUSB2", "ttyUSB3"]` for Quectel modems.
    /// NOTE: These are absolute device names, only reliable with a single modem.
    /// With multiple modems, `at_interface_preference` is used instead.
    pub at_port_preference: Vec<String>,
    /// Preferred USB interface numbers in priority order for AT port selection.
    /// e.g. `[2, 3]` means prefer the port on USB interface X:1.2, then X:1.3.
    /// Used for multi-modem setups where ttyUSB numbering is unpredictable.
    #[serde(default)]
    pub at_interface_preference: Vec<u32>,
    /// Baud rate for serial communication.
    #[serde(default = "default_baud_rate")]
    pub baud_rate: u32,
}

fn default_baud_rate() -> u32 { 115200 }

/// USB-net mode detection configuration. Boot-time only — no caching loop.
///
/// Diagnostic only — the detected mode is exposed via the API for engineers but
/// must never be surfaced in operator-facing UI per the mode-agnostic principle
/// (see `feedback_modem_mode_agnostic.md`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UsbNetDetectConfig {
    /// AT command issued at modem init to detect USB-net mode.
    /// None = detection disabled for this profile (caches `UsbNetMode::Unknown`).
    #[serde(default)]
    pub query_cmd: Option<String>,
    /// Parser key. Selects the response-mapping function in `usbnet::parse_usbnet_response`.
    /// Known keys: `"quectel_qcfg_usbnet"`, `"telit_usbcfg"`. None = log only, don't map.
    #[serde(default)]
    pub parser: Option<String>,
}

/// Modem capabilities for UI display and feature gating.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModemCapabilities {
    pub supports_5g: bool,
    pub supports_carrier_aggregation: bool,
    /// Supported network generations, e.g. ["2G", "3G", "4G", "5G"]
    #[serde(default)]
    pub supported_technologies: Vec<String>,
    /// Supported RF bands, e.g. ["B1", "B3", "n78"]
    #[serde(default)]
    pub max_supported_bands: Vec<String>,
    /// Supported communication protocols, e.g. ["qmi", "at", "mbim"]
    #[serde(default)]
    pub supported_protocols: Vec<String>,
    #[serde(default)]
    pub has_temperature_sensor: bool,
    #[serde(default)]
    pub has_gps: bool,
}

// ============================================================================
// Band & Mode Control Configuration
// ============================================================================

/// Band locking and network mode control configuration for a modem profile.
///
/// Fully data-driven: AT command templates, supported modes, and band lists
/// are all defined per-profile so different modem vendors can be supported
/// without code changes.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BandModeConfig {
    /// Whether band/mode control is supported by this modem.
    #[serde(default)]
    pub supported: bool,
    /// AT command templates for querying/setting mode and bands.
    #[serde(default)]
    pub commands: BandModeCommands,
    /// Available network modes for this modem.
    #[serde(default)]
    pub modes: Vec<NetworkModeOption>,
    /// Supported LTE bands (by number, e.g. [1, 2, 3, 7, 20]).
    #[serde(default)]
    pub lte_bands: Vec<u32>,
    /// Supported NR5G NSA bands (by number).
    #[serde(default)]
    pub nsa_nr5g_bands: Vec<u32>,
    /// Supported NR5G SA bands (by number).
    #[serde(default)]
    pub sa_nr5g_bands: Vec<u32>,
    /// Supported NRDC NR5G bands (advanced, by number).
    #[serde(default)]
    pub nrdc_nr5g_bands: Vec<u32>,
    /// Whether the modem reboots when band configuration changes.
    /// Sierra modems do; Quectel does not.
    #[serde(default)]
    pub reboot_on_band_change: bool,
    /// Band list separator in AT commands. Default: ":"
    #[serde(default = "default_band_separator")]
    pub band_separator: String,
    /// Band command variant controls how bands are queried and set.
    /// "per_type" (default) = Quectel-style: separate AT commands per band type (LTE, NSA, SA).
    /// "telit_bnd" = Telit AT#BND: single command with hex bitmask for all band types.
    #[serde(default = "default_band_command_variant")]
    pub band_command_variant: String,
}

fn default_band_separator() -> String { ":".to_string() }
fn default_band_command_variant() -> String { "per_type".to_string() }

/// AT command templates for band/mode control.
///
/// Commands with `{value}` placeholders are filled at runtime.
/// `None` means the modem doesn't support that operation.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BandModeCommands {
    /// Query current mode preference, e.g. `AT+QNWPREFCFG="mode_pref"`
    #[serde(default)]
    pub query_mode: Option<String>,
    /// Set mode preference (template), e.g. `AT+QNWPREFCFG="mode_pref",{value}`
    #[serde(default)]
    pub set_mode: Option<String>,
    /// Query NR5G disable mode, e.g. `AT+QNWPREFCFG="nr5g_disable_mode"`
    #[serde(default)]
    pub query_nr5g_disable: Option<String>,
    /// Set NR5G disable mode (template), e.g. `AT+QNWPREFCFG="nr5g_disable_mode",{value}`
    #[serde(default)]
    pub set_nr5g_disable: Option<String>,
    /// Query LTE bands
    #[serde(default)]
    pub query_lte_bands: Option<String>,
    /// Set LTE bands (template with {value} = colon-separated band list)
    #[serde(default)]
    pub set_lte_bands: Option<String>,
    /// Query NSA NR5G bands
    #[serde(default)]
    pub query_nsa_bands: Option<String>,
    /// Set NSA NR5G bands (template)
    #[serde(default)]
    pub set_nsa_bands: Option<String>,
    /// Query SA NR5G bands
    #[serde(default)]
    pub query_sa_bands: Option<String>,
    /// Set SA NR5G bands (template)
    #[serde(default)]
    pub set_sa_bands: Option<String>,
    /// Query NRDC NR5G bands
    #[serde(default)]
    pub query_nrdc_bands: Option<String>,
    /// Set NRDC NR5G bands (template)
    #[serde(default)]
    pub set_nrdc_bands: Option<String>,
    /// Query NRDC mode (0/1)
    #[serde(default)]
    pub query_nrdc_mode: Option<String>,
    /// Set NRDC mode (template)
    #[serde(default)]
    pub set_nrdc_mode: Option<String>,
    /// Query all band types in a single command (Telit AT#BND?).
    /// Used when band_command_variant = "telit_bnd".
    #[serde(default)]
    pub query_all_bands: Option<String>,
    /// Set all band types in a single command (Telit AT#BND=...).
    /// Template with {lte_low},{lte_high},{nsa_low},{nsa_high},{sa_low},{sa_high} placeholders.
    #[serde(default)]
    pub set_all_bands: Option<String>,
    /// Restore all bands to factory default, e.g. `AT+QNWPREFCFG="restore_band"`
    #[serde(default)]
    pub restore_bands: Option<String>,
}

/// A network mode option available for this modem.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkModeOption {
    /// Unique identifier, e.g. "auto", "lte", "5g_sa"
    pub id: String,
    /// Display name shown in UI, e.g. "Auto", "LTE Only"
    pub label: String,
    /// Value to send via the set_mode command, e.g. "AUTO", "LTE", "NR5G"
    pub mode_value: String,
    /// Value for nr5g_disable_mode command. None = don't send this command.
    #[serde(default)]
    pub nr5g_disable_value: Option<u8>,
    /// Which band sections are active (not greyed out) in this mode.
    pub active_sections: BandSections,
}

/// Which band sections (LTE, NSA, SA) are active for a given mode.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BandSections {
    pub lte: bool,
    pub nsa: bool,
    pub sa: bool,
}

// ============================================================================
// APN Profile Apply Configuration
// ============================================================================

/// Template-driven APN profile apply sequence for a modem model.
///
/// When the user clicks "Apply Profile", these steps are executed in order.
/// The apply sequence handles MBN selection + APN configuration + reboot.
/// Different modem vendors define their own sequences via modem profiles.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ApnApplyConfig {
    /// Whether APN profile apply is supported by this modem.
    #[serde(default)]
    pub supported: bool,
    /// Ordered list of AT command steps to execute when applying a profile.
    #[serde(default)]
    pub steps: Vec<ApnApplyStep>,
    /// Whether to always reboot the modem after applying (MBN changes need reboot).
    #[serde(default)]
    pub always_reboot: bool,
    /// Delay in ms before sending the reboot command (allows prior commands to settle).
    #[serde(default = "default_pre_reboot_delay_ms")]
    pub pre_reboot_delay_ms: u64,
}

fn default_pre_reboot_delay_ms() -> u64 { 500 }

/// A single step in an APN profile apply sequence.
///
/// AT command templates use placeholders: `{mbn_profile}`, `{cid}`, `{ip_type}`, `{apn}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApnApplyStep {
    /// Human-readable label for debug log, e.g. "Deactivate MBN".
    pub label: String,
    /// AT command template with placeholders.
    pub command: String,
    /// If true, skip this step when the APN profile has no `mbn_profile` set.
    #[serde(default)]
    pub requires_mbn: bool,
    /// Timeout in seconds for this AT command.
    #[serde(default = "default_step_timeout")]
    pub timeout_secs: u64,
}

fn default_step_timeout() -> u64 { 10 }

// ============================================================================
// MBN Carrier Profile Configuration
// ============================================================================

/// MBN (Modem Binary Nomenclature) carrier profile management configuration.
///
/// Data-driven: AT command templates for querying and managing carrier profiles
/// are defined per-modem-profile. Quectel uses AT+QMBNCFG; other vendors may
/// use different commands (e.g. Sierra AT!IMPREF).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MbnConfig {
    /// Whether MBN/carrier profile management is supported by this modem.
    #[serde(default)]
    pub supported: bool,
    /// AT command templates for MBN operations.
    #[serde(default)]
    pub commands: MbnCommands,
    /// Whether a reboot is recommended after profile selection/deactivation.
    #[serde(default)]
    pub reboot_recommended: bool,
}

/// AT command templates for MBN carrier profile management.
///
/// Commands with `{value}` placeholders are filled at runtime.
/// `None` means the modem doesn't support that operation.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MbnCommands {
    /// List all carrier profiles, e.g. `AT+QMBNCFG="List"`
    #[serde(default)]
    pub list_profiles: Option<String>,
    /// Query auto-select state, e.g. `AT+QMBNCFG="AutoSel"`
    #[serde(default)]
    pub query_auto_select: Option<String>,
    /// Set auto-select (template), e.g. `AT+QMBNCFG="AutoSel",{value}`
    #[serde(default)]
    pub set_auto_select: Option<String>,
    /// Query currently selected profile, e.g. `AT+QMBNCFG="Select"`
    #[serde(default)]
    pub query_selected: Option<String>,
    /// Select a profile (template), e.g. `AT+QMBNCFG="Select","{value}"`
    #[serde(default)]
    pub select_profile: Option<String>,
    /// Deactivate current profile, e.g. `AT+QMBNCFG="Deactivate"`
    #[serde(default)]
    pub deactivate: Option<String>,
}

// ============================================================================
// Live APN Read/Write Configuration (QICSGP)
// ============================================================================

/// AT-command templates for reading/writing the *live* APN directly on a PDP
/// context, used by the APN/PDP panel (Item #42).
///
/// Quectel modems expose `AT+QICSGP` for this; other vendors do not, so the
/// backend falls back to `AT+CGDCONT` when both fields are `None`. `None`/`None`
/// (the `Default`) means "this modem has no QICSGP support".
///
/// # Placeholder semantics (filled by the backend at runtime)
///
/// The query template uses `{cid}`; the write template uses
/// `{cid}`, `{context_type}`, `{apn}`, `{username}`, `{password}`, `{auth}`:
///
/// - `{cid}` — literal PDP context id (e.g. `1`).
/// - `{context_type}` — numeric Quectel context type derived from `IpType`:
///   `1` = IPv4, `2` = IPv6, `3` = IPv4v6.
/// - `{apn}` — literal APN string (placed inside the quotes in the template).
/// - `{username}` / `{password}` — literal auth credentials (inside quotes).
/// - `{auth}` — numeric auth method derived from `AuthType`:
///   `0` = none, `1` = PAP, `2` = CHAP.
///
/// Bench-confirmed field order (Phase 0, RM551E): `AT+QICSGP=1` returns
/// `+QICSGP: <context_type>,"<apn>","<user>","<pass>",<auth>`, which the write
/// template mirrors.
///
/// # Security
///
/// The backend MUST NOT log the *filled* write command — it contains the
/// PDP password (see the security rule "never log passwords"). Only the
/// query command (no secrets) may be traced.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ApnLiveConfig {
    /// Live-APN read template, e.g. `AT+QICSGP={cid}`.
    /// `None` = no QICSGP query support (fall back to CGDCONT).
    #[serde(default)]
    pub query: Option<String>,
    /// Live-APN write template, e.g.
    /// `AT+QICSGP={cid},{context_type},"{apn}","{username}","{password}",{auth}`.
    /// `None` = no QICSGP write support (fall back to CGDCONT).
    #[serde(default)]
    pub write: Option<String>,
}

// ============================================================================
// Dual SIM Configuration
// ============================================================================

/// Dual SIM slot management configuration for a modem model.
///
/// Data-driven: AT command templates for querying and switching SIM slots
/// are defined per-modem-profile. Quectel uses AT+QUIMSLOT; other vendors
/// may use different commands.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DualSimConfig {
    /// Whether dual SIM is supported by this modem.
    #[serde(default)]
    pub supported: bool,
    /// Number of SIM slots (typically 2).
    #[serde(default = "default_slot_count")]
    pub slot_count: u8,
    /// AT command to query active slot, e.g. `AT+QUIMSLOT?`
    #[serde(default)]
    pub query_slot_cmd: Option<String>,
    /// Regex to parse active slot from response, e.g. `\+Q?UIMSLOT:\s*(\d+)`
    /// Capture group 1 = slot number.
    #[serde(default)]
    pub query_slot_regex: Option<String>,
    /// AT command template to set active slot, e.g. `AT+QUIMSLOT={slot}`
    #[serde(default)]
    pub set_slot_cmd: Option<String>,
    /// AT command to query SIM initialization status, e.g. `AT+QINISTAT`
    #[serde(default)]
    pub sim_init_cmd: Option<String>,
    /// Regex to parse SIM init status bitmask, e.g. `\+QINISTAT:\s*(\d+)`
    /// Capture group 1 = bitmask (7 = fully initialized).
    #[serde(default)]
    pub sim_init_regex: Option<String>,
    /// Bitmask value that means fully initialized (e.g. 7 = CPIN+SMS+PB).
    #[serde(default = "default_sim_init_complete")]
    pub sim_init_complete_value: u8,
    /// Max time in seconds to wait for SIM init after slot switch.
    #[serde(default = "default_sim_init_timeout")]
    pub sim_init_timeout_secs: u64,
}

fn default_slot_count() -> u8 { 2 }
fn default_sim_init_complete() -> u8 { 7 }
fn default_sim_init_timeout() -> u64 { 30 }

// =============================================================================
// Carrier Aggregation Configuration
// =============================================================================

/// Band name prefix mapping for converting raw AT response band names to short form.
///
/// Example: `{ prefix: "LTE BAND ", replacement: "B" }` converts `"LTE BAND 7"` to `"B7"`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BandPrefixMapping {
    /// Raw prefix in AT response band name, e.g. `"LTE BAND "`
    pub prefix: String,
    /// Short replacement prefix, e.g. `"B"` (appended with the band number)
    pub replacement: String,
}

/// Carrier aggregation query configuration for a modem model.
///
/// Data-driven: AT command templates and parsing regexes for querying secondary
/// carrier components and network type are defined per-modem-profile.
/// Quectel uses AT+QCAINFO/AT+QNWINFO; other vendors will define their own.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CarrierAggregationConfig {
    /// Whether CA querying is supported by this modem.
    #[serde(default)]
    pub supported: bool,
    /// AT command to query carrier aggregation info (e.g. `"AT+QCAINFO"`).
    #[serde(default)]
    pub ca_info_cmd: Option<String>,
    /// Regex to parse LTE secondary component carriers from CA info response.
    /// Capture groups: 1=PCC/SCC, 2=earfcn, 3=bw, 4=band_name,
    /// 5=pcid, 6=rsrp, 7=rsrq, 8=rssi, 9=sinr.
    #[serde(default)]
    pub lte_scc_regex: Option<String>,
    /// Regex to parse NR5G secondary component carriers from CA info response.
    /// Capture groups: 1=PCC/SCC, 2=nrarfcn, 3=bw, 4=band_name,
    /// 5=remaining_fields (variable, split by comma post-capture).
    #[serde(default)]
    pub nr5g_scc_regex: Option<String>,
    /// AT command to query network type (e.g. `"AT+QNWINFO"`).
    #[serde(default)]
    pub network_type_cmd: Option<String>,
    /// Regex to parse network type from response.
    /// Capture group 1 = technology string (e.g. `"FDD LTE"`, `"NR5G-SA"`).
    #[serde(default)]
    pub network_type_regex: Option<String>,
    /// Band name prefix-to-short-form mappings.
    /// Applied in order; first matching prefix wins.
    #[serde(default)]
    pub band_prefix_mappings: Vec<BandPrefixMapping>,
    /// Parser variant for CA info response.
    /// "qcainfo" (default) = Quectel +QCAINFO format with regex-based parsing.
    /// "telit_cainfo" = Telit #CAINFO format with band_class encoding.
    #[serde(default = "default_ca_parser_variant")]
    pub ca_parser_variant: String,
}

fn default_ca_parser_variant() -> String { "qcainfo".to_string() }

impl Default for CarrierAggregationConfig {
    fn default() -> Self {
        Self {
            supported: false,
            ca_info_cmd: None,
            lte_scc_regex: None,
            nr5g_scc_regex: None,
            network_type_cmd: None,
            network_type_regex: None,
            band_prefix_mappings: vec![],
            ca_parser_variant: default_ca_parser_variant(),
        }
    }
}

// =============================================================================
// GPS Configuration
// =============================================================================

/// GPS command configuration for a modem profile.
///
/// Data-driven: AT commands, error code handling, and response parsing for GPS
/// GPS coordinate format returned by the modem.
///
/// Quectel modems (AT+QGPSLOC=2) return decimal degrees directly.
/// Telit modems (AT$GPSACP) return NMEA format (DDMM.MMMM) which needs conversion.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub enum GpsCoordinateFormat {
    /// Coordinates are already in decimal degrees (e.g. 37.387458).
    /// This is the default — existing Quectel behavior is preserved.
    #[default]
    Decimal,
    /// Coordinates are in NMEA format (DDMM.MMMM) and need conversion.
    /// The regex must also capture `ns` and `ew` named groups for hemisphere.
    Nmea,
}

/// GPS command configuration for a modem profile.
///
/// All GPS operations are defined per-modem-profile. Quectel uses AT+QGPS/AT+QGPSLOC/
/// AT+QGPSEND; other vendors may use different commands or have always-on GPS.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GpsConfig {
    /// Whether GPS is supported.
    #[serde(default)]
    pub supported: bool,

    /// AT command to start GPS engine (e.g. "AT+QGPS=1" for Quectel).
    /// None = GPS engine is always-on or doesn't need explicit start.
    #[serde(default)]
    pub start_cmd: Option<String>,

    /// Error codes from start_cmd that mean "already started" (not a real error).
    /// e.g. [504] for Quectel.
    #[serde(default)]
    pub start_already_running_codes: Vec<u32>,

    /// AT command to query GPS position (e.g. "AT+QGPSLOC=2" for Quectel).
    #[serde(default)]
    pub query_cmd: Option<String>,

    /// Error codes from query_cmd that mean "no fix yet" (return default GpsInfo).
    /// e.g. [516] for Quectel.
    #[serde(default)]
    pub no_fix_error_codes: Vec<u32>,

    /// Regex to parse GPS position response.
    /// Named capture groups: lat, lon, alt, speed, heading, fix, date, time, satellites
    /// (all optional — missing groups produce default values)
    #[serde(default)]
    pub query_regex: Option<String>,

    /// AT command to stop GPS engine (e.g. "AT+QGPSEND" for Quectel).
    #[serde(default)]
    pub stop_cmd: Option<String>,

    /// Error codes from stop_cmd that mean "already stopped" (not a real error).
    /// e.g. [505] for Quectel.
    #[serde(default)]
    pub stop_already_stopped_codes: Vec<u32>,

    /// If true, a bare ERROR response (no CME code) to the start command is
    /// treated as "GPS already running" — logged but not fatal.
    /// Needed for Telit FN990 which returns bare ERROR when GPS is already on.
    /// Default: false (bare ERROR is fatal).
    #[serde(default)]
    pub start_tolerates_bare_error: bool,

    /// Coordinate format returned by the GPS query response.
    /// Default: Decimal (existing Quectel behavior preserved).
    /// Set to Nmea for modems that return DDMM.MMMM format (e.g. Telit AT$GPSACP).
    #[serde(default)]
    pub coordinate_format: GpsCoordinateFormat,
}

// =============================================================================
// Antenna Metrics Configuration
// =============================================================================

/// Antenna metrics command configuration for a modem profile.
///
/// Data-driven: AT commands, sentinel values, and valid ranges for per-antenna
/// RSRP/SINR/RSRQ queries are defined per-modem-profile. Quectel uses
/// AT+QRSRP/AT+QSINR/AT+QRSRQ; other vendors may use different commands.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AntennaMetricsConfig {
    /// Whether per-antenna metrics are supported.
    #[serde(default)]
    pub supported: bool,

    /// AT command to query per-antenna RSRP (e.g. "AT+QRSRP" for Quectel).
    #[serde(default)]
    pub rsrp_cmd: Option<String>,

    /// AT command to query per-antenna SINR (e.g. "AT+QSINR" for Quectel).
    #[serde(default)]
    pub sinr_cmd: Option<String>,

    /// AT command to query per-antenna RSRQ (e.g. "AT+QRSRQ" for Quectel).
    #[serde(default)]
    pub rsrq_cmd: Option<String>,

    /// Sentinel value meaning "not available" (e.g. -32768 for Quectel).
    #[serde(default = "default_antenna_sentinel")]
    pub sentinel_value: i32,

    /// Valid RSRP range min (inclusive). Values outside this range are filtered.
    #[serde(default = "default_rsrp_min")]
    pub rsrp_min: i32,

    /// Valid RSRP range max (inclusive).
    #[serde(default = "default_rsrp_max")]
    pub rsrp_max: i32,

    /// Valid RSRQ range min (inclusive).
    #[serde(default = "default_rsrq_min")]
    pub rsrq_min: i32,

    /// Valid RSRQ range max (inclusive).
    #[serde(default = "default_rsrq_max")]
    pub rsrq_max: i32,

    /// Valid SINR range min (inclusive).
    #[serde(default = "default_sinr_min")]
    pub sinr_min: i32,

    /// Valid SINR range max (inclusive).
    #[serde(default = "default_sinr_max")]
    pub sinr_max: i32,

    /// When true, rsrp_cmd returns interleaved RSRP/RSRQ values per port:
    /// `rsrp_rx0, rsrq_rx0, rsrp_rx1, rsrq_rx1[, rsrp_rx2, rsrq_rx2, rsrp_rx3, rsrq_rx3]`
    /// In this mode, rsrq_cmd is ignored (RSRQ is extracted from the same response).
    /// Default false (Quectel-style: separate commands for RSRP, RSRQ, SINR).
    #[serde(default)]
    pub interleaved_rsrp_rsrq: bool,
}

fn default_antenna_sentinel() -> i32 { -32768 }
fn default_rsrp_min() -> i32 { -140 }
fn default_rsrp_max() -> i32 { -44 }
fn default_rsrq_min() -> i32 { -20 }
fn default_rsrq_max() -> i32 { 0 }
fn default_sinr_min() -> i32 { -23 }
fn default_sinr_max() -> i32 { 40 }

impl Default for AntennaMetricsConfig {
    fn default() -> Self {
        Self {
            supported: false,
            rsrp_cmd: None,
            sinr_cmd: None,
            rsrq_cmd: None,
            sentinel_value: default_antenna_sentinel(),
            rsrp_min: default_rsrp_min(),
            rsrp_max: default_rsrp_max(),
            rsrq_min: default_rsrq_min(),
            rsrq_max: default_rsrq_max(),
            sinr_min: default_sinr_min(),
            sinr_max: default_sinr_max(),
            interleaved_rsrp_rsrq: false,
        }
    }
}

// =============================================================================
// Signal Parsing Configuration
// =============================================================================

/// A signal response format variant.
/// The engine tries variants in order; first match wins.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalFormatVariant {
    /// Human-readable label for debug traces (e.g. "NR5G-NSA 3-line", "LTE single-line")
    pub label: String,

    /// Quick-check substring: if the response does NOT contain this string, skip this variant.
    /// Empty string = always try this variant.
    #[serde(default)]
    pub requires_substring: String,

    /// Regex to match and extract fields. Uses named capture groups:
    /// rsrp, rsrq, rssi, sinr, band, cellid (all optional — missing groups produce defaults).
    pub regex: String,

    /// Band prefix to prepend to the captured band number.
    /// "B" for LTE (band "14" -> "B14"), "n" for NR5G (band "77" -> "n77").
    #[serde(default = "default_band_prefix")]
    pub band_prefix: String,

    /// Technology: "4G" = Gen4, "5G" = Gen5.
    pub technology: String,
}

fn default_band_prefix() -> String { "B".into() }

fn default_restart_command() -> String { "AT+CFUN=1,1".to_string() }

/// Complete signal parsing configuration for a modem profile.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SignalParseConfig {
    /// Ordered list of response format variants to try. First match wins.
    /// Empty = use CSQ fallback only.
    #[serde(default)]
    pub variants: Vec<SignalFormatVariant>,
}

// =============================================================================
// Firmware Version Configuration
// =============================================================================

/// Firmware version query configuration for a modem model.
///
/// Different vendors use different AT commands to retrieve firmware version.
/// Quectel uses `AT+QGMR`, Telit uses `AT+CGMR`, others may vary.
/// When `firmware_cmd` is None, the handler falls back to the ATI Revision line.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FirmwareVersionConfig {
    /// Vendor-specific firmware version command (e.g. "AT+QGMR" for Quectel).
    /// None = use ATI Revision line as fallback only.
    #[serde(default)]
    pub firmware_cmd: Option<String>,

    /// Regex to parse firmware version response.
    /// Capture group 1 = version string.
    /// None = use the first non-empty, non-AT, non-OK line from the response.
    #[serde(default)]
    pub firmware_regex: Option<String>,
}

/// Additional AT commands to add to the whitelist for this modem.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProfileAtWhitelist {
    /// Commands safe to execute without confirmation
    #[serde(default)]
    pub safe_commands: Vec<String>,
    /// Commands that require user confirmation
    #[serde(default)]
    pub confirmation_commands: Vec<String>,
    /// Command prefixes that should be blocked
    #[serde(default)]
    pub blocked_prefixes: Vec<String>,
}

/// Complete modem profile defining behavior for a specific modem model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModemProfile {
    /// Modem identification (vendor/product ID, model name)
    pub identity: ModemIdentity,
    /// AT command mappings and parsing patterns
    pub commands: AtCommandSet,
    /// Serial port preferences
    pub port_mapping: PortMapping,
    /// Hardware capabilities
    pub capabilities: ModemCapabilities,
    /// Additional AT whitelist entries for this modem
    #[serde(default)]
    pub at_whitelist_additions: ProfileAtWhitelist,
    /// Short label for AT whitelist UI display, e.g. "RM551".
    /// If None, falls back to the model name from identity.
    #[serde(default)]
    pub whitelist_label: Option<String>,
    /// Band locking and network mode control configuration.
    /// None/default = band control not supported for this modem.
    #[serde(default)]
    pub band_mode_config: BandModeConfig,
    /// MBN carrier profile management configuration.
    /// Default = MBN management not supported for this modem.
    #[serde(default)]
    pub mbn_config: MbnConfig,
    /// APN profile apply sequence configuration.
    /// Default = APN profile apply not supported for this modem.
    #[serde(default)]
    pub apn_apply_config: ApnApplyConfig,
    /// Live APN read/write (QICSGP) command templates.
    /// Default = no QICSGP support; backend falls back to CGDCONT.
    #[serde(default)]
    pub apn_live_config: ApnLiveConfig,
    /// Dual SIM slot management configuration.
    /// Default = dual SIM not supported for this modem.
    #[serde(default)]
    pub dual_sim_config: DualSimConfig,
    /// Carrier aggregation query configuration.
    /// Default = CA querying not supported for this modem.
    #[serde(default)]
    pub ca_config: CarrierAggregationConfig,
    /// Firmware version query configuration.
    /// Default = ATI Revision fallback only (no vendor-specific command).
    #[serde(default)]
    pub firmware_config: FirmwareVersionConfig,
    /// GPS command configuration.
    /// Default = GPS not supported.
    #[serde(default)]
    pub gps_config: GpsConfig,
    /// Per-antenna metrics command configuration.
    /// Default = antenna metrics not supported.
    #[serde(default)]
    pub antenna_metrics_config: AntennaMetricsConfig,
    /// Signal parsing configuration with ordered format variants.
    /// Default = no vendor-specific parsing, CSQ fallback only.
    #[serde(default)]
    pub signal_parse_config: SignalParseConfig,
    /// Human-readable notes about this modem
    pub notes: Option<String>,
    /// AT command to reboot the modem when watchdog recovery is needed.
    /// Quectel: "AT+CFUN=1,1" (radio restart with reboot).
    /// Telit: "AT#REBOOT" (full device reboot).
    /// Default: "AT+CFUN=1,1" (safe for most modems).
    #[serde(default = "default_restart_command")]
    pub restart_command: String,
    /// USB-net mode detection. Default = no detection (mode stays `Unknown`).
    /// Diagnostic only — see `feedback_modem_mode_agnostic.md`.
    #[serde(default)]
    pub usbnet_detect: UsbNetDetectConfig,
}

impl ModemProfile {
    /// Returns true if this is the generic fallback profile.
    pub fn is_generic(&self) -> bool {
        self.identity.vendor_id == "0000" && self.identity.product_id == "0000"
    }

    /// A short identifier string for this profile, e.g. "quectel_rm551e_gl"
    pub fn profile_id(&self) -> String {
        if self.is_generic() {
            "generic".to_string()
        } else {
            format!(
                "{}_{}",
                self.identity.manufacturer.to_lowercase().replace(' ', "_"),
                self.identity.model.to_lowercase().replace(['-', ' '], "_")
            )
        }
    }
}

// ============================================================================
// Built-in Profiles
// ============================================================================

/// Returns all built-in modem profiles compiled into the binary.
pub fn builtin_profiles() -> Vec<ModemProfile> {
    vec![
        quectel_rm551e_gl(),
        quectel_rm520n_gl(),
        quectel_rm500q_gl(),
        telit_fn990(),
        // Future profiles:
        // sierra_em7455(),
        // etc.
    ]
}

/// Quectel RM551E-GL — primary target hardware.
///
/// 5G Sub-6 + LTE Cat 20 module. AT port on ttyUSB2.
/// Uses AT+QENG for detailed signal info, AT+QSPN for operator name.
fn quectel_rm551e_gl() -> ModemProfile {
    ModemProfile {
        identity: ModemIdentity {
            vendor_id: "2c7c".to_string(),
            product_id: "0122".to_string(),
            manufacturer: "Quectel".to_string(),
            model: "RM551E-GL".to_string(),
            model_variants: vec!["RM551E".to_string()],
        },
        commands: AtCommandSet {
            signal_cmd: Some(r#"AT+QENG="servingcell""#.to_string()),
            // Named capture groups for generic signal parser.
            // Matches: +QENG: "servingcell","NOCONN","LTE","FDD",310,410,<cellid>,123,<earfcn>,<band>,5,5,1A2B,<rsrp>,<rsrq>,<rssi>,<sinr>
            signal_parse_regex: Some(
                r#"\+QENG:\s*"servingcell","[^"]*","LTE","[^"]*",\d+,\d+,(?P<cellid>[0-9A-Fa-f]+),\d+,(?P<earfcn>\d+),(?P<band>\d+),\d+,\d+,[0-9A-Fa-f]+,(?P<rsrp>-?\d+),(?P<rsrq>-?\d+),(?P<rssi>-?\d+),(?P<sinr>-?\d+)"#.to_string()
            ),
            generic_signal_cmd: "AT+CSQ".to_string(),
            operator_name_cmd: Some("AT+QSPN".to_string()),
            // +QSPN: "AT&T","AT&T","",0,"310410"
            operator_name_regex: Some(r#"\+QSPN:\s*"([^"]*)""#.to_string()),
            generic_operator_cmd: "AT+COPS?".to_string(),
            generic_operator_regex: r#"\+COPS:\s*\d+,\d+,"([^"]*)""#.to_string(),
            iccid_cmd: "AT+QCCID".to_string(),
            // +QCCID: 8901410327...
            iccid_regex: Some(r"\+QCCID:\s*([0-9A-Fa-f]+)".to_string()),
            registration_cmd: "AT+CEREG?".to_string(),
        },
        port_mapping: PortMapping {
            at_port_preference: vec!["ttyUSB2".to_string(), "ttyUSB3".to_string()],
            // RM551E-GL AT port is on USB interface 2 (:1.2), modem/PPP on 3 (:1.3)
            at_interface_preference: vec![2, 3],
            baud_rate: 115200,
        },
        capabilities: ModemCapabilities {
            supports_5g: true,
            supports_carrier_aggregation: true,
            supported_technologies: vec![
                "2G".into(), "3G".into(), "4G".into(), "5G".into(),
            ],
            max_supported_bands: vec![
                // LTE bands
                "B1".into(), "B2".into(), "B3".into(), "B4".into(), "B5".into(),
                "B8".into(), "B12".into(), "B13".into(), "B14".into(), "B17".into(),
                "B18".into(), "B19".into(), "B20".into(), "B25".into(), "B26".into(),
                "B28".into(), "B29".into(), "B30".into(), "B32".into(), "B34".into(),
                "B39".into(), "B40".into(), "B41".into(), "B42".into(), "B43".into(),
                "B46".into(), "B48".into(), "B53".into(), "B66".into(), "B70".into(),
                "B71".into(),
                // NR bands (sub-6 + mmWave)
                "n1".into(), "n2".into(), "n3".into(), "n5".into(), "n7".into(),
                "n8".into(), "n12".into(), "n13".into(), "n14".into(), "n18".into(),
                "n20".into(), "n25".into(), "n26".into(), "n28".into(), "n29".into(),
                "n30".into(), "n38".into(), "n40".into(), "n41".into(), "n48".into(),
                "n53".into(), "n66".into(), "n70".into(), "n71".into(), "n75".into(),
                "n77".into(), "n78".into(), "n79".into(), "n91".into(), "n92".into(),
                "n93".into(), "n94".into(),
                // NR mmWave
                "n257".into(), "n258".into(), "n259".into(), "n260".into(), "n261".into(),
            ],
            supported_protocols: vec!["qmi".into(), "at".into(), "mbim".into()],
            has_temperature_sensor: true,
            has_gps: true,
        },
        at_whitelist_additions: ProfileAtWhitelist {
            safe_commands: vec![
                // Signal & network (read)
                r#"AT+QENG="servingcell""#.into(),
                r#"AT+QENG="neighbourcell""#.into(),
                "AT+QNWINFO".into(),
                "AT+QCAINFO".into(),
                "AT+QSPN".into(),
                "AT+QTEMP".into(),
                // Per-antenna metrics (read)
                "AT+QRSRP".into(),
                "AT+QSINR".into(),
                "AT+QRSRQ".into(),
                "AT+QCSQ".into(),
                // AMBR (read)
                r#"AT+QNWCFG="lte_ambr""#.into(),
                r#"AT+QNWCFG="nr5g_ambr""#.into(),
                // GPS (read)
                "AT+QGPSLOC=2".into(),
                "AT+QGPS?".into(),
                "AT+QGPSCFG?".into(),
                // Identification
                "AT+QGMR".into(),
                // MBN carrier profile queries (read-only)
                r#"AT+QMBNCFG="List""#.into(),
                r#"AT+QMBNCFG="AutoSel""#.into(),
                r#"AT+QMBNCFG="Select""#.into(),
                // Dual SIM queries (read-only)
                "AT+QUIMSLOT?".into(),
                "AT+QINISTAT".into(),
                "AT+QSIMSTAT?".into(),
                "AT+QPINC".into(),
                // Live APN read (QICSGP) — query on common PDP context CIDs.
                // Exact-match safe; writes fall through to the bare AT+QICSGP
                // confirmation prefix below.
                "AT+QICSGP=1".into(),
                "AT+QICSGP=2".into(),
                "AT+QICSGP=3".into(),
                "AT+QICSGP=4".into(),
                "AT+QICSGP=5".into(),
                "AT+QICSGP=6".into(),
                "AT+QICSGP=7".into(),
                "AT+QICSGP=8".into(),
            ],
            confirmation_commands: vec![
                // Power control
                "AT+QPOWD".into(),
                // GPS control (state-changing)
                "AT+QGPS".into(),
                "AT+QGPSEND".into(),
                "AT+QGPSCFG".into(),
                // Band selection
                "AT+QNWPREFCFG".into(),
                // MBN carrier profile changes (Select set, Deactivate, AutoSel set)
                "AT+QMBNCFG".into(),
                // SIM slot switching (writes NVM)
                "AT+QUIMSLOT".into(),
                // SIM insertion status reporting (writes NVM)
                "AT+QSIMSTAT".into(),
                // Live APN write (QICSGP=<cid>,<type>,...) — sets PDP context.
                "AT+QICSGP".into(),
            ],
            blocked_prefixes: vec![
                // Firmware/boot
                "AT+QFASTBOOT".into(),
                "AT+QDOWNLOAD".into(),
                // Factory reset
                "AT+QPRTPARA".into(),
                // NVRAM writes
                "AT+QNVW".into(),
                "AT+QNVFW".into(),
                // SIM detect config (hardware-dependent, requires reboot)
                "AT+QSIMDET".into(),
                // Quectel power down (use dedicated endpoint)
                "AT$QCPWRDN".into(),
                // Quectel DM mode
                "AT$QCDMG".into(),
            ],
        },
        whitelist_label: Some("RM551".to_string()),
        band_mode_config: BandModeConfig {
            supported: true,
            commands: BandModeCommands {
                query_mode: Some(r#"AT+QNWPREFCFG="mode_pref""#.into()),
                set_mode: Some(r#"AT+QNWPREFCFG="mode_pref",{value}"#.into()),
                query_nr5g_disable: Some(r#"AT+QNWPREFCFG="nr5g_disable_mode""#.into()),
                set_nr5g_disable: Some(r#"AT+QNWPREFCFG="nr5g_disable_mode",{value}"#.into()),
                query_lte_bands: Some(r#"AT+QNWPREFCFG="lte_band""#.into()),
                set_lte_bands: Some(r#"AT+QNWPREFCFG="lte_band",{value}"#.into()),
                query_nsa_bands: Some(r#"AT+QNWPREFCFG="nsa_nr5g_band""#.into()),
                set_nsa_bands: Some(r#"AT+QNWPREFCFG="nsa_nr5g_band",{value}"#.into()),
                query_sa_bands: Some(r#"AT+QNWPREFCFG="nr5g_band""#.into()),
                set_sa_bands: Some(r#"AT+QNWPREFCFG="nr5g_band",{value}"#.into()),
                query_nrdc_bands: Some(r#"AT+QNWPREFCFG="nrdc_nr5g_band""#.into()),
                set_nrdc_bands: Some(r#"AT+QNWPREFCFG="nrdc_nr5g_band",{value}"#.into()),
                query_nrdc_mode: Some(r#"AT+QNWPREFCFG="nrdc_mode""#.into()),
                set_nrdc_mode: Some(r#"AT+QNWPREFCFG="nrdc_mode",{value}"#.into()),
                query_all_bands: None,
                set_all_bands: None,
                restore_bands: Some(r#"AT+QNWPREFCFG="restore_band""#.into()),
            },
            modes: vec![
                NetworkModeOption {
                    id: "auto".into(),
                    label: "Auto".into(),
                    mode_value: "AUTO".into(),
                    nr5g_disable_value: Some(0),
                    active_sections: BandSections { lte: true, nsa: true, sa: true },
                },
                NetworkModeOption {
                    id: "lte".into(),
                    label: "LTE".into(),
                    mode_value: "LTE".into(),
                    nr5g_disable_value: Some(2),
                    active_sections: BandSections { lte: true, nsa: false, sa: false },
                },
                NetworkModeOption {
                    id: "5g_sa".into(),
                    label: "5G SA".into(),
                    mode_value: "NR5G".into(),
                    nr5g_disable_value: Some(2),
                    active_sections: BandSections { lte: false, nsa: false, sa: true },
                },
                NetworkModeOption {
                    id: "5g_nsa".into(),
                    label: "5G NSA".into(),
                    mode_value: "LTE:NR5G".into(),
                    nr5g_disable_value: Some(1),
                    active_sections: BandSections { lte: true, nsa: true, sa: false },
                },
                NetworkModeOption {
                    id: "auto_no_nsa".into(),
                    label: "Auto (no NSA)".into(),
                    mode_value: "AUTO".into(),
                    nr5g_disable_value: Some(2),
                    active_sections: BandSections { lte: true, nsa: false, sa: true },
                },
                NetworkModeOption {
                    id: "auto_no_sa".into(),
                    label: "Auto (no SA)".into(),
                    mode_value: "AUTO".into(),
                    nr5g_disable_value: Some(1),
                    active_sections: BandSections { lte: true, nsa: true, sa: false },
                },
            ],
            // Band lists from actual modem after AT+QNWPREFCFG="restore_band"
            lte_bands: vec![
                1, 2, 3, 4, 5, 7, 8, 12, 13, 14, 17, 18, 19, 20, 25, 26, 28,
                29, 30, 32, 34, 38, 39, 40, 41, 42, 43, 46, 48, 53, 66, 70, 71,
            ],
            nsa_nr5g_bands: vec![
                1, 2, 3, 5, 7, 8, 12, 13, 14, 18, 20, 25, 26, 28, 29, 30, 38,
                40, 41, 48, 53, 66, 70, 71, 75, 76, 77, 78, 79, 92, 94,
                257, 258, 260, 261,
            ],
            sa_nr5g_bands: vec![
                1, 2, 3, 5, 7, 8, 12, 13, 14, 18, 20, 25, 26, 28, 29, 30, 38,
                40, 41, 48, 53, 66, 70, 71, 75, 76, 77, 78, 79, 91, 92, 93, 94,
                257, 258, 260, 261,
            ],
            nrdc_nr5g_bands: vec![
                1, 2, 3, 5, 7, 8, 12, 13, 14, 18, 20, 25, 26, 28, 29, 30, 38,
                40, 41, 48, 53, 66, 70, 71, 75, 76, 77, 78, 79, 91, 92, 93, 94,
                257, 258, 260, 261,
            ],
            reboot_on_band_change: false,
            band_separator: ":".into(),
            band_command_variant: "per_type".into(),
        },
        mbn_config: MbnConfig {
            supported: true,
            commands: MbnCommands {
                list_profiles: Some(r#"AT+QMBNCFG="List""#.into()),
                query_auto_select: Some(r#"AT+QMBNCFG="AutoSel""#.into()),
                set_auto_select: Some(r#"AT+QMBNCFG="AutoSel",{value}"#.into()),
                query_selected: Some(r#"AT+QMBNCFG="Select""#.into()),
                select_profile: Some(r#"AT+QMBNCFG="Select","{value}""#.into()),
                deactivate: Some(r#"AT+QMBNCFG="Deactivate""#.into()),
            },
            reboot_recommended: true,
        },
        apn_apply_config: ApnApplyConfig {
            supported: true,
            steps: vec![
                ApnApplyStep {
                    label: "Deactivate MBN".into(),
                    command: r#"AT+QMBNCFG="Deactivate""#.into(),
                    requires_mbn: true,
                    timeout_secs: 10,
                },
                ApnApplyStep {
                    label: "Select MBN profile".into(),
                    command: r#"AT+QMBNCFG="Select","{mbn_profile}""#.into(),
                    requires_mbn: true,
                    timeout_secs: 10,
                },
                ApnApplyStep {
                    label: "Set APN on PDP context".into(),
                    command: r#"AT+CGDCONT={cid},"{ip_type}","{apn}""#.into(),
                    requires_mbn: false,
                    timeout_secs: 10,
                },
                ApnApplyStep {
                    label: "Disable MBN AutoSelect".into(),
                    command: r#"AT+QMBNCFG="AutoSel",0"#.into(),
                    requires_mbn: true,
                    timeout_secs: 10,
                },
            ],
            always_reboot: true,
            pre_reboot_delay_ms: 500,
        },
        apn_live_config: ApnLiveConfig {
            query: Some("AT+QICSGP={cid}".into()),
            write: Some(
                r#"AT+QICSGP={cid},{context_type},"{apn}","{username}","{password}",{auth}"#
                    .into(),
            ),
        },
        dual_sim_config: DualSimConfig {
            supported: true,
            slot_count: 2,
            query_slot_cmd: Some("AT+QUIMSLOT?".into()),
            // Manual example shows +QUSIMSLOT: 1 but documented format is +QUIMSLOT: <slot>
            // Handle both with optional "U" in the prefix
            query_slot_regex: Some(r"\+QU?IMSLOT:\s*(\d+)".into()),
            set_slot_cmd: Some("AT+QUIMSLOT={slot}".into()),
            sim_init_cmd: Some("AT+QINISTAT".into()),
            sim_init_regex: Some(r"\+QINISTAT:\s*(\d+)".into()),
            sim_init_complete_value: 7, // 1 (CPIN) + 2 (SMS) + 4 (PB) = fully init
            sim_init_timeout_secs: 30,
        },
        ca_config: CarrierAggregationConfig {
            supported: true,
            ca_info_cmd: Some("AT+QCAINFO".into()),
            lte_scc_regex: Some(
                r#"\+QCAINFO:\s*"(PCC|SCC)",(\d+),(\d+),"(LTE[^"]*)",\d+,(\d+),(-?\d+),(-?\d+),(-?\d+),(-?\d+)"#.into()
            ),
            nr5g_scc_regex: Some(
                r#"\+QCAINFO:\s*"(PCC|SCC)",(\d+),(\d+),"(NR5G[^"]*)"(?:,(.+))?"#.into()
            ),
            network_type_cmd: Some("AT+QNWINFO".into()),
            network_type_regex: Some(
                r#"\+QNWINFO:\s*"([^"]*)""#.into()
            ),
            band_prefix_mappings: vec![
                BandPrefixMapping { prefix: "LTE BAND ".into(), replacement: "B".into() },
                BandPrefixMapping { prefix: "NR5G BAND ".into(), replacement: "n".into() },
            ],
            ca_parser_variant: "qcainfo".into(),
        },
        firmware_config: FirmwareVersionConfig {
            firmware_cmd: Some("AT+QGMR".into()),
            firmware_regex: None, // first clean line extraction works for Quectel
        },
        gps_config: GpsConfig {
            supported: true,
            start_cmd: Some("AT+QGPS=1".into()),
            start_already_running_codes: vec![504],
            query_cmd: Some("AT+QGPSLOC=2".into()),
            no_fix_error_codes: vec![516],
            query_regex: Some(
                r"\+QGPSLOC:\s*(?P<time>\d+\.\d+),(?P<lat>[+-]?\d+\.\d+),(?P<lon>[+-]?\d+\.\d+),[^,]*,(?P<alt>[+-]?\d+\.?\d*),(?P<fix>\d),[^,]*,(?P<speed>\d+\.?\d*),[^,]*,(?P<date>\d+),(?P<satellites>\d+)".into()
            ),
            stop_cmd: Some("AT+QGPSEND".into()),
            stop_already_stopped_codes: vec![505],
            start_tolerates_bare_error: false,
            coordinate_format: GpsCoordinateFormat::Decimal,
        },
        antenna_metrics_config: AntennaMetricsConfig {
            supported: true,
            rsrp_cmd: Some("AT+QRSRP".into()),
            sinr_cmd: Some("AT+QSINR".into()),
            rsrq_cmd: Some("AT+QRSRQ".into()),
            sentinel_value: -32768,
            rsrp_min: -140, rsrp_max: -44,
            rsrq_min: -20, rsrq_max: 0,
            sinr_min: -23, sinr_max: 40,
            interleaved_rsrp_rsrq: false,
        },
        signal_parse_config: SignalParseConfig {
            variants: vec![
                // Variant 1: NR5G-NSA 3-line — MUST be first!
                // In NSA mode, LTE is always the primary anchor (PCC).
                // This captures the LTE anchor line, reports as 4G.
                SignalFormatVariant {
                    label: "NR5G-NSA 3-line".into(),
                    requires_substring: "NR5G-NSA".into(),
                    regex: r#"\+QENG:\s*"LTE","[^"]*",\d+,\d+,(?P<cellid>[0-9A-Fa-f]+),\d+,\d+,(?P<band>\d+),\d+,\d+,[0-9A-Fa-f]+,(?P<rsrp>-?\d+),(?P<rsrq>-?\d+),(?P<rssi>-?\d+),(?P<sinr>-?\d+)"#.into(),
                    band_prefix: "B".into(),
                    technology: "4G".into(),
                },
                // Variant 2: LTE single-line
                // +QENG: "servingcell","NOCONN","LTE","FDD",mcc,mnc,cellid,...
                SignalFormatVariant {
                    label: "LTE single-line".into(),
                    requires_substring: String::new(),
                    regex: r#"\+QENG:\s*"servingcell","[^"]*","LTE","[^"]*",\d+,\d+,(?P<cellid>[0-9A-Fa-f]+),\d+,(?P<earfcn>\d+),(?P<band>\d+),\d+,\d+,[0-9A-Fa-f]+,(?P<rsrp>-?\d+),(?P<rsrq>-?\d+),(?P<rssi>-?\d+),(?P<sinr>-?\d+)"#.into(),
                    band_prefix: "B".into(),
                    technology: "4G".into(),
                },
                // Variant 3: LTE 2-line (firmware variants that split the response)
                // Line 1: +QENG: "servingcell","NOCONN"
                // Line 2: +QENG: "LTE","FDD",mcc,mnc,...
                SignalFormatVariant {
                    label: "LTE 2-line".into(),
                    requires_substring: String::new(),
                    regex: r#"\+QENG:\s*"LTE","[^"]*",\d+,\d+,(?P<cellid>[0-9A-Fa-f]+),\d+,(?P<earfcn>\d+),(?P<band>\d+),\d+,\d+,[0-9A-Fa-f]+,(?P<rsrp>-?\d+),(?P<rsrq>-?\d+),(?P<rssi>-?\d+),(?P<sinr>-?\d+)"#.into(),
                    band_prefix: "B".into(),
                    technology: "4G".into(),
                },
                // Variant 4: NR5G-SA single-line (standalone 5G, no LTE anchor)
                // Fields after cellid can be hex (e.g. ARFCN) and firmware versions
                // may insert extra fields; use flexible intermediate matching.
                SignalFormatVariant {
                    label: "NR5G-SA single-line".into(),
                    requires_substring: "NR5G-SA".into(),
                    regex: r#"\+QENG:\s*"servingcell","[^"]*","NR5G-SA","[^"]*",\d+,\d+,(?P<cellid>[0-9A-Fa-f]+),[0-9A-Fa-f]+,(?:[0-9A-Fa-f]+,)*(?P<band>\d+),\d+,(?P<rsrp>-?\d+),(?P<rsrq>-?\d+),(?P<sinr>-?\d+)"#.into(),
                    band_prefix: "n".into(),
                    technology: "5G".into(),
                },
            ],
        },
        notes: Some(
            "Primary target hardware. AT port is ttyUSB2. Uses QMI for data, AT for control."
                .into(),
        ),
        restart_command: "AT+CFUN=1,1".to_string(),
        usbnet_detect: UsbNetDetectConfig {
            query_cmd: Some("AT+QCFG=\"usbnet\"".to_string()),
            parser: Some("quectel_qcfg_usbnet".to_string()),
        },
    }
}

/// Quectel RM520N-GL — 5G Sub-6 module.
///
/// Nearly identical to RM551E-GL in AT command support. Same Quectel AT+Q*
/// family: QENG, QRSRP, QSINR, QRSRQ, QCAINFO, QNWINFO, QNWPREFCFG,
/// QMBNCFG, QGPS, QGPSLOC, QGPSEND, QSPN, QCCID, QTEMP.
/// AT port on ttyUSB2 (USB interface 2, Quectel convention).
/// Differences from RM551E: different LTE/NR band lists, PID 0801.
fn quectel_rm520n_gl() -> ModemProfile {
    ModemProfile {
        identity: ModemIdentity {
            vendor_id: "2c7c".to_string(),
            product_id: "0801".to_string(),
            manufacturer: "Quectel".to_string(),
            model: "RM520N-GL".to_string(),
            model_variants: vec!["RM520N-GL".to_string(), "RM520N".to_string()],
        },
        commands: AtCommandSet {
            signal_cmd: Some(r#"AT+QENG="servingcell""#.to_string()),
            signal_parse_regex: Some(
                r#"\+QENG:\s*"servingcell","[^"]*","LTE","[^"]*",\d+,\d+,(?P<cellid>[0-9A-Fa-f]+),\d+,(?P<earfcn>\d+),(?P<band>\d+),\d+,\d+,[0-9A-Fa-f]+,(?P<rsrp>-?\d+),(?P<rsrq>-?\d+),(?P<rssi>-?\d+),(?P<sinr>-?\d+)"#.to_string()
            ),
            generic_signal_cmd: "AT+CSQ".to_string(),
            operator_name_cmd: Some("AT+QSPN".to_string()),
            operator_name_regex: Some(r#"\+QSPN:\s*"([^"]*)""#.to_string()),
            generic_operator_cmd: "AT+COPS?".to_string(),
            generic_operator_regex: r#"\+COPS:\s*\d+,\d+,"([^"]*)""#.to_string(),
            iccid_cmd: "AT+QCCID".to_string(),
            iccid_regex: Some(r"\+QCCID:\s*([0-9A-Fa-f]+)".to_string()),
            registration_cmd: "AT+CEREG?".to_string(),
        },
        port_mapping: PortMapping {
            at_port_preference: vec!["ttyUSB2".to_string(), "ttyUSB3".to_string()],
            at_interface_preference: vec![2, 3],
            baud_rate: 115200,
        },
        capabilities: ModemCapabilities {
            supports_5g: true,
            supports_carrier_aggregation: true,
            supported_technologies: vec![
                "2G".into(), "3G".into(), "4G".into(), "5G".into(),
            ],
            max_supported_bands: vec![
                // LTE bands (31 bands)
                "B1".into(), "B2".into(), "B3".into(), "B4".into(), "B5".into(),
                "B7".into(), "B8".into(), "B12".into(), "B13".into(), "B14".into(),
                "B17".into(), "B18".into(), "B19".into(), "B20".into(), "B25".into(),
                "B26".into(), "B28".into(), "B29".into(), "B30".into(), "B32".into(),
                "B34".into(), "B38".into(), "B39".into(), "B40".into(), "B41".into(),
                "B42".into(), "B43".into(), "B46".into(), "B48".into(), "B66".into(),
                "B71".into(),
                // NR bands (sub-6, 27 bands)
                "n1".into(), "n2".into(), "n3".into(), "n5".into(), "n7".into(),
                "n8".into(), "n12".into(), "n13".into(), "n14".into(), "n18".into(),
                "n20".into(), "n25".into(), "n26".into(), "n28".into(), "n29".into(),
                "n30".into(), "n38".into(), "n40".into(), "n48".into(), "n66".into(),
                "n70".into(), "n71".into(), "n75".into(), "n76".into(), "n77".into(),
                "n78".into(), "n79".into(),
            ],
            supported_protocols: vec!["qmi".into(), "at".into(), "mbim".into()],
            has_temperature_sensor: true,
            has_gps: true,
        },
        at_whitelist_additions: ProfileAtWhitelist {
            safe_commands: vec![
                // Signal & network (read)
                r#"AT+QENG="servingcell""#.into(),
                r#"AT+QENG="neighbourcell""#.into(),
                "AT+QNWINFO".into(),
                "AT+QCAINFO".into(),
                "AT+QSPN".into(),
                "AT+QTEMP".into(),
                // Per-antenna metrics (read)
                "AT+QRSRP".into(),
                "AT+QSINR".into(),
                "AT+QRSRQ".into(),
                "AT+QCSQ".into(),
                // AMBR (read)
                r#"AT+QNWCFG="lte_ambr""#.into(),
                r#"AT+QNWCFG="nr5g_ambr""#.into(),
                // GPS (read)
                "AT+QGPSLOC=2".into(),
                "AT+QGPS?".into(),
                "AT+QGPSCFG?".into(),
                // Identification
                "AT+QGMR".into(),
                // MBN carrier profile queries (read-only)
                r#"AT+QMBNCFG="List""#.into(),
                r#"AT+QMBNCFG="AutoSel""#.into(),
                r#"AT+QMBNCFG="Select""#.into(),
                // Dual SIM queries (read-only)
                "AT+QUIMSLOT?".into(),
                "AT+QINISTAT".into(),
                "AT+QSIMSTAT?".into(),
                "AT+QPINC".into(),
                // Live APN read (QICSGP) — query on common PDP context CIDs.
                // Exact-match safe; writes fall through to the bare AT+QICSGP
                // confirmation prefix below.
                "AT+QICSGP=1".into(),
                "AT+QICSGP=2".into(),
                "AT+QICSGP=3".into(),
                "AT+QICSGP=4".into(),
                "AT+QICSGP=5".into(),
                "AT+QICSGP=6".into(),
                "AT+QICSGP=7".into(),
                "AT+QICSGP=8".into(),
            ],
            confirmation_commands: vec![
                // Power control
                "AT+QPOWD".into(),
                // GPS control (state-changing)
                "AT+QGPS".into(),
                "AT+QGPSEND".into(),
                "AT+QGPSCFG".into(),
                // Band selection
                "AT+QNWPREFCFG".into(),
                // MBN carrier profile changes (Select set, Deactivate, AutoSel set)
                "AT+QMBNCFG".into(),
                // SIM slot switching (writes NVM)
                "AT+QUIMSLOT".into(),
                // SIM insertion status reporting (writes NVM)
                "AT+QSIMSTAT".into(),
                // Live APN write (QICSGP=<cid>,<type>,...) — sets PDP context.
                "AT+QICSGP".into(),
            ],
            blocked_prefixes: vec![
                // Firmware/boot
                "AT+QFASTBOOT".into(),
                "AT+QDOWNLOAD".into(),
                // Factory reset
                "AT+QPRTPARA".into(),
                // NVRAM writes
                "AT+QNVW".into(),
                "AT+QNVFW".into(),
                // SIM detect config (hardware-dependent, requires reboot)
                "AT+QSIMDET".into(),
                // Quectel power down (use dedicated endpoint)
                "AT$QCPWRDN".into(),
                // Quectel DM mode
                "AT$QCDMG".into(),
            ],
        },
        whitelist_label: Some("RM520N".to_string()),
        band_mode_config: BandModeConfig {
            supported: true,
            commands: BandModeCommands {
                query_mode: Some(r#"AT+QNWPREFCFG="mode_pref""#.into()),
                set_mode: Some(r#"AT+QNWPREFCFG="mode_pref",{value}"#.into()),
                query_nr5g_disable: Some(r#"AT+QNWPREFCFG="nr5g_disable_mode""#.into()),
                set_nr5g_disable: Some(r#"AT+QNWPREFCFG="nr5g_disable_mode",{value}"#.into()),
                query_lte_bands: Some(r#"AT+QNWPREFCFG="lte_band""#.into()),
                set_lte_bands: Some(r#"AT+QNWPREFCFG="lte_band",{value}"#.into()),
                query_nsa_bands: Some(r#"AT+QNWPREFCFG="nsa_nr5g_band""#.into()),
                set_nsa_bands: Some(r#"AT+QNWPREFCFG="nsa_nr5g_band",{value}"#.into()),
                query_sa_bands: Some(r#"AT+QNWPREFCFG="nr5g_band""#.into()),
                set_sa_bands: Some(r#"AT+QNWPREFCFG="nr5g_band",{value}"#.into()),
                query_nrdc_bands: Some(r#"AT+QNWPREFCFG="nrdc_nr5g_band""#.into()),
                set_nrdc_bands: Some(r#"AT+QNWPREFCFG="nrdc_nr5g_band",{value}"#.into()),
                query_nrdc_mode: Some(r#"AT+QNWPREFCFG="nrdc_mode""#.into()),
                set_nrdc_mode: Some(r#"AT+QNWPREFCFG="nrdc_mode",{value}"#.into()),
                query_all_bands: None,
                set_all_bands: None,
                restore_bands: Some(r#"AT+QNWPREFCFG="restore_band""#.into()),
            },
            modes: vec![
                NetworkModeOption {
                    id: "auto".into(),
                    label: "Auto".into(),
                    mode_value: "AUTO".into(),
                    nr5g_disable_value: Some(0),
                    active_sections: BandSections { lte: true, nsa: true, sa: true },
                },
                NetworkModeOption {
                    id: "lte".into(),
                    label: "LTE".into(),
                    mode_value: "LTE".into(),
                    nr5g_disable_value: Some(2),
                    active_sections: BandSections { lte: true, nsa: false, sa: false },
                },
                NetworkModeOption {
                    id: "5g_sa".into(),
                    label: "5G SA".into(),
                    mode_value: "NR5G".into(),
                    nr5g_disable_value: Some(2),
                    active_sections: BandSections { lte: false, nsa: false, sa: true },
                },
                NetworkModeOption {
                    id: "5g_nsa".into(),
                    label: "5G NSA".into(),
                    mode_value: "LTE:NR5G".into(),
                    nr5g_disable_value: Some(1),
                    active_sections: BandSections { lte: true, nsa: true, sa: false },
                },
                NetworkModeOption {
                    id: "auto_no_nsa".into(),
                    label: "Auto (no NSA)".into(),
                    mode_value: "AUTO".into(),
                    nr5g_disable_value: Some(2),
                    active_sections: BandSections { lte: true, nsa: false, sa: true },
                },
                NetworkModeOption {
                    id: "auto_no_sa".into(),
                    label: "Auto (no SA)".into(),
                    mode_value: "AUTO".into(),
                    nr5g_disable_value: Some(1),
                    active_sections: BandSections { lte: true, nsa: true, sa: false },
                },
            ],
            // Band lists from actual RM520N-GL hardware query
            lte_bands: vec![
                1, 2, 3, 4, 5, 7, 8, 12, 13, 14, 17, 18, 19, 20, 25, 26, 28,
                29, 30, 32, 34, 38, 39, 40, 41, 42, 43, 46, 48, 66, 71,
            ],
            nsa_nr5g_bands: vec![
                1, 2, 3, 5, 7, 8, 12, 13, 14, 18, 20, 25, 26, 28, 29, 30, 38,
                40, 48, 66, 70, 71, 75, 76, 77, 78, 79,
            ],
            sa_nr5g_bands: vec![
                1, 2, 3, 5, 7, 8, 12, 13, 14, 18, 20, 25, 26, 28, 29, 30, 38,
                40, 48, 66, 70, 71, 75, 76, 77, 78, 79,
            ],
            nrdc_nr5g_bands: vec![
                1, 2, 3, 5, 7, 8, 12, 13, 14, 18, 20, 25, 26, 28, 29, 30, 34,
                38, 39, 40, 41, 46, 48, 50, 51, 53, 65, 66, 70, 71, 74, 75, 76,
                77, 78, 79, 80, 81, 82, 83, 84, 85, 86, 91, 92, 93, 94,
                257, 258, 259, 260, 261,
            ],
            reboot_on_band_change: false,
            band_separator: ":".into(),
            band_command_variant: "per_type".into(),
        },
        mbn_config: MbnConfig {
            supported: true,
            commands: MbnCommands {
                list_profiles: Some(r#"AT+QMBNCFG="List""#.into()),
                query_auto_select: Some(r#"AT+QMBNCFG="AutoSel""#.into()),
                set_auto_select: Some(r#"AT+QMBNCFG="AutoSel",{value}"#.into()),
                query_selected: Some(r#"AT+QMBNCFG="Select""#.into()),
                select_profile: Some(r#"AT+QMBNCFG="Select","{value}""#.into()),
                deactivate: Some(r#"AT+QMBNCFG="Deactivate""#.into()),
            },
            reboot_recommended: true,
        },
        apn_apply_config: ApnApplyConfig {
            supported: true,
            steps: vec![
                ApnApplyStep {
                    label: "Deactivate MBN".into(),
                    command: r#"AT+QMBNCFG="Deactivate""#.into(),
                    requires_mbn: true,
                    timeout_secs: 10,
                },
                ApnApplyStep {
                    label: "Select MBN profile".into(),
                    command: r#"AT+QMBNCFG="Select","{mbn_profile}""#.into(),
                    requires_mbn: true,
                    timeout_secs: 10,
                },
                ApnApplyStep {
                    label: "Set APN on PDP context".into(),
                    command: r#"AT+CGDCONT={cid},"{ip_type}","{apn}""#.into(),
                    requires_mbn: false,
                    timeout_secs: 10,
                },
                ApnApplyStep {
                    label: "Disable MBN AutoSelect".into(),
                    command: r#"AT+QMBNCFG="AutoSel",0"#.into(),
                    requires_mbn: true,
                    timeout_secs: 10,
                },
            ],
            always_reboot: true,
            pre_reboot_delay_ms: 500,
        },
        apn_live_config: ApnLiveConfig {
            query: Some("AT+QICSGP={cid}".into()),
            write: Some(
                r#"AT+QICSGP={cid},{context_type},"{apn}","{username}","{password}",{auth}"#
                    .into(),
            ),
        },
        dual_sim_config: DualSimConfig {
            supported: true,
            slot_count: 2,
            query_slot_cmd: Some("AT+QUIMSLOT?".into()),
            query_slot_regex: Some(r"\+QU?IMSLOT:\s*(\d+)".into()),
            set_slot_cmd: Some("AT+QUIMSLOT={slot}".into()),
            sim_init_cmd: Some("AT+QINISTAT".into()),
            sim_init_regex: Some(r"\+QINISTAT:\s*(\d+)".into()),
            sim_init_complete_value: 7,
            sim_init_timeout_secs: 30,
        },
        ca_config: CarrierAggregationConfig {
            supported: true,
            ca_info_cmd: Some("AT+QCAINFO".into()),
            lte_scc_regex: Some(
                r#"\+QCAINFO:\s*"(PCC|SCC)",(\d+),(\d+),"(LTE[^"]*)",\d+,(\d+),(-?\d+),(-?\d+),(-?\d+),(-?\d+)"#.into()
            ),
            nr5g_scc_regex: Some(
                r#"\+QCAINFO:\s*"(PCC|SCC)",(\d+),(\d+),"(NR5G[^"]*)"(?:,(.+))?"#.into()
            ),
            network_type_cmd: Some("AT+QNWINFO".into()),
            network_type_regex: Some(
                r#"\+QNWINFO:\s*"([^"]*)""#.into()
            ),
            band_prefix_mappings: vec![
                BandPrefixMapping { prefix: "LTE BAND ".into(), replacement: "B".into() },
                BandPrefixMapping { prefix: "NR5G BAND ".into(), replacement: "n".into() },
            ],
            ca_parser_variant: "qcainfo".into(),
        },
        firmware_config: FirmwareVersionConfig {
            firmware_cmd: Some("AT+QGMR".into()),
            firmware_regex: None, // first clean line extraction works for Quectel
        },
        gps_config: GpsConfig {
            supported: true,
            start_cmd: Some("AT+QGPS=1".into()),
            start_already_running_codes: vec![504],
            query_cmd: Some("AT+QGPSLOC=2".into()),
            no_fix_error_codes: vec![516],
            query_regex: Some(
                r"\+QGPSLOC:\s*(?P<time>\d+\.\d+),(?P<lat>[+-]?\d+\.\d+),(?P<lon>[+-]?\d+\.\d+),[^,]*,(?P<alt>[+-]?\d+\.?\d*),(?P<fix>\d),[^,]*,(?P<speed>\d+\.?\d*),[^,]*,(?P<date>\d+),(?P<satellites>\d+)".into()
            ),
            stop_cmd: Some("AT+QGPSEND".into()),
            stop_already_stopped_codes: vec![505],
            start_tolerates_bare_error: false,
            coordinate_format: GpsCoordinateFormat::Decimal,
        },
        antenna_metrics_config: AntennaMetricsConfig {
            supported: true,
            rsrp_cmd: Some("AT+QRSRP".into()),
            sinr_cmd: Some("AT+QSINR".into()),
            rsrq_cmd: Some("AT+QRSRQ".into()),
            sentinel_value: -32768,
            rsrp_min: -140, rsrp_max: -44,
            rsrq_min: -20, rsrq_max: 0,
            sinr_min: -23, sinr_max: 40,
            interleaved_rsrp_rsrq: false,
        },
        signal_parse_config: SignalParseConfig {
            variants: vec![
                // Variant 1: NR5G-NSA 3-line — MUST be first!
                // In NSA mode, LTE is always the primary anchor (PCC).
                // This captures the LTE anchor line, reports as 4G.
                SignalFormatVariant {
                    label: "NR5G-NSA 3-line".into(),
                    requires_substring: "NR5G-NSA".into(),
                    regex: r#"\+QENG:\s*"LTE","[^"]*",\d+,\d+,(?P<cellid>[0-9A-Fa-f]+),\d+,\d+,(?P<band>\d+),\d+,\d+,[0-9A-Fa-f]+,(?P<rsrp>-?\d+),(?P<rsrq>-?\d+),(?P<rssi>-?\d+),(?P<sinr>-?\d+)"#.into(),
                    band_prefix: "B".into(),
                    technology: "4G".into(),
                },
                // Variant 2: LTE single-line
                SignalFormatVariant {
                    label: "LTE single-line".into(),
                    requires_substring: String::new(),
                    regex: r#"\+QENG:\s*"servingcell","[^"]*","LTE","[^"]*",\d+,\d+,(?P<cellid>[0-9A-Fa-f]+),\d+,(?P<earfcn>\d+),(?P<band>\d+),\d+,\d+,[0-9A-Fa-f]+,(?P<rsrp>-?\d+),(?P<rsrq>-?\d+),(?P<rssi>-?\d+),(?P<sinr>-?\d+)"#.into(),
                    band_prefix: "B".into(),
                    technology: "4G".into(),
                },
                // Variant 3: LTE 2-line (firmware variants that split the response)
                SignalFormatVariant {
                    label: "LTE 2-line".into(),
                    requires_substring: String::new(),
                    regex: r#"\+QENG:\s*"LTE","[^"]*",\d+,\d+,(?P<cellid>[0-9A-Fa-f]+),\d+,(?P<earfcn>\d+),(?P<band>\d+),\d+,\d+,[0-9A-Fa-f]+,(?P<rsrp>-?\d+),(?P<rsrq>-?\d+),(?P<rssi>-?\d+),(?P<sinr>-?\d+)"#.into(),
                    band_prefix: "B".into(),
                    technology: "4G".into(),
                },
                // Variant 4: NR5G-SA single-line (standalone 5G, no LTE anchor)
                // Fields after cellid can be hex (e.g. ARFCN) and firmware versions
                // may insert extra fields; use flexible intermediate matching.
                SignalFormatVariant {
                    label: "NR5G-SA single-line".into(),
                    requires_substring: "NR5G-SA".into(),
                    regex: r#"\+QENG:\s*"servingcell","[^"]*","NR5G-SA","[^"]*",\d+,\d+,(?P<cellid>[0-9A-Fa-f]+),[0-9A-Fa-f]+,(?:[0-9A-Fa-f]+,)*(?P<band>\d+),\d+,(?P<rsrp>-?\d+),(?P<rsrq>-?\d+),(?P<sinr>-?\d+)"#.into(),
                    band_prefix: "n".into(),
                    technology: "5G".into(),
                },
            ],
        },
        notes: Some(
            "Quectel RM520N-GL 5G Sub-6 module. Same AT command family as RM551E-GL. AT port is ttyUSB2."
                .into(),
        ),
        restart_command: "AT+CFUN=1,1".to_string(),
        usbnet_detect: UsbNetDetectConfig {
            query_cmd: Some("AT+QCFG=\"usbnet\"".to_string()),
            parser: Some("quectel_qcfg_usbnet".to_string()),
        },
    }
}

/// Quectel RM500Q-GL — 5G Sub-6 module (also covers RM500Q-AE variant).
///
/// Same Quectel AT command family as the RM551E-GL, but:
/// - No AT+QSINR or AT+QRSRQ (antenna metrics limited to RSRP only)
/// - No NRDC band support (sub-6 only, no mmWave)
/// - Different LTE/NR band lists
/// - AT port on ttyUSB2 (USB interface 2, Quectel convention).
fn quectel_rm500q_gl() -> ModemProfile {
    ModemProfile {
        identity: ModemIdentity {
            vendor_id: "2c7c".to_string(),
            product_id: "0800".to_string(),
            manufacturer: "Quectel".to_string(),
            model: "RM500Q-GL".to_string(),
            model_variants: vec!["RM500Q-GL".to_string(), "RM500Q-AE".to_string()],
        },
        commands: AtCommandSet {
            signal_cmd: Some(r#"AT+QENG="servingcell""#.to_string()),
            signal_parse_regex: Some(
                r#"\+QENG:\s*"servingcell","[^"]*","LTE","[^"]*",\d+,\d+,(?P<cellid>[0-9A-Fa-f]+),\d+,(?P<earfcn>\d+),(?P<band>\d+),\d+,\d+,[0-9A-Fa-f]+,(?P<rsrp>-?\d+),(?P<rsrq>-?\d+),(?P<rssi>-?\d+),(?P<sinr>-?\d+)"#.to_string()
            ),
            generic_signal_cmd: "AT+CSQ".to_string(),
            operator_name_cmd: Some("AT+QSPN".to_string()),
            operator_name_regex: Some(r#"\+QSPN:\s*"([^"]*)""#.to_string()),
            generic_operator_cmd: "AT+COPS?".to_string(),
            generic_operator_regex: r#"\+COPS:\s*\d+,\d+,"([^"]*)""#.to_string(),
            iccid_cmd: "AT+QCCID".to_string(),
            iccid_regex: Some(r"\+QCCID:\s*([0-9A-Fa-f]+)".to_string()),
            registration_cmd: "AT+CEREG?".to_string(),
        },
        port_mapping: PortMapping {
            at_port_preference: vec!["ttyUSB2".to_string(), "ttyUSB3".to_string()],
            at_interface_preference: vec![2, 3],
            baud_rate: 115200,
        },
        capabilities: ModemCapabilities {
            supports_5g: true,
            supports_carrier_aggregation: true,
            supported_technologies: vec![
                "2G".into(), "3G".into(), "4G".into(), "5G".into(),
            ],
            max_supported_bands: vec![
                // LTE bands (46 bands)
                "B1".into(), "B2".into(), "B3".into(), "B4".into(), "B5".into(),
                "B6".into(), "B7".into(), "B8".into(), "B9".into(), "B10".into(),
                "B11".into(), "B12".into(), "B13".into(), "B14".into(), "B17".into(),
                "B18".into(), "B19".into(), "B20".into(), "B21".into(), "B23".into(),
                "B24".into(), "B25".into(), "B26".into(), "B27".into(), "B28".into(),
                "B29".into(), "B30".into(), "B31".into(), "B32".into(), "B33".into(),
                "B34".into(), "B35".into(), "B36".into(), "B37".into(), "B38".into(),
                "B39".into(), "B40".into(), "B41".into(), "B42".into(), "B43".into(),
                "B46".into(), "B47".into(), "B48".into(), "B49".into(), "B66".into(),
                "B71".into(),
                // NR bands (sub-6 only, 19 bands — no mmWave)
                "n1".into(), "n2".into(), "n3".into(), "n5".into(), "n7".into(),
                "n8".into(), "n12".into(), "n20".into(), "n25".into(), "n28".into(),
                "n38".into(), "n40".into(), "n41".into(), "n48".into(), "n66".into(),
                "n71".into(), "n77".into(), "n78".into(), "n79".into(),
            ],
            supported_protocols: vec!["qmi".into(), "at".into(), "mbim".into()],
            has_temperature_sensor: true,
            has_gps: true,
        },
        at_whitelist_additions: ProfileAtWhitelist {
            safe_commands: vec![
                // Signal & network (read)
                r#"AT+QENG="servingcell""#.into(),
                r#"AT+QENG="neighbourcell""#.into(),
                "AT+QNWINFO".into(),
                "AT+QCAINFO".into(),
                "AT+QSPN".into(),
                "AT+QTEMP".into(),
                // Per-antenna metrics (read) — RSRP only, no QSINR/QRSRQ
                "AT+QRSRP".into(),
                "AT+QCSQ".into(),
                // AMBR (read)
                r#"AT+QNWCFG="lte_ambr""#.into(),
                r#"AT+QNWCFG="nr5g_ambr""#.into(),
                // GPS (read)
                "AT+QGPSLOC=2".into(),
                "AT+QGPS?".into(),
                "AT+QGPSCFG?".into(),
                // Identification
                "AT+QGMR".into(),
                // MBN carrier profile queries (read-only)
                r#"AT+QMBNCFG="List""#.into(),
                r#"AT+QMBNCFG="AutoSel""#.into(),
                r#"AT+QMBNCFG="Select""#.into(),
                // Dual SIM queries (read-only)
                "AT+QUIMSLOT?".into(),
                "AT+QINISTAT".into(),
                "AT+QSIMSTAT?".into(),
                "AT+QPINC".into(),
                // Live APN read (QICSGP) — query on common PDP context CIDs.
                // Exact-match safe; writes fall through to the bare AT+QICSGP
                // confirmation prefix below.
                "AT+QICSGP=1".into(),
                "AT+QICSGP=2".into(),
                "AT+QICSGP=3".into(),
                "AT+QICSGP=4".into(),
                "AT+QICSGP=5".into(),
                "AT+QICSGP=6".into(),
                "AT+QICSGP=7".into(),
                "AT+QICSGP=8".into(),
            ],
            confirmation_commands: vec![
                // Power control
                "AT+QPOWD".into(),
                // GPS control (state-changing)
                "AT+QGPS".into(),
                "AT+QGPSEND".into(),
                "AT+QGPSCFG".into(),
                // Band selection
                "AT+QNWPREFCFG".into(),
                // MBN carrier profile changes
                "AT+QMBNCFG".into(),
                // SIM slot switching (writes NVM)
                "AT+QUIMSLOT".into(),
                // SIM insertion status reporting (writes NVM)
                "AT+QSIMSTAT".into(),
                // Live APN write (QICSGP=<cid>,<type>,...) — sets PDP context.
                "AT+QICSGP".into(),
            ],
            blocked_prefixes: vec![
                // Firmware/boot
                "AT+QFASTBOOT".into(),
                "AT+QDOWNLOAD".into(),
                // Factory reset
                "AT+QPRTPARA".into(),
                // NVRAM writes
                "AT+QNVW".into(),
                "AT+QNVFW".into(),
                // SIM detect config (hardware-dependent, requires reboot)
                "AT+QSIMDET".into(),
                // Quectel power down (use dedicated endpoint)
                "AT$QCPWRDN".into(),
                // Quectel DM mode
                "AT$QCDMG".into(),
            ],
        },
        whitelist_label: Some("RM500Q".to_string()),
        band_mode_config: BandModeConfig {
            supported: true,
            commands: BandModeCommands {
                query_mode: Some(r#"AT+QNWPREFCFG="mode_pref""#.into()),
                set_mode: Some(r#"AT+QNWPREFCFG="mode_pref",{value}"#.into()),
                query_nr5g_disable: Some(r#"AT+QNWPREFCFG="nr5g_disable_mode""#.into()),
                set_nr5g_disable: Some(r#"AT+QNWPREFCFG="nr5g_disable_mode",{value}"#.into()),
                query_lte_bands: Some(r#"AT+QNWPREFCFG="lte_band""#.into()),
                set_lte_bands: Some(r#"AT+QNWPREFCFG="lte_band",{value}"#.into()),
                query_nsa_bands: Some(r#"AT+QNWPREFCFG="nsa_nr5g_band""#.into()),
                set_nsa_bands: Some(r#"AT+QNWPREFCFG="nsa_nr5g_band",{value}"#.into()),
                query_sa_bands: Some(r#"AT+QNWPREFCFG="nr5g_band""#.into()),
                set_sa_bands: Some(r#"AT+QNWPREFCFG="nr5g_band",{value}"#.into()),
                // NRDC not supported on RM500Q
                query_nrdc_bands: None,
                set_nrdc_bands: None,
                query_nrdc_mode: None,
                set_nrdc_mode: None,
                query_all_bands: None,
                set_all_bands: None,
                restore_bands: Some(r#"AT+QNWPREFCFG="restore_band""#.into()),
            },
            modes: vec![
                NetworkModeOption {
                    id: "auto".into(),
                    label: "Auto".into(),
                    mode_value: "AUTO".into(),
                    nr5g_disable_value: Some(0),
                    active_sections: BandSections { lte: true, nsa: true, sa: true },
                },
                NetworkModeOption {
                    id: "lte".into(),
                    label: "LTE".into(),
                    mode_value: "LTE".into(),
                    nr5g_disable_value: Some(2),
                    active_sections: BandSections { lte: true, nsa: false, sa: false },
                },
                NetworkModeOption {
                    id: "5g_sa".into(),
                    label: "5G SA".into(),
                    mode_value: "NR5G".into(),
                    nr5g_disable_value: Some(2),
                    active_sections: BandSections { lte: false, nsa: false, sa: true },
                },
                NetworkModeOption {
                    id: "5g_nsa".into(),
                    label: "5G NSA".into(),
                    mode_value: "LTE:NR5G".into(),
                    nr5g_disable_value: Some(1),
                    active_sections: BandSections { lte: true, nsa: true, sa: false },
                },
                NetworkModeOption {
                    id: "auto_no_nsa".into(),
                    label: "Auto (no NSA)".into(),
                    mode_value: "AUTO".into(),
                    nr5g_disable_value: Some(2),
                    active_sections: BandSections { lte: true, nsa: false, sa: true },
                },
                NetworkModeOption {
                    id: "auto_no_sa".into(),
                    label: "Auto (no SA)".into(),
                    mode_value: "AUTO".into(),
                    nr5g_disable_value: Some(1),
                    active_sections: BandSections { lte: true, nsa: true, sa: false },
                },
            ],
            // Band lists from actual RM500Q-GL hardware query
            lte_bands: vec![
                1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 17, 18, 19, 20,
                21, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37,
                38, 39, 40, 41, 42, 43, 46, 47, 48, 49, 66, 71,
            ],
            nsa_nr5g_bands: vec![
                1, 2, 3, 5, 7, 8, 12, 20, 25, 28, 38, 40, 41, 48, 66, 71,
                77, 78, 79,
            ],
            sa_nr5g_bands: vec![
                1, 2, 3, 5, 7, 8, 12, 20, 25, 28, 38, 40, 41, 48, 66, 71,
                77, 78, 79,
            ],
            // NRDC not supported — no mmWave bands
            nrdc_nr5g_bands: vec![],
            reboot_on_band_change: false,
            band_separator: ":".into(),
            band_command_variant: "per_type".into(),
        },
        mbn_config: MbnConfig {
            supported: true,
            commands: MbnCommands {
                list_profiles: Some(r#"AT+QMBNCFG="List""#.into()),
                query_auto_select: Some(r#"AT+QMBNCFG="AutoSel""#.into()),
                set_auto_select: Some(r#"AT+QMBNCFG="AutoSel",{value}"#.into()),
                query_selected: Some(r#"AT+QMBNCFG="Select""#.into()),
                select_profile: Some(r#"AT+QMBNCFG="Select","{value}""#.into()),
                deactivate: Some(r#"AT+QMBNCFG="Deactivate""#.into()),
            },
            reboot_recommended: true,
        },
        apn_apply_config: ApnApplyConfig {
            supported: true,
            steps: vec![
                ApnApplyStep {
                    label: "Deactivate MBN".into(),
                    command: r#"AT+QMBNCFG="Deactivate""#.into(),
                    requires_mbn: true,
                    timeout_secs: 10,
                },
                ApnApplyStep {
                    label: "Select MBN profile".into(),
                    command: r#"AT+QMBNCFG="Select","{mbn_profile}""#.into(),
                    requires_mbn: true,
                    timeout_secs: 10,
                },
                ApnApplyStep {
                    label: "Set APN on PDP context".into(),
                    command: r#"AT+CGDCONT={cid},"{ip_type}","{apn}""#.into(),
                    requires_mbn: false,
                    timeout_secs: 10,
                },
                ApnApplyStep {
                    label: "Disable MBN AutoSelect".into(),
                    command: r#"AT+QMBNCFG="AutoSel",0"#.into(),
                    requires_mbn: true,
                    timeout_secs: 10,
                },
            ],
            always_reboot: true,
            pre_reboot_delay_ms: 500,
        },
        apn_live_config: ApnLiveConfig {
            query: Some("AT+QICSGP={cid}".into()),
            write: Some(
                r#"AT+QICSGP={cid},{context_type},"{apn}","{username}","{password}",{auth}"#
                    .into(),
            ),
        },
        dual_sim_config: DualSimConfig {
            supported: true,
            slot_count: 2,
            query_slot_cmd: Some("AT+QUIMSLOT?".into()),
            query_slot_regex: Some(r"\+QU?IMSLOT:\s*(\d+)".into()),
            set_slot_cmd: Some("AT+QUIMSLOT={slot}".into()),
            sim_init_cmd: Some("AT+QINISTAT".into()),
            sim_init_regex: Some(r"\+QINISTAT:\s*(\d+)".into()),
            sim_init_complete_value: 7,
            sim_init_timeout_secs: 30,
        },
        ca_config: CarrierAggregationConfig {
            supported: true,
            ca_info_cmd: Some("AT+QCAINFO".into()),
            lte_scc_regex: Some(
                r#"\+QCAINFO:\s*"(PCC|SCC)",(\d+),(\d+),"(LTE[^"]*)",\d+,(\d+),(-?\d+),(-?\d+),(-?\d+),(-?\d+)"#.into()
            ),
            nr5g_scc_regex: Some(
                r#"\+QCAINFO:\s*"(PCC|SCC)",(\d+),(\d+),"(NR5G[^"]*)"(?:,(.+))?"#.into()
            ),
            network_type_cmd: Some("AT+QNWINFO".into()),
            network_type_regex: Some(
                r#"\+QNWINFO:\s*"([^"]*)""#.into()
            ),
            band_prefix_mappings: vec![
                BandPrefixMapping { prefix: "LTE BAND ".into(), replacement: "B".into() },
                BandPrefixMapping { prefix: "NR5G BAND ".into(), replacement: "n".into() },
            ],
            ca_parser_variant: "qcainfo".into(),
        },
        firmware_config: FirmwareVersionConfig {
            firmware_cmd: Some("AT+QGMR".into()),
            firmware_regex: None, // first clean line extraction works for Quectel
        },
        gps_config: GpsConfig {
            supported: true,
            start_cmd: Some("AT+QGPS=1".into()),
            start_already_running_codes: vec![504],
            query_cmd: Some("AT+QGPSLOC=2".into()),
            no_fix_error_codes: vec![516],
            query_regex: Some(
                r"\+QGPSLOC:\s*(?P<time>\d+\.\d+),(?P<lat>[+-]?\d+\.\d+),(?P<lon>[+-]?\d+\.\d+),[^,]*,(?P<alt>[+-]?\d+\.?\d*),(?P<fix>\d),[^,]*,(?P<speed>\d+\.?\d*),[^,]*,(?P<date>\d+),(?P<satellites>\d+)".into()
            ),
            stop_cmd: Some("AT+QGPSEND".into()),
            stop_already_stopped_codes: vec![505],
            start_tolerates_bare_error: false,
            coordinate_format: GpsCoordinateFormat::Decimal,
        },
        antenna_metrics_config: AntennaMetricsConfig {
            supported: true,
            rsrp_cmd: Some("AT+QRSRP".into()),
            sinr_cmd: None,  // AT+QSINR not supported on RM500Q
            rsrq_cmd: None,  // AT+QRSRQ not supported on RM500Q
            sentinel_value: -32768,
            rsrp_min: -140, rsrp_max: -44,
            rsrq_min: -20, rsrq_max: 0,
            sinr_min: -23, sinr_max: 40,
            interleaved_rsrp_rsrq: false,
        },
        signal_parse_config: SignalParseConfig {
            variants: vec![
                // Variant 1: NR5G-NSA 3-line — MUST be first!
                // In NSA mode, LTE is always the primary anchor (PCC).
                SignalFormatVariant {
                    label: "NR5G-NSA 3-line".into(),
                    requires_substring: "NR5G-NSA".into(),
                    regex: r#"\+QENG:\s*"LTE","[^"]*",\d+,\d+,(?P<cellid>[0-9A-Fa-f]+),\d+,\d+,(?P<band>\d+),\d+,\d+,[0-9A-Fa-f]+,(?P<rsrp>-?\d+),(?P<rsrq>-?\d+),(?P<rssi>-?\d+),(?P<sinr>-?\d+)"#.into(),
                    band_prefix: "B".into(),
                    technology: "4G".into(),
                },
                // Variant 2: LTE single-line
                SignalFormatVariant {
                    label: "LTE single-line".into(),
                    requires_substring: String::new(),
                    regex: r#"\+QENG:\s*"servingcell","[^"]*","LTE","[^"]*",\d+,\d+,(?P<cellid>[0-9A-Fa-f]+),\d+,(?P<earfcn>\d+),(?P<band>\d+),\d+,\d+,[0-9A-Fa-f]+,(?P<rsrp>-?\d+),(?P<rsrq>-?\d+),(?P<rssi>-?\d+),(?P<sinr>-?\d+)"#.into(),
                    band_prefix: "B".into(),
                    technology: "4G".into(),
                },
                // Variant 3: LTE 2-line (firmware variants that split the response)
                SignalFormatVariant {
                    label: "LTE 2-line".into(),
                    requires_substring: String::new(),
                    regex: r#"\+QENG:\s*"LTE","[^"]*",\d+,\d+,(?P<cellid>[0-9A-Fa-f]+),\d+,(?P<earfcn>\d+),(?P<band>\d+),\d+,\d+,[0-9A-Fa-f]+,(?P<rsrp>-?\d+),(?P<rsrq>-?\d+),(?P<rssi>-?\d+),(?P<sinr>-?\d+)"#.into(),
                    band_prefix: "B".into(),
                    technology: "4G".into(),
                },
                // Variant 4: NR5G-SA single-line (standalone 5G, no LTE anchor)
                // Fields after cellid can be hex (e.g. ARFCN) and firmware versions
                // may insert extra fields; use flexible intermediate matching.
                SignalFormatVariant {
                    label: "NR5G-SA single-line".into(),
                    requires_substring: "NR5G-SA".into(),
                    regex: r#"\+QENG:\s*"servingcell","[^"]*","NR5G-SA","[^"]*",\d+,\d+,(?P<cellid>[0-9A-Fa-f]+),[0-9A-Fa-f]+,(?:[0-9A-Fa-f]+,)*(?P<band>\d+),\d+,(?P<rsrp>-?\d+),(?P<rsrq>-?\d+),(?P<sinr>-?\d+)"#.into(),
                    band_prefix: "n".into(),
                    technology: "5G".into(),
                },
            ],
        },
        notes: Some(
            "Quectel RM500Q-GL 5G Sub-6 module. Same AT family as RM551E but no QSINR/QRSRQ. AT port is ttyUSB2."
                .into(),
        ),
        restart_command: "AT+CFUN=1,1".to_string(),
        usbnet_detect: UsbNetDetectConfig {
            query_cmd: Some("AT+QCFG=\"usbnet\"".to_string()),
            parser: Some("quectel_qcfg_usbnet".to_string()),
        },
    }
}

/// Telit FN990 — 5G Sub-6 module.
///
/// AT port on ttyUSB6 (bus-port 4-1.2 typically has 5 ports: ttyUSB4-8).
/// Uses standard 3GPP AT commands.
fn telit_fn990() -> ModemProfile {
    ModemProfile {
        identity: ModemIdentity {
            vendor_id: "1bc7".to_string(),
            product_id: "1073".to_string(),
            manufacturer: "Telit".to_string(),
            model: "FN990".to_string(),
            model_variants: vec!["FN990A".to_string()],
        },
        commands: AtCommandSet {
            signal_cmd: Some("AT#RFSTS".to_string()),
            signal_parse_regex: None,  // Uses signal_parse_config variants instead
            generic_signal_cmd: "AT+CSQ".to_string(),
            operator_name_cmd: None,  // Use generic AT+COPS
            operator_name_regex: None,
            generic_operator_cmd: "AT+COPS?".to_string(),
            generic_operator_regex: r#"\+COPS:\s*\d+,\d+,"([^"]*)""#.to_string(),
            iccid_cmd: "AT+ICCID".to_string(),
            iccid_regex: Some(r"\+ICCID:\s*(\d+)".to_string()),
            registration_cmd: "AT+CEREG?".to_string(),
        },
        port_mapping: PortMapping {
            // FN990 AT port is ttyUSB6 (interface 6), fallback to other interfaces
            at_port_preference: vec![
                "ttyUSB6".to_string(),
                "ttyUSB5".to_string(),
                "ttyUSB7".to_string(),
                "ttyUSB8".to_string(),
                "ttyUSB4".to_string(),
            ],
            at_interface_preference: vec![6, 5, 7, 8, 4],
            baud_rate: 115200,
        },
        capabilities: ModemCapabilities {
            supports_5g: true,
            supports_carrier_aggregation: true,
            supported_technologies: vec!["2G".into(), "3G".into(), "4G".into(), "5G".into()],
            max_supported_bands: vec![],  // TODO: populate with actual FN990 bands
            supported_protocols: vec!["at".into()],
            has_temperature_sensor: false,  // TODO: verify
            has_gps: true,
        },
        at_whitelist_additions: ProfileAtWhitelist {
            safe_commands: vec![
                // Signal query (read-only)
                "AT#RFSTS".into(),
                // ICCID query (read-only)
                "AT+ICCID".into(),
                // Per-antenna metrics (read-only, interleaved RSRP/RSRQ)
                "AT#LAPS".into(),
                "AT#NRAPS".into(),
                // Carrier aggregation info (read-only)
                "AT#CAINFO?".into(),
                // GPS (read)
                "AT$GPSACP".into(),
                "AT$GPSP?".into(),
                // Band query (read-only)
                "AT#BND?".into(),
                // Mode query (read-only)
                "AT+WS46?".into(),
            ],
            confirmation_commands: vec![
                // GPS control (state-changing)
                "AT$GPSP".into(),
                // Band selection (state-changing)
                "AT#BND".into(),
                // Mode selection (state-changing)
                "AT+WS46".into(),
            ],
            blocked_prefixes: vec![],
        },
        whitelist_label: Some("Telit".into()),
        band_mode_config: BandModeConfig {
            supported: true,
            commands: BandModeCommands {
                query_mode: Some("AT+WS46?".into()),
                set_mode: Some("AT+WS46={value}".into()),
                query_nr5g_disable: None,
                set_nr5g_disable: None,
                query_lte_bands: None,  // Telit uses query_all_bands instead
                set_lte_bands: None,
                query_nsa_bands: None,
                set_nsa_bands: None,
                query_sa_bands: None,
                set_sa_bands: None,
                query_nrdc_bands: None,
                set_nrdc_bands: None,
                query_nrdc_mode: None,
                set_nrdc_mode: None,
                query_all_bands: Some("AT#BND?".into()),
                set_all_bands: Some("AT#BND=0,0,{lte_low},{lte_high},{nsa_low},{nsa_high},{sa_low},{sa_high}".into()),
                // No dedicated restore command — restore by setting all supported bits
                restore_bands: Some("AT#BND=0,0,A7E2BB0F38DF,42,1A0290828D7,7042,81A03B0A38D7,7C42".into()),
            },
            modes: vec![
                NetworkModeOption {
                    id: "auto".into(),
                    label: "Auto".into(),
                    mode_value: "37".into(),
                    nr5g_disable_value: None,
                    active_sections: BandSections { lte: true, nsa: true, sa: true },
                },
                NetworkModeOption {
                    id: "lte".into(),
                    label: "LTE Only".into(),
                    mode_value: "28".into(),
                    nr5g_disable_value: None,
                    active_sections: BandSections { lte: true, nsa: false, sa: false },
                },
                NetworkModeOption {
                    id: "lte_nr".into(),
                    label: "LTE + NR (no SA)".into(),
                    mode_value: "36".into(),
                    nr5g_disable_value: None,
                    active_sections: BandSections { lte: true, nsa: true, sa: false },
                },
            ],
            // Decoded from AT#BND=? hex bitmasks
            lte_bands: vec![
                1, 2, 3, 4, 5, 7, 8, 12, 13, 14, 17, 18, 19, 20, 25, 26, 28,
                29, 30, 32, 34, 38, 39, 40, 41, 42, 43, 46, 48, 50, 55,
            ],
            nsa_nr5g_bands: vec![
                1, 2, 3, 5, 7, 8, 12, 14, 20, 25, 28, 30, 38, 40, 41, 66, 71,
                77, 78, 79,
            ],
            sa_nr5g_bands: vec![
                1, 2, 3, 5, 7, 8, 12, 13, 14, 18, 20, 25, 26, 28, 29, 30, 38,
                40, 41, 48, 66, 71, 75, 76, 77, 78, 79,
            ],
            nrdc_nr5g_bands: vec![],
            reboot_on_band_change: false,
            band_separator: ":".into(),
            band_command_variant: "telit_bnd".into(),
        },
        apn_apply_config: ApnApplyConfig {
            supported: false,  // Use generic connect flow
            steps: vec![],
            always_reboot: false,
            pre_reboot_delay_ms: 0,
        },
        // Telit has no QICSGP; backend falls back to CGDCONT.
        apn_live_config: ApnLiveConfig::default(),
        dual_sim_config: DualSimConfig {
            supported: false,  // TODO: verify if FN990 supports dual SIM
            slot_count: 1,
            query_slot_cmd: None,
            query_slot_regex: None,
            set_slot_cmd: None,
            sim_init_cmd: None,
            sim_init_regex: None,
            sim_init_complete_value: 0,
            sim_init_timeout_secs: 0,
        },
        mbn_config: MbnConfig {
            supported: false,  // Telit doesn't use Quectel MBN format
            commands: MbnCommands::default(),
            reboot_recommended: false,
        },
        ca_config: CarrierAggregationConfig {
            supported: true,
            ca_info_cmd: Some("AT#CAINFO?".into()),
            // Telit uses a completely different format — parsed by parse_telit_cainfo()
            lte_scc_regex: None,
            nr5g_scc_regex: None,
            network_type_cmd: None,  // Network type derived from CA band_class encoding
            network_type_regex: None,
            band_prefix_mappings: vec![], // Not used — Telit parser converts band_class directly
            ca_parser_variant: "telit_cainfo".into(),
        },
        firmware_config: FirmwareVersionConfig {
            firmware_cmd: Some("AT+CGMR".into()),
            firmware_regex: None, // first clean line extraction for standard 3GPP response
        },
        gps_config: GpsConfig {
            supported: true,
            start_cmd: Some("AT$GPSP=1".into()),
            start_already_running_codes: vec![],
            start_tolerates_bare_error: true,
            query_cmd: Some("AT$GPSACP".into()),
            no_fix_error_codes: vec![],
            query_regex: Some(
                r"\$GPSACP:\s*(?P<time>\d+\.\d+),(?P<lat>\d+\.\d+),(?P<ns>[NS]),(?P<lon>\d+\.\d+),(?P<ew>[EW]),[\d.]*,(?P<alt>[\d.]+),(?P<fix>\d+),[\d.]*,(?P<speed>[\d.]+),[\d.]*,(?P<date>\d+),(?P<satellites>\d+)".into()
            ),
            stop_cmd: Some("AT$GPSP=0".into()),
            stop_already_stopped_codes: vec![],
            coordinate_format: GpsCoordinateFormat::Nmea,
        },
        antenna_metrics_config: AntennaMetricsConfig {
            supported: true,
            rsrp_cmd: Some("AT#LAPS".into()),
            sinr_cmd: None, // Telit LAPS doesn't provide SINR
            rsrq_cmd: None, // RSRQ is interleaved in the LAPS response
            sentinel_value: -32768, // Telit sentinel (same as Quectel convention)
            rsrp_min: -140, rsrp_max: -44,
            rsrq_min: -20, rsrq_max: 0,
            sinr_min: -23, sinr_max: 40,
            interleaved_rsrp_rsrq: true,
        },
        signal_parse_config: SignalParseConfig {
            variants: vec![
                // Variant 1: LTE (also matches ENDC/NSA — captures LTE anchor as primary)
                // #RFSTS: "<PLMN>",<EARFCN>,<RSRP>,<RSSI>,<RSRQ>,<TAC>,<RAC>,[<TXPWR>],
                //         <DRX>,<MM>,<RRC>,<CID>,"<IMSI>","<NetName>",<SD>,<ABND>[,NR fields...]
                // NSA PCC rule: LTE present → LTE is always primary.
                SignalFormatVariant {
                    label: "Telit RFSTS LTE".into(),
                    requires_substring: "#RFSTS:".into(),
                    regex: concat!(
                        r#"#RFSTS:\s*"[^"]*","#,              // PLMN (quoted, skip)
                        r"(?P<earfcn>\d+),",                   // EARFCN
                        r"(?P<rsrp>-?\d+),",                   // RSRP
                        r"(?P<rssi>-?\d+),",                   // RSSI
                        r"(?P<rsrq>-?\d+),",                   // RSRQ
                        r"[0-9A-Fa-f]+,",                      // TAC (hex, skip)
                        r"[0-9A-Fa-f]+,",                      // RAC (hex/dec, skip)
                        r"-?\d*,",                             // TXPWR (optional, skip)
                        r"\d+,",                               // DRX (skip)
                        r"\d+,",                               // MM (skip)
                        r"\d+,",                               // RRC (skip)
                        r"(?P<cellid>[0-9A-Fa-f]+),",          // CID (hex)
                        r#""[^"]*","[^"]*","#,                 // IMSI + NetName (quoted, skip)
                        r"\d+,",                               // SD (skip)
                        r"(?P<band>\d+)",                      // ABND (band number)
                    ).into(),
                    band_prefix: "B".into(),
                    technology: "4G".into(),
                },
                // Variant 2: NR5G SA (standalone 5G, no LTE anchor)
                // #RFSTS: "<PLMN>",<NR_CH>,<NR_ULCH>,<NR_RSRP>,<NR_RSSI>,<NR_RSRQ>,
                //         <NR_BAND>,<NR_BW>,<NR_ULBW>[,<NR_TXPWR>]
                // Fewer fields, no quoted IMSI/NetName. Variant 1 won't match this.
                SignalFormatVariant {
                    label: "Telit RFSTS NR SA".into(),
                    requires_substring: "#RFSTS:".into(),
                    regex: concat!(
                        r#"#RFSTS:\s*"[^"]*","#,              // PLMN (quoted, skip)
                        r"\d+,",                               // NR_CH (skip)
                        r"\d+,",                               // NR_ULCH (skip)
                        r"(?P<rsrp>-?\d+),",                   // NR_RSRP
                        r"(?P<rssi>-?\d+),",                   // NR_RSSI
                        r"(?P<rsrq>-?\d+),",                   // NR_RSRQ
                        r"(?P<band>\d+),",                     // NR_BAND
                        r"\d+,",                               // NR_BW (skip)
                        r"\d+",                                // NR_ULBW (skip)
                    ).into(),
                    band_prefix: "n".into(),
                    technology: "5G".into(),
                },
            ],
        },
        notes: Some(
            "Telit FN990 5G module. AT port is ttyUSB6 (interface 6). Signal via AT#RFSTS.".into(),
        ),
        restart_command: "AT#REBOOT".to_string(),
        usbnet_detect: UsbNetDetectConfig {
            query_cmd: Some("AT#USBCFG?".to_string()),
            parser: Some("telit_usbcfg".to_string()),
        },
    }
}

/// Generic fallback profile using only 3GPP-standard AT commands.
///
/// Used when no specific profile matches the detected modem. Provides
/// basic functionality that should work with any AT-capable modem.
fn generic_modem() -> ModemProfile {
    ModemProfile {
        identity: ModemIdentity {
            vendor_id: "0000".to_string(),
            product_id: "0000".to_string(),
            manufacturer: "Generic".to_string(),
            model: "Unknown Modem".to_string(),
            model_variants: vec![],
        },
        commands: AtCommandSet {
            signal_cmd: None,
            signal_parse_regex: None,
            generic_signal_cmd: "AT+CSQ".to_string(),
            operator_name_cmd: None,
            operator_name_regex: None,
            generic_operator_cmd: "AT+COPS?".to_string(),
            generic_operator_regex: r#"\+COPS:\s*\d+,\d+,"([^"]*)""#.to_string(),
            iccid_cmd: "AT+CCID".to_string(),
            iccid_regex: None,
            registration_cmd: "AT+CEREG?".to_string(),
        },
        port_mapping: PortMapping {
            at_port_preference: vec![
                "ttyUSB2".to_string(),
                "ttyUSB1".to_string(),
                "ttyUSB0".to_string(),
                "ttyUSB3".to_string(),
            ],
            // Generic: try common AT interface positions (2, 3, 1, 0)
            at_interface_preference: vec![2, 3, 1, 0],
            baud_rate: 115200,
        },
        capabilities: ModemCapabilities {
            supports_5g: false,
            supports_carrier_aggregation: false,
            supported_technologies: vec!["2G".into(), "3G".into(), "4G".into()],
            max_supported_bands: vec![],
            supported_protocols: vec!["at".into()],
            has_temperature_sensor: false,
            has_gps: false,
        },
        at_whitelist_additions: ProfileAtWhitelist {
            safe_commands: vec![
                // Dual SIM queries (read-only) — needed for generic dual SIM support
                "AT+QUIMSLOT?".into(),
                "AT+QINISTAT".into(),
            ],
            confirmation_commands: vec![
                // SIM slot switching (writes NVM)
                "AT+QUIMSLOT".into(),
            ],
            ..Default::default()
        },
        whitelist_label: None,
        band_mode_config: BandModeConfig::default(), // supported: false
        mbn_config: MbnConfig::default(),              // supported: false
        apn_apply_config: ApnApplyConfig::default(), // supported: false
        apn_live_config: ApnLiveConfig::default(),   // no QICSGP; CGDCONT fallback
        dual_sim_config: DualSimConfig {
            supported: true,
            slot_count: 2,
            query_slot_cmd: Some("AT+QUIMSLOT?".into()),
            query_slot_regex: Some(r"\+QU?IMSLOT:\s*(\d+)".into()),
            set_slot_cmd: Some("AT+QUIMSLOT={slot}".into()),
            sim_init_cmd: Some("AT+QINISTAT".into()),
            sim_init_regex: Some(r"\+QINISTAT:\s*(\d+)".into()),
            sim_init_complete_value: 7,
            sim_init_timeout_secs: 30,
        },
        ca_config: CarrierAggregationConfig::default(), // supported: false
        firmware_config: FirmwareVersionConfig::default(), // ATI Revision fallback only
        gps_config: GpsConfig::default(), // supported: false
        antenna_metrics_config: AntennaMetricsConfig::default(), // supported: false
        signal_parse_config: SignalParseConfig::default(), // empty variants, falls through to CSQ
        notes: Some("Generic fallback profile. Uses only standard 3GPP AT commands.".into()),
        restart_command: default_restart_command(),
        usbnet_detect: UsbNetDetectConfig::default(),
    }
}

// ============================================================================
// Profile Registry
// ============================================================================

/// Registry of all known modem profiles (built-in + filesystem overrides).
pub struct ProfileRegistry {
    profiles: Vec<ModemProfile>,
    generic: ModemProfile,
}

impl ProfileRegistry {
    /// Load all profiles: built-in first, then filesystem overrides.
    ///
    /// Filesystem profiles in `/etc/modem-interface/profiles/*.toml` can
    /// add new profiles or override built-in ones by matching vendor_id + product_id.
    pub fn load() -> Self {
        let mut profiles = builtin_profiles();
        let profiles_dir = "/etc/modem-interface/profiles";

        // Load filesystem overrides
        if let Ok(entries) = std::fs::read_dir(profiles_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "toml") {
                    match std::fs::read_to_string(&path) {
                        Ok(content) => match toml::from_str::<ModemProfile>(&content) {
                            Ok(profile) => {
                                let vid = &profile.identity.vendor_id;
                                let pid = &profile.identity.product_id;
                                // Remove any existing profile with same vendor/product ID
                                profiles.retain(|p| {
                                    !(p.identity.vendor_id == *vid
                                        && p.identity.product_id == *pid)
                                });
                                info!(
                                    "Loaded profile override: {} {} from {}",
                                    profile.identity.manufacturer,
                                    profile.identity.model,
                                    path.display()
                                );
                                profiles.push(profile);
                            }
                            Err(e) => {
                                warn!("Failed to parse profile {}: {}", path.display(), e);
                            }
                        },
                        Err(e) => {
                            warn!("Failed to read profile {}: {}", path.display(), e);
                        }
                    }
                }
            }
        }

        let generic = generic_modem();
        info!(
            "Profile registry loaded: {} profile(s) (+ generic fallback)",
            profiles.len()
        );

        Self { profiles, generic }
    }

    /// Find the best matching profile for a modem by vendor and product ID.
    ///
    /// Returns the generic fallback profile if no specific match is found.
    pub fn match_profile(&self, vendor_id: &str, product_id: &str) -> &ModemProfile {
        // Normalize hex values: sysfs PRODUCT= may omit leading zeros (e.g. "122" vs "0122")
        let vid_num = u32::from_str_radix(vendor_id.trim(), 16).unwrap_or(0);
        let pid_num = u32::from_str_radix(product_id.trim(), 16).unwrap_or(0);

        self.profiles
            .iter()
            .find(|p| {
                let p_vid = u32::from_str_radix(&p.identity.vendor_id, 16).unwrap_or(u32::MAX);
                let p_pid = u32::from_str_radix(&p.identity.product_id, 16).unwrap_or(u32::MAX);
                p_vid == vid_num && p_pid == pid_num
            })
            .unwrap_or(&self.generic)
    }

    /// Find a profile by its profile_id string (e.g. "quectel_rm551e_gl").
    #[allow(dead_code)]
    pub fn find_by_id(&self, profile_id: &str) -> Option<&ModemProfile> {
        if profile_id == "generic" {
            return Some(&self.generic);
        }
        self.profiles.iter().find(|p| p.profile_id() == profile_id)
    }

    /// Get the generic fallback profile.
    pub fn generic(&self) -> &ModemProfile {
        &self.generic
    }

    /// List all specific (non-generic) profiles.
    pub fn all_profiles(&self) -> &[ModemProfile] {
        &self.profiles
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const QICSGP_QUERY: &str = "AT+QICSGP={cid}";
    const QICSGP_WRITE: &str =
        r#"AT+QICSGP={cid},{context_type},"{apn}","{username}","{password}",{auth}"#;

    /// All 3 Quectel profiles must expose populated QICSGP live-APN templates.
    #[test]
    fn quectel_profiles_expose_qicsgp_apn_live_config() {
        for profile in [
            quectel_rm551e_gl(),
            quectel_rm520n_gl(),
            quectel_rm500q_gl(),
        ] {
            assert_eq!(
                profile.apn_live_config.query.as_deref(),
                Some(QICSGP_QUERY),
                "{} query template",
                profile.identity.model
            );
            assert_eq!(
                profile.apn_live_config.write.as_deref(),
                Some(QICSGP_WRITE),
                "{} write template",
                profile.identity.model
            );
        }
    }

    /// Telit and generic profiles have no QICSGP support (None/None).
    #[test]
    fn non_quectel_profiles_have_no_apn_live_config() {
        for profile in [telit_fn990(), generic_modem()] {
            assert!(
                profile.apn_live_config.query.is_none(),
                "{} query must be None",
                profile.identity.model
            );
            assert!(
                profile.apn_live_config.write.is_none(),
                "{} write must be None",
                profile.identity.model
            );
        }
    }

    /// `ApnLiveConfig::default()` is None/None (CGDCONT fallback signal).
    #[test]
    fn apn_live_config_default_is_none_none() {
        let cfg = ApnLiveConfig::default();
        assert!(cfg.query.is_none());
        assert!(cfg.write.is_none());
    }

    /// The 3 Quectel whitelists must mark QICSGP queries on common CIDs as safe
    /// and the bare QICSGP prefix as confirmation-gated (writes).
    #[test]
    fn quectel_whitelists_cover_qicsgp_query_and_write() {
        for profile in [
            quectel_rm551e_gl(),
            quectel_rm520n_gl(),
            quectel_rm500q_gl(),
        ] {
            let wl = &profile.at_whitelist_additions;
            // Exact-match safe entries for the common PDP context CID range 1..=8.
            for cid in 1..=8 {
                let q = format!("AT+QICSGP={cid}");
                assert!(
                    wl.safe_commands.contains(&q),
                    "{} safe_commands must contain {q}",
                    profile.identity.model
                );
            }
            // Bare prefix in confirmation_commands gates every QICSGP write.
            assert!(
                wl.confirmation_commands.contains(&"AT+QICSGP".to_string()),
                "{} confirmation_commands must contain bare AT+QICSGP",
                profile.identity.model
            );
        }
    }
}
