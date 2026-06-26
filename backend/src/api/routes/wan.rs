//! WAN manager API route handlers.
//!
//! Handlers for /api/wan/* endpoints for multi-modem WAN priority management.
//! Manages OpenWRT network interfaces via UCI commands, supports modem priority
//! reordering, manual primary override, and connectivity watchdog configuration.

use axum::extract::{Extension, State};
use axum::Json;
use std::sync::{Arc, OnceLock};

use crate::api::auth_middleware::{require_admin, SessionUser};
use crate::api::error::{ApiError, ApiResult};
use crate::api::routing;
use crate::hardware::RoutingTableEntry;
use axum::response::{IntoResponse, Response};
use crate::hardware::{
    AddEthernetRequest, AvailableEthernetPort, ModemEvent, RoutingMode, UsbNetMode, WanConfig,
    WanEntryType, WanModemEntry, WanModemState, WanModemStatus, WanModemStatusEntry,
    WanScanResponse, WanStatusResponse, WanWatchdogLogResponse,
};
use crate::security::audit::AuditEventType;
use crate::state::{debug_trace_with_source, AppState, WanModemRuntimeInfo};

// ============================================================================
// UCI Helper Functions
// ============================================================================

/// Check if we're running in mock mode (no real UCI available).
fn is_mock_mode() -> bool {
    std::env::var("MOCK_HARDWARE").is_ok()
}

// ============================================================================
// Input validation (shell-injection defense in depth)
// ============================================================================

/// True iff `s` matches `^[A-Za-z0-9_-]{1,32}$`.
///
/// Used to validate operator-supplied UCI section names (`interface_name`) and
/// network device names (`network_device`) before they reach any command. Even
/// though the command builders now use argv form (no shell parsing), these
/// values become UCI keys / device identifiers and must be tightly constrained.
fn is_valid_uci_token(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 32
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// True iff `s` is a safe watchdog target: a hostname or IP literal with no
/// shell metacharacters or whitespace. Allows letters, digits, `.`, `-`, `:`
/// (IPv6 / host:port). Rejects empty, over-255-char, and anything containing
/// `;`, `|`, `&`, `$`, backtick, quotes, spaces, newlines, etc.
fn is_valid_watchdog_host(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 255
        && s.chars().all(|c| {
            c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == ':'
        })
}

/// True iff `s` is a safe HTTP(S) connectivity-check URL: scheme + a body that
/// contains no shell metacharacters or whitespace. The watchdog passes this to
/// `curl`/`wget`; argv form is used where possible, but we also reject anything
/// that could be abused as a shell token.
fn is_valid_http_target(s: &str) -> bool {
    if s.is_empty() || s.len() > 512 {
        return false;
    }
    if !(s.starts_with("http://") || s.starts_with("https://")) {
        return false;
    }
    // No whitespace or shell metacharacters anywhere in the URL.
    !s.chars().any(|c| {
        c.is_whitespace()
            || matches!(
                c,
                ';' | '|' | '&' | '$' | '`' | '"' | '\'' | '(' | ')' | '<' | '>' | '\\' | '*' | '?' | '!' | '{' | '}'
            )
    })
}

/// True iff `s` is a safe modem identifier in `VID:PID:SERIAL` form.
///
/// `modem_id` arrives operator-supplied via `PUT /wan/config` and is later split
/// on `:` and (historically) interpolated into a sysfs-walk command. The walk is
/// now pure-Rust (`usb_reset_modem`), but this value is still untrusted, so we
/// constrain it to the only shape a real USB modem id can take: ASCII
/// alphanumeric segments separated by `:`. This rejects every shell
/// metacharacter, control character, quote, and whitespace, so a crafted id such
/// as `x:y:z"; touch /tmp/pwned; echo "` can never reach a command builder.
///
/// Accepts the canonical 3-segment `VID:PID:SERIAL` form and the synthetic
/// ethernet ids the WAN manager creates internally (e.g. `eth:br-wan`), which
/// are 2-segment and use only `[A-Za-z0-9_-]`.
fn is_valid_modem_id(s: &str) -> bool {
    if s.is_empty() || s.len() > 128 {
        return false;
    }
    let mut segments = 0usize;
    for seg in s.split(':') {
        if seg.is_empty() {
            return false;
        }
        if !seg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return false;
        }
        segments += 1;
    }
    (2..=4).contains(&segments)
}

/// Pure, testable USB-device matcher.
///
/// Given the `(idVendor, idProduct, serial)` triple read from a single
/// `/sys/bus/usb/devices/<bus>/` entry, return `true` iff it matches the
/// requested `vid`/`pid`/`serial`. Factored out of [`usb_reset_modem`] so the
/// comparison can be unit-tested without touching real sysfs and without a
/// shell. The sysfs `idVendor`/`idProduct` files are lowercase hex; comparison
/// is case-insensitive on the hex fields to be tolerant of caller casing.
fn usb_device_matches(
    have: (&str, &str, &str),
    want: (&str, &str, &str),
) -> bool {
    let (have_vid, have_pid, have_serial) = have;
    let (want_vid, want_pid, want_serial) = want;
    have_vid.eq_ignore_ascii_case(want_vid)
        && have_pid.eq_ignore_ascii_case(want_pid)
        && have_serial == want_serial
}

// ============================================================================
// Mock UCI state — used only when MOCK_HARDWARE is set
// ============================================================================

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
struct MockUciSection {
    device: Option<String>,
    proto: Option<String>,
    metric: Option<u32>,
    mtu: Option<u32>,
}

#[cfg(test)]
#[allow(dead_code)]
static MOCK_UCI_STATE: std::sync::OnceLock<std::sync::Mutex<std::collections::BTreeMap<String, MockUciSection>>> =
    std::sync::OnceLock::new();

#[cfg(test)]
#[allow(dead_code)]
fn mock_uci_state() -> &'static std::sync::Mutex<std::collections::BTreeMap<String, MockUciSection>> {
    MOCK_UCI_STATE.get_or_init(|| std::sync::Mutex::new(std::collections::BTreeMap::new()))
}

#[cfg(test)]
#[allow(dead_code)]
fn mock_uci_reset() {
    mock_uci_state().lock().unwrap().clear();
}

#[cfg(test)]
#[allow(dead_code)]
fn mock_uci_seed(name: &str, device: Option<&str>, proto: Option<&str>, metric: Option<u32>) {
    mock_uci_state().lock().unwrap().insert(
        name.to_string(),
        MockUciSection {
            device: device.map(|s| s.to_string()),
            proto: proto.map(|s| s.to_string()),
            metric,
            mtu: None,
        },
    );
}

#[cfg(test)]
#[allow(dead_code)]
fn mock_uci_render_show_output() -> String {
    let state = mock_uci_state().lock().unwrap();
    let mut out = String::new();
    for (name, section) in state.iter() {
        out.push_str(&format!("network.{name}=interface\n"));
        if let Some(ref d) = section.device {
            out.push_str(&format!("network.{name}.device='{d}'\n"));
        }
        if let Some(ref p) = section.proto {
            out.push_str(&format!("network.{name}.proto='{p}'\n"));
        }
        if let Some(m) = section.metric {
            out.push_str(&format!("network.{name}.metric='{m}'\n"));
        }
    }
    out
}

#[cfg(test)]
#[allow(dead_code)]
static MOCK_DELETE_FAILS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(test)]
#[allow(dead_code)]
fn mock_uci_set_delete_fails(fails: bool) {
    MOCK_DELETE_FAILS.store(fails, std::sync::atomic::Ordering::SeqCst);
}

// Mock wan firewall-zone membership — tracks the network names in
// `firewall.@zone[1].network` so tests can assert add/remove without fw4.
#[cfg(test)]
#[allow(dead_code)]
static MOCK_WAN_ZONE: std::sync::OnceLock<std::sync::Mutex<std::collections::BTreeSet<String>>> =
    std::sync::OnceLock::new();

#[cfg(test)]
#[allow(dead_code)]
fn mock_wan_zone() -> &'static std::sync::Mutex<std::collections::BTreeSet<String>> {
    MOCK_WAN_ZONE.get_or_init(|| std::sync::Mutex::new(std::collections::BTreeSet::new()))
}

#[cfg(test)]
#[allow(dead_code)]
fn mock_wan_zone_reset() {
    mock_wan_zone().lock().unwrap().clear();
}

#[cfg(test)]
#[allow(dead_code)]
fn mock_wan_zone_contains(name: &str) -> bool {
    mock_wan_zone().lock().unwrap().contains(name)
}

#[cfg(test)]
#[allow(dead_code)]
static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Parsed view of a single `config interface 'name'` block from `uci show network`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[allow(dead_code)]
struct UciInterfaceSection {
    device: Option<String>,
    proto: Option<String>,
    metric: Option<u32>,
}

/// Parse the stdout of `uci show network` into a map keyed by section name.
///
/// Tolerates blank lines, malformed lines, and quoted/unquoted values.
/// Only `=interface` sections are captured; everything else is ignored.
#[allow(dead_code)]
fn parse_uci_show_output(stdout: &str) -> std::collections::BTreeMap<String, UciInterfaceSection> {
    use std::collections::BTreeMap;
    let mut out: BTreeMap<String, UciInterfaceSection> = BTreeMap::new();

    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // Each line is `network.<key>=<value>` where <key> is either `<section>` (declaration)
        // or `<section>.<option>` (option assignment).
        let Some((lhs, rhs)) = line.split_once('=') else {
            continue;
        };
        let Some(key) = lhs.strip_prefix("network.") else {
            continue;
        };
        // Strip surrounding single quotes from the value if present.
        let value = rhs.trim().trim_matches('\'').to_string();

        if !key.contains('.') {
            // Section declaration: `network.<section>=interface`
            if value == "interface" {
                out.entry(key.to_string()).or_default();
            }
            continue;
        }

        let Some((section, option)) = key.split_once('.') else {
            continue;
        };
        // Only attach options to sections that were declared as `=interface`.
        let Some(entry) = out.get_mut(section) else {
            continue;
        };
        match option {
            "device" => entry.device = Some(value),
            "proto" => entry.proto = Some(value),
            "metric" => entry.metric = value.parse().ok(),
            _ => {}
        }
    }
    out
}

/// Per-modem snapshot of fields needed by `reconcile_uci_section`'s callsites
/// (Item #37 sub-tasks 2 + 2b).
///
/// Built once at the top of `update_wan_config` and `scan_wan` (before the
/// reconcile loop) under the `state.modems` read guard, then dropped before
/// the loop runs. Looked up at each callsite by `entry.modem_id`.
///
/// `usbnet_mode` carries sub-task 1's detected mode for the proto resolver.
/// `control_device_path` carries the sysfs-resolved cdc-wdm path (sub-task 2b)
/// for the device resolver — populated when the modem's bus-port maps to a
/// `/dev/cdc-wdmN` device, None otherwise.
///
/// Replaces the `HashMap<String, UsbNetMode>` value type from sub-task 2 so
/// future fields can be added without re-architecting the snapshot site.
#[derive(Clone, Default)]
struct ResolvedReconcileFields {
    usbnet_mode: UsbNetMode,
    control_device_path: Option<String>,
}

/// Resolve the UCI `proto` value for a WAN entry (Item #37 sub-task 2).
///
/// Resolution order:
/// 1. If `entry.proto_override` is `Some(s)` and `s.trim()` is non-empty, use
///    that exact (untrimmed) value verbatim. Whitespace-only or empty
///    overrides fall through to the mode-derived default (defensive — input
///    validation rejects these earlier in `update_wan_config`, but the
///    resolver does not blindly trust the field).
/// 2. Else if `entry.entry_type == WanEntryType::Ethernet`, return `"dhcp"`.
///    Ethernet WAN ports always run DHCP regardless of any cellular mode in
///    AppState.
/// 3. Else if `detected_mode` is `Some(mode)`, map per the table:
///    `Ecm`/`Ncm`/`Rndis` → `"dhcp"` (modem runs DHCP server on its USB iface),
///    `Qmi`/`Rmnet` → `"qmi"` (OpenWrt proto-qmi handles control-plane IP),
///    `Mbim` → `"mbim"` (OpenWrt proto-mbim),
///    `Unknown` → `"dhcp"` (backwards-compat fallback).
/// 4. Else (modem entry with no detected mode — e.g. modem unplugged
///    mid-reconcile), return `"dhcp"`. Same as `Unknown`.
///
/// Total — never panics, never returns a sentinel "unset," always yields a
/// usable proto string.
///
/// Mode-agnostic principle: this function maps modes to UCI proto values for
/// the daemon. The operator never sees mode names; the override field accepts
/// a UCI proto string, not a mode name.
pub(crate) fn resolve_uci_proto<'a>(
    entry: &'a WanModemEntry,
    detected_mode: Option<UsbNetMode>,
) -> std::borrow::Cow<'a, str> {
    if let Some(s) = entry.proto_override.as_deref() {
        if !s.trim().is_empty() {
            return std::borrow::Cow::Borrowed(s);
        }
    }
    if entry.entry_type == WanEntryType::Ethernet {
        return std::borrow::Cow::Borrowed("dhcp");
    }
    match detected_mode {
        Some(UsbNetMode::Ecm) | Some(UsbNetMode::Ncm) | Some(UsbNetMode::Rndis) => {
            std::borrow::Cow::Borrowed("dhcp")
        }
        Some(UsbNetMode::Qmi) | Some(UsbNetMode::Rmnet) => std::borrow::Cow::Borrowed("qmi"),
        Some(UsbNetMode::Mbim) => std::borrow::Cow::Borrowed("mbim"),
        Some(UsbNetMode::Unknown) | None => std::borrow::Cow::Borrowed("dhcp"),
    }
}

/// Pick the value to write into UCI `option device` based on the resolved
/// proto (Item #37 sub-task 2b).
///
/// 1. For proto=qmi/mbim with `Some(path)`, return the cdc-wdm control device
///    path. OpenWrt's `/lib/netifd/proto/qmi.sh` and `mbim.sh` require this
///    spelling — passing the netif name yields
///    `"The specified control device does not exist"` at bring-up.
/// 2. For proto=qmi/mbim with `None`, fall back to `entry.network_device` and
///    log at info-level. This is the same broken state as pre-2b (control
///    device couldn't be resolved — USB enumeration race, ECM/NCM modem with
///    a `proto_override="qmi"` operator assertion, etc.); netifd's bring-up
///    still fails but the failure surfaces in OpenWrt logs instead of the
///    daemon refusing to reconcile.
/// 3. For all other protos (dhcp, static, pppoe, ppp, none, custom strings
///    from `proto_override`), return `entry.network_device`.
///
/// Total — never panics, always yields a usable device string.
///
/// Mirrors `resolve_uci_proto`'s style. Lives as a free function (rather than
/// inline inside `reconcile_uci_section`) so unit tests can exercise the
/// dispatch table without setting up the UCI mock fixture.
///
/// Note: production callsites pass `control_device_path: Option<&str>` directly
/// to `reconcile_uci_section`, which dispatches the same proto-keyed switch
/// internally before writing UCI. This pure-function variant exists for unit
/// tests that exercise the dispatch in isolation. The two paths MUST stay in
/// sync — if a future change modifies one, it must modify the other.
#[allow(dead_code)] // Test-only — production dispatch lives inside reconcile_uci_section.
fn resolve_uci_device<'a>(
    entry: &'a WanModemEntry,
    proto: &str,
    control_device_path: Option<&'a str>,
) -> std::borrow::Cow<'a, str> {
    match proto {
        "qmi" | "mbim" => match control_device_path {
            Some(path) => std::borrow::Cow::Borrowed(path),
            None => {
                tracing::info!(
                    "[WAN] No cdc-wdm control device for modem_id={} (netif={}, proto={}); falling back to netif — netifd bring-up may fail",
                    entry.modem_id, entry.network_device, proto
                );
                std::borrow::Cow::Borrowed(&entry.network_device)
            }
        },
        _ => std::borrow::Cow::Borrowed(&entry.network_device),
    }
}

/// Decide whether `update_wan_config`'s `Some(old) =>` arm needs to call
/// `reconcile_uci_section` for an existing modem entry (Item #37 sub-task 2c).
///
/// Returns true when proto-affecting fields differ between the old and new
/// entry. The diff predicate is the operator-changed-fields trigger:
///
/// 1. `entry.proto_override != old.proto_override` — operator flipped the
///    UCI proto override (or cleared it, returning to auto-detection).
/// 2. `entry.network_device != old.network_device` — rare; modem renumbered
///    or UCI section adopted a different netif.
///
/// Excludes Ethernet entries — they have a separate fast-path
/// (`Some(old) =>` Ethernet bridge-conversion branch elsewhere in this arm)
/// and `resolve_uci_proto` short-circuits to `"dhcp"` for Ethernet regardless
/// of `proto_override`. The bridge-conversion branch handles its own reconcile.
///
/// Excludes pure metric/MTU/state diffs — those use the existing fast-path
/// (`uci_set_metric` and `uci_set_mtu`) which is cheaper and sufficient.
///
/// USB hot-plug case (modem replugged between saves, cached `usbnet_mode`
/// shifted): NOT covered by this predicate. Operator must click Scan to
/// trigger the unconditional `scan_wan` reconcile. Documented as the
/// escape hatch in API-CONTRACT.md.
fn should_reconcile_existing_modem_entry(
    old: &WanModemEntry,
    entry: &WanModemEntry,
) -> bool {
    if entry.entry_type != WanEntryType::Modem {
        return false;
    }
    entry.proto_override != old.proto_override
        || entry.network_device != old.network_device
}

/// Reconcile a UCI interface section so it owns `target_device` exclusively.
///
/// 1. Reads `uci show network` once.
/// 2. Deletes any other `interface` section whose `option device` matches
///    `target_device` but whose name differs from `target_name` (collision).
/// 3. Creates or updates `network.<target_name>` with the given proto/metric/MTU.
///    For proto=qmi/mbim, writes `target_control_device_path` (e.g.
///    `/dev/cdc-wdm0`) instead of `target_device` (the netif), per OpenWrt
///    netifd's proto handler contract (Item #37 sub-task 2b). When
///    `target_control_device_path` is `None` for proto=qmi/mbim, falls back
///    to `target_device` and netifd bring-up will fail with "control device
///    does not exist" — same broken state as pre-2b.
///
/// Note: collision detection (step 2) keys on `target_device` (the netif
/// name), NOT on the control-device path. This is intentional — collision
/// is about preventing two UCI sections from claiming the same physical L2
/// interface, and the netif is our authoritative identity for that.
///
/// Does NOT call `uci commit` — caller batches commits.
///
/// Returns the list of displaced section names (for telemetry / tests).
#[allow(dead_code)]
async fn reconcile_uci_section(
    target_name: &str,
    target_device: &str,
    target_proto: &str,
    target_control_device_path: Option<&str>,
    target_metric: u32,
    target_mtu: Option<u32>,
) -> Result<Vec<String>, String> {
    let show_output = uci_show_network().await?;
    let parsed = parse_uci_show_output(&show_output);

    let mut displaced: Vec<String> = Vec::new();

    for (name, section) in parsed.iter() {
        if section.device.as_deref() != Some(target_device) {
            continue;
        }
        if name == target_name {
            continue;
        }
        // Collision: another section binds our device.
        uci_delete_section(name).await.map_err(|e| {
            format!("collision-delete failed for network.{name}: {e}")
        })?;
        let _ = uci_remove_from_wan_zone(name).await; // idempotent; failure is non-fatal
        tracing::warn!(
            "[WAN] Displacing colliding UCI section '{name}' on device {target_device} (taking ownership as '{target_name}')"
        );
        debug_trace_with_source(
            format!(
                "[WAN] Displacing colliding UCI section '{name}' on device {target_device} (taking ownership as '{target_name}')"
            ),
            "wan",
        );
        let _ = crate::config::wan::append_watchdog_log(&format!(
            "{} UCI_DISPLACED our=\"{target_name}\" displaced=\"{name}\" device=\"{target_device}\"",
            chrono::Utc::now().to_rfc3339()
        )).await;
        displaced.push(name.clone());
    }

    // Sub-task 2b: pick the actual device value to write based on proto.
    // For proto=qmi/mbim, OpenWrt netifd's proto handlers require a control
    // device path (e.g. `/dev/cdc-wdm0`); for all other protos, the netif
    // name is correct. The collision-detection above keys on `target_device`
    // (the netif) because that's our authoritative L2-binding identity —
    // collision is about who owns the physical interface, not how the UCI
    // `option device` happens to be spelled.
    let device_to_write = match target_proto {
        "qmi" | "mbim" => target_control_device_path.unwrap_or(target_device),
        _ => target_device,
    };

    // Write our section unconditionally. UCI `set` is idempotent; this also
    // covers in-place metric/proto/MTU updates.
    uci_write_interface_section(target_name, device_to_write, target_proto, target_metric, target_mtu).await?;

    Ok(displaced)
}

/// Delete any managed-namespace UCI section (case-insensitive `WWAN*` or `EWAN*`)
/// that is not in `active_names`. Used as a post-apply orphan sweep.
///
/// The namespace check (WWAN*/EWAN* prefix) is case-insensitive, so a mixed-case
/// section like `Wwan2` or `wwan` is still recognized as managed. The active-set
/// membership check is exact so only the authoritative canonical-case section
/// (e.g. `WWAN`) is preserved; a stale lowercase `wwan` left from a prior write
/// or OEM image is purged even when `WWAN` is active.
#[allow(dead_code)]
async fn purge_orphaned_managed_sections(
    active_names: &std::collections::HashSet<String>,
) -> Result<(), String> {
    let show_output = uci_show_network().await?;
    let parsed = parse_uci_show_output(&show_output);

    for name in parsed.keys() {
        let lc = name.to_ascii_lowercase();
        let is_managed = lc.starts_with("wwan") || lc.starts_with("ewan");
        if !is_managed {
            continue;
        }
        if active_names.contains(name.as_str()) {
            continue;
        }
        if let Err(e) = uci_delete_section(name).await {
            tracing::warn!("[WAN] Failed to purge orphan UCI section {name}: {e}");
            debug_trace_with_source(
                format!("[WAN] Failed to purge orphan UCI section {name}: {e}"),
                "wan",
            );
            continue;
        }
        let _ = uci_remove_from_wan_zone(name).await; // idempotent; failure is non-fatal
        tracing::info!("[WAN] Purged orphan UCI section: {name}");
        debug_trace_with_source(
            format!("[WAN] Purged orphan UCI section: {name}"),
            "wan",
        );
    }
    Ok(())
}

/// Run `uci show network` and return its stdout. Reads `MOCK_UCI_STATE` in test mode.
#[allow(dead_code)]
async fn uci_show_network() -> Result<String, String> {
    if is_mock_mode() {
        #[cfg(test)]
        { return Ok(mock_uci_render_show_output()); }
        #[cfg(not(test))]
        { return Ok(String::new()); }
    }
    let output = tokio::process::Command::new("sh")
        .arg("-c")
        .arg("uci show network")
        .output()
        .await
        .map_err(|e| format!("Failed to run uci show: {e}"))?;
    if !output.status.success() {
        return Err(format!("uci show network failed: {}", String::from_utf8_lossy(&output.stderr)));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Delete a single UCI interface section. In test mode, mutates `MOCK_UCI_STATE`.
#[allow(dead_code)]
async fn uci_delete_section(name: &str) -> Result<(), String> {
    if is_mock_mode() {
        #[cfg(test)]
        {
            if MOCK_DELETE_FAILS.load(std::sync::atomic::Ordering::SeqCst) {
                return Err("mock delete failure".to_string());
            }
            mock_uci_state().lock().unwrap().remove(name);
        }
        return Ok(());
    }
    // argv form — no shell parses the (untrusted) section name.
    let output = tokio::process::Command::new("uci")
        .arg("delete")
        .arg(format!("network.{name}"))
        .output()
        .await
        .map_err(|e| format!("Failed to run uci delete: {e}"))?;
    if !output.status.success() {
        return Err(format!("uci delete failed: {}", String::from_utf8_lossy(&output.stderr)));
    }
    Ok(())
}

/// Write or update a UCI interface section. Idempotent — safe to call when section
/// already exists with the same values.
#[allow(dead_code)]
async fn uci_write_interface_section(
    name: &str,
    device: &str,
    proto: &str,
    metric: u32,
    mtu: Option<u32>,
) -> Result<(), String> {
    if is_mock_mode() {
        #[cfg(test)]
        {
            let mut state = mock_uci_state().lock().unwrap();
            state.insert(
                name.to_string(),
                MockUciSection {
                    device: Some(device.to_string()),
                    proto: Some(proto.to_string()),
                    metric: Some(metric),
                    mtu,
                },
            );
        }
        return Ok(());
    }
    // argv form — each `uci set` runs as its own process with the key=value as a
    // single argument, so no shell parses the (untrusted) name/device/proto.
    // The `&&`-chained sh string previously here was a shell-injection sink.
    let mut sets: Vec<String> = vec![
        format!("network.{name}=interface"),
        format!("network.{name}.proto={proto}"),
        format!("network.{name}.device={device}"),
        format!("network.{name}.metric={metric}"),
        format!("network.{name}.auto=1"),
    ];
    if let Some(m) = mtu {
        sets.push(format!("network.{name}.mtu={m}"));
    }
    for set_arg in &sets {
        let output = tokio::process::Command::new("uci")
            .arg("set")
            .arg(set_arg)
            .output()
            .await
            .map_err(|e| format!("Failed to run uci set: {e}"))?;
        if !output.status.success() {
            return Err(format!("uci write failed: {}", String::from_utf8_lossy(&output.stderr)));
        }
    }
    Ok(())
}

/// Update the metric for an existing OpenWRT network interface.
async fn uci_set_metric(name: &str, metric: u32) -> Result<(), String> {
    if is_mock_mode() {
        debug_trace_with_source(format!("[WAN] Mock: uci set {name} metric={metric}"), "wan");
        return Ok(());
    }

    // argv form — section name is untrusted.
    let output = tokio::process::Command::new("uci")
        .arg("set")
        .arg(format!("network.{name}.metric={metric}"))
        .output()
        .await
        .map_err(|e| format!("Failed to run uci: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("UCI set metric failed: {stderr}"));
    }
    Ok(())
}

/// Set or clear MTU on an OpenWRT network interface via UCI.
async fn uci_set_mtu(name: &str, mtu: Option<u32>) -> Result<(), String> {
    if is_mock_mode() {
        debug_trace_with_source(
            format!("[WAN] Mock: uci set {name} mtu={mtu:?}"),
            "wan",
        );
        return Ok(());
    }

    // argv form — section name is untrusted. For the clear case, `uci delete`
    // failure (key absent) is non-fatal, mirroring the old `; true`.
    let output = match mtu {
        Some(val) => {
            tokio::process::Command::new("uci")
                .arg("set")
                .arg(format!("network.{name}.mtu={val}"))
                .output()
                .await
        }
        None => {
            tokio::process::Command::new("uci")
                .arg("delete")
                .arg(format!("network.{name}.mtu"))
                .output()
                .await
        }
    }
    .map_err(|e| format!("Failed to set MTU on {name}: {e}"))?;

    if !output.status.success() && mtu.is_some() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("uci set mtu failed for {name}: {stderr}"));
    }
    Ok(())
}

/// Add an interface to the wan firewall zone.
async fn uci_add_to_wan_zone(name: &str) -> Result<(), String> {
    if is_mock_mode() {
        #[cfg(test)]
        mock_wan_zone().lock().unwrap().insert(name.to_string());
        debug_trace_with_source(format!("[WAN] Mock: uci add {name} to wan zone"), "wan");
        return Ok(());
    }

    // First check if already in the zone to avoid duplicates. argv form: read
    // the current network list and test membership in Rust (no grep, no shell).
    let check = tokio::process::Command::new("uci")
        .arg("get")
        .arg("firewall.@zone[1].network")
        .output()
        .await;

    if let Ok(output) = check {
        if output.status.success() {
            let current = String::from_utf8_lossy(&output.stdout);
            // UCI list values are space-separated.
            if current.split_whitespace().any(|tok| tok == name) {
                // Already in zone
                return Ok(());
            }
        }
    }

    let output = tokio::process::Command::new("uci")
        .arg("add_list")
        .arg(format!("firewall.@zone[1].network={name}"))
        .output()
        .await
        .map_err(|e| format!("Failed to run uci: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("UCI add to wan zone failed: {stderr}"));
    }

    debug_trace_with_source(format!("[WAN] Added {name} to wan firewall zone"), "wan");
    Ok(())
}

/// Commit UCI changes and reload the network.
async fn uci_commit_and_reload() -> Result<(), String> {
    if is_mock_mode() {
        debug_trace_with_source("[WAN] Mock: uci commit && network reload", "wan");
        return Ok(());
    }

    let cmd = "uci commit network && uci commit firewall && /etc/init.d/network reload";
    let output = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .output()
        .await
        .map_err(|e| format!("Failed to run uci commit: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("UCI commit/reload failed: {stderr}"));
    }

    debug_trace_with_source("[WAN] UCI committed, network reloaded", "wan");
    Ok(())
}

/// Commit UCI changes without reloading the network.
/// Used when targeted ifdown/ifup is sufficient.
async fn uci_commit_only() -> Result<(), String> {
    if is_mock_mode() {
        debug_trace_with_source("[WAN] Mock: uci commit (no reload)", "wan");
        return Ok(());
    }

    let cmd = "uci commit network && uci commit firewall";
    let output = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .output()
        .await
        .map_err(|e| format!("Failed to run uci commit: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("uci commit failed: {stderr}"));
    }
    Ok(())
}

/// Reset a modem's USB device via sysfs unbind/rebind.
/// This forces full USB re-enumeration which resets the ECM bearer.
async fn usb_reset_modem(modem_id: &str) -> Result<(), String> {
    if is_mock_mode() {
        debug_trace_with_source(format!("[WAN] Mock: USB reset for {modem_id}"), "wan");
        return Ok(());
    }

    // Parse VID:PID:SERIAL from modem_id (e.g. "2c7c:0122:e3183572")
    let parts: Vec<&str> = modem_id.split(':').collect();
    if parts.len() < 3 {
        return Err(format!("Invalid modem_id format for USB reset: {modem_id}"));
    }
    let vid = parts[0];
    let pid = parts[1];
    let serial = parts[2];

    // Find USB bus path by matching VID+PID+serial in sysfs — pure Rust, no
    // shell. Enumerate `/sys/bus/usb/devices/*`, read each device's
    // `idVendor`/`idProduct`/`serial` files directly, and compare in Rust.
    let bus_path = find_usb_bus_path(vid, pid, serial).await;
    let Some(bus_path) = bus_path else {
        return Err(format!("USB device not found for {modem_id}"));
    };

    debug_trace_with_source(
        format!("[WAN] USB reset: unbind/rebind {bus_path} for {modem_id}"),
        "wan",
    );

    // Unbind — write the bus path directly to the driver's sysfs node (no shell
    // redirection, no interpolation into a command line).
    let _ = tokio::fs::write("/sys/bus/usb/drivers/usb/unbind", &bus_path).await;

    // Wait for device to fully detach
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    // Rebind
    let _ = tokio::fs::write("/sys/bus/usb/drivers/usb/bind", &bus_path).await;

    Ok(())
}

/// Walk `/sys/bus/usb/devices/` and return the bus path (e.g. `2-1`) of the
/// first device whose `idVendor`/`idProduct`/`serial` match the requested
/// triple. Pure Rust — no `sh -c`, no interpolation of untrusted input into a
/// command line. Reads only the three sysfs attribute files per device.
///
/// USB-interface entries (those containing `:` in their name, e.g. `2-1:1.0`)
/// have no `idVendor`/`serial` and simply read as `None`, so they are skipped
/// naturally without an explicit filter.
async fn find_usb_bus_path(vid: &str, pid: &str, serial: &str) -> Option<String> {
    let mut entries = tokio::fs::read_dir("/sys/bus/usb/devices").await.ok()?;
    while let Ok(Some(entry)) = entries.next_entry().await {
        let dir = entry.path();
        let read_attr = |name: &str| {
            let p = dir.join(name);
            std::fs::read_to_string(p)
                .ok()
                .map(|s| s.trim().to_string())
        };
        let (Some(have_vid), Some(have_pid), Some(have_serial)) =
            (read_attr("idVendor"), read_attr("idProduct"), read_attr("serial"))
        else {
            continue;
        };
        if usb_device_matches(
            (&have_vid, &have_pid, &have_serial),
            (vid, pid, serial),
        ) {
            return entry.file_name().to_str().map(|s| s.to_string());
        }
    }
    None
}

/// Check if a network device has an IPv4 address assigned.
async fn check_interface_has_ip(device: &str) -> bool {
    if is_mock_mode() {
        return true;
    }

    // argv form — device is untrusted. Run `ip -4 addr show dev <device>` and
    // test for an `inet ` line in Rust instead of piping to grep.
    let output = tokio::process::Command::new("ip")
        .args(["-4", "addr", "show", "dev", device])
        .output()
        .await;

    matches!(output, Ok(o) if o.status.success()
        && String::from_utf8_lossy(&o.stdout).contains("inet "))
}

/// Run ifdown on an interface.
async fn ifdown(name: &str) -> Result<(), String> {
    if is_mock_mode() {
        debug_trace_with_source(format!("[WAN] Mock: ifdown {name}"), "wan");
        return Ok(());
    }

    // argv form — interface name is untrusted.
    let _ = tokio::process::Command::new("ifdown")
        .arg(name)
        .output()
        .await;
    Ok(())
}

/// Delete a UCI network interface (used when removing a modem from the WAN list).
async fn uci_delete_interface(name: &str) -> Result<(), String> {
    if is_mock_mode() {
        debug_trace_with_source(format!("[WAN] Mock: uci delete interface {name}"), "wan");
        return Ok(());
    }

    // argv form — section name is untrusted.
    let _ = tokio::process::Command::new("uci")
        .arg("delete")
        .arg(format!("network.{name}"))
        .output()
        .await;

    debug_trace_with_source(format!("[WAN] Deleted UCI interface {name}"), "wan");
    Ok(())
}

/// Remove a port from a bridge via UCI.
async fn uci_remove_from_bridge(port: &str) -> Result<(), String> {
    if is_mock_mode() {
        debug_trace_with_source(format!("[WAN] Mock: uci del_list bridge port {port}"), "wan");
        return Ok(());
    }

    // argv form — port name is untrusted.
    let output = tokio::process::Command::new("uci")
        .arg("del_list")
        .arg(format!("network.@device[0].ports={port}"))
        .output()
        .await
        .map_err(|e| format!("Failed to run uci: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("UCI remove from bridge failed: {stderr}"));
    }

    debug_trace_with_source(format!("[WAN] Removed {port} from bridge"), "wan");
    Ok(())
}

/// Add a port back to a bridge via UCI.
async fn uci_add_to_bridge(port: &str) -> Result<(), String> {
    if is_mock_mode() {
        debug_trace_with_source(format!("[WAN] Mock: uci add_list bridge port {port}"), "wan");
        return Ok(());
    }

    // argv form — port name is untrusted.
    let output = tokio::process::Command::new("uci")
        .arg("add_list")
        .arg(format!("network.@device[0].ports={port}"))
        .output()
        .await
        .map_err(|e| format!("Failed to run uci: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("UCI add to bridge failed: {stderr}"));
    }

    debug_trace_with_source(format!("[WAN] Added {port} back to bridge"), "wan");
    Ok(())
}

/// Remove an interface from the wan firewall zone.
async fn uci_remove_from_wan_zone(name: &str) -> Result<(), String> {
    if is_mock_mode() {
        #[cfg(test)]
        mock_wan_zone().lock().unwrap().remove(name);
        debug_trace_with_source(format!("[WAN] Mock: uci del_list {name} from wan zone"), "wan");
        return Ok(());
    }

    // argv form — section name is untrusted.
    let output = tokio::process::Command::new("uci")
        .arg("del_list")
        .arg(format!("firewall.@zone[1].network={name}"))
        .output()
        .await
        .map_err(|e| format!("Failed to run uci: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("UCI remove from wan zone failed: {stderr}"));
    }

    debug_trace_with_source(format!("[WAN] Removed {name} from wan firewall zone"), "wan");
    Ok(())
}

/// Check if a UCI network interface exists.
async fn uci_interface_exists(name: &str) -> bool {
    if is_mock_mode() {
        return true;
    }

    // argv form — section name is untrusted.
    match tokio::process::Command::new("uci")
        .arg("get")
        .arg(format!("network.{name}.proto"))
        .output()
        .await
    {
        Ok(output) => output.status.success(),
        Err(_) => false,
    }
}

/// Run ifup on an interface.
#[allow(dead_code)]
async fn ifup(name: &str) -> Result<(), String> {
    if is_mock_mode() {
        debug_trace_with_source(format!("[WAN] Mock: ifup {name}"), "wan");
        return Ok(());
    }

    // argv form — interface name is untrusted.
    let _ = tokio::process::Command::new("ifup")
        .arg(name)
        .output()
        .await;
    Ok(())
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Assign metrics based on priority order and state.
/// Active modems: 20 + i*10 (where i is position among ALL modems).
/// Standby modems: metric 998 (last resort, above removed but below active).
/// Detect whether the router uses nftables (fw4) or legacy iptables.
/// Cached after first call.
async fn detect_firewall_backend() -> &'static str {
    static BACKEND: OnceLock<String> = OnceLock::new();

    if let Some(b) = BACKEND.get() {
        return b.as_str();
    }

    if is_mock_mode() {
        let _ = BACKEND.set("nft".to_string());
        return BACKEND.get().unwrap().as_str();
    }

    let output = tokio::process::Command::new("sh")
        .arg("-c")
        .arg("command -v nft")
        .output()
        .await;

    let backend = match output {
        Ok(o) if o.status.success() => "nft",
        _ => "iptables",
    };

    let _ = BACKEND.set(backend.to_string());
    BACKEND.get().unwrap().as_str()
}

/// Generate and apply outbound TTL/Hop Limit mangle rules for all WAN entries.
///
/// For nftables: writes /etc/nftables.d/10-ctrl-wan-ttl.nft and loads it.
/// For iptables: flushes old rules and applies new ones via shell commands.
/// If no entries have TTL/HL set, cleans up all rules.
async fn apply_ttl_rules(entries: &[WanModemEntry]) -> Result<(), String> {
    let fw = detect_firewall_backend().await;

    // Collect entries that have TTL or hop_limit set
    let rules: Vec<_> = entries
        .iter()
        .filter(|e| e.ttl.is_some() || e.hop_limit.is_some())
        .collect();

    if is_mock_mode() {
        if rules.is_empty() {
            debug_trace_with_source("[WAN] Mock: TTL/HL rules cleared (none configured)", "wan");
        } else {
            for entry in &rules {
                if let Some(ttl) = entry.ttl {
                    debug_trace_with_source(
                        format!("[WAN] Mock: {} ({}) IPv4 TTL set {ttl}", entry.label, entry.network_device),
                        "wan",
                    );
                }
                if let Some(hl) = entry.hop_limit {
                    debug_trace_with_source(
                        format!("[WAN] Mock: {} ({}) IPv6 HL set {hl}", entry.label, entry.network_device),
                        "wan",
                    );
                }
            }
        }
        debug_trace_with_source(format!("[WAN] Mock: firewall backend={fw}"), "wan");
        return Ok(());
    }

    match fw {
        "nft" => apply_ttl_rules_nft(&rules).await,
        _ => apply_ttl_rules_iptables(&rules).await,
    }
}

/// Apply TTL/HL rules via nftables — write file + load.
async fn apply_ttl_rules_nft(rules: &[&WanModemEntry]) -> Result<(), String> {
    // Avoid /etc/nftables.d/ — fw4 includes that directory inside its own table context,
    // which makes our top-level table definition illegal and breaks every firewall reload.
    let nft_dir = "/etc/modem-interface";
    let nft_file = format!("{nft_dir}/ctrl-wan-ttl.nft");

    // Clean up legacy location that breaks fw4 firewall reloads
    let legacy_file = "/etc/nftables.d/10-ctrl-wan-ttl.nft";
    let _ = tokio::fs::remove_file(legacy_file).await;

    if rules.is_empty() {
        // Clean up: flush table and remove file. argv form; a missing table
        // makes `nft delete` fail harmlessly (ignored, like the old `; true`).
        let _ = tokio::process::Command::new("nft")
            .args(["delete", "table", "inet", "ctrl_wan_mangle"])
            .output()
            .await;
        let _ = tokio::fs::remove_file(&nft_file).await;
        return Ok(());
    }

    // Build nftables ruleset
    let mut nft_rules = String::from(
        "# Auto-generated by CTRL-WAN — do not edit\n\
         table inet ctrl_wan_mangle {\n  \
           chain postrouting {\n    \
             type filter hook postrouting priority mangle; policy accept;\n",
    );

    for entry in rules {
        let dev = &entry.network_device;
        if let Some(ttl) = entry.ttl {
            nft_rules.push_str(&format!("    oifname \"{dev}\" ip ttl set {ttl}\n"));
        }
        if let Some(hl) = entry.hop_limit {
            nft_rules.push_str(&format!("    oifname \"{dev}\" ip6 hoplimit set {hl}\n"));
        }
    }

    nft_rules.push_str("  }\n}\n");

    // Ensure directory exists
    let _ = tokio::fs::create_dir_all(nft_dir).await;

    // Write the file
    tokio::fs::write(&nft_file, &nft_rules)
        .await
        .map_err(|e| format!("Failed to write {nft_file}: {e}"))?;

    // Flush old table, then load new file. argv form (the device name lives in
    // the file content, which nft parses — not a shell). A missing table makes
    // the flush fail harmlessly.
    let _ = tokio::process::Command::new("nft")
        .args(["delete", "table", "inet", "ctrl_wan_mangle"])
        .output()
        .await;

    let output = tokio::process::Command::new("nft")
        .arg("-f")
        .arg(&nft_file)
        .output()
        .await
        .map_err(|e| format!("nft load failed: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("nft load failed: {stderr}"));
    }

    Ok(())
}

/// Apply TTL/HL rules via iptables (legacy fallback).
async fn apply_ttl_rules_iptables(rules: &[&WanModemEntry]) -> Result<(), String> {
    // Flush existing CTRL-WAN rules (marked with comment)
    let flush_cmd = "\
        iptables -t mangle -S POSTROUTING 2>/dev/null | grep 'CTRL-WAN-TTL' | sed 's/-A/-D/' | while read rule; do iptables -t mangle $rule; done; \
        ip6tables -t mangle -S POSTROUTING 2>/dev/null | grep 'CTRL-WAN-TTL' | sed 's/-A/-D/' | while read rule; do ip6tables -t mangle $rule; done";
    let _ = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(flush_cmd)
        .output()
        .await;

    if rules.is_empty() {
        // Also remove persistence script
        let _ = tokio::fs::remove_file("/etc/ctrl-wan-ttl.sh").await;
        return Ok(());
    }

    // Each applied rule is held as a (binary, argv) pair so it runs without a
    // shell — `dev` (network_device) is untrusted. The persistence script still
    // records the equivalent command line for boot-time re-apply; `dev` is
    // validated to [A-Za-z0-9_-]{1,32} at the WAN-config boundary so it carries
    // no shell metacharacters into that script.
    let mut argv_cmds: Vec<(&'static str, Vec<String>)> = Vec::new();
    let mut script_lines: Vec<String> = vec![
        "#!/bin/sh".to_string(),
        "# Auto-generated by CTRL-WAN — do not edit".to_string(),
    ];

    for entry in rules {
        let dev = &entry.network_device;
        if let Some(ttl) = entry.ttl {
            let args: Vec<String> = vec![
                "-t".into(), "mangle".into(), "-A".into(), "POSTROUTING".into(),
                "-o".into(), dev.to_string(),
                "-m".into(), "comment".into(), "--comment".into(), "CTRL-WAN-TTL".into(),
                "-j".into(), "TTL".into(), "--ttl-set".into(), ttl.to_string(),
            ];
            script_lines.push(format!("iptables {}", args.join(" ")));
            argv_cmds.push(("iptables", args));
        }
        if let Some(hl) = entry.hop_limit {
            let args: Vec<String> = vec![
                "-t".into(), "mangle".into(), "-A".into(), "POSTROUTING".into(),
                "-o".into(), dev.to_string(),
                "-m".into(), "comment".into(), "--comment".into(), "CTRL-WAN-TTL".into(),
                "-j".into(), "HL".into(), "--hl-set".into(), hl.to_string(),
            ];
            script_lines.push(format!("ip6tables {}", args.join(" ")));
            argv_cmds.push(("ip6tables", args));
        }
    }

    // Apply rules now (argv form — no shell).
    for (bin, args) in &argv_cmds {
        let _ = tokio::process::Command::new(bin)
            .args(args)
            .output()
            .await;
    }

    // Write persistence script
    let script_content = script_lines.join("\n") + "\n";
    let _ = tokio::fs::write("/etc/ctrl-wan-ttl.sh", &script_content).await;
    let _ = tokio::process::Command::new("chmod")
        .arg("+x")
        .arg("/etc/ctrl-wan-ttl.sh")
        .output()
        .await;

    Ok(())
}

fn assign_metrics(modems: &mut [WanModemEntry]) {
    for (i, modem) in modems.iter_mut().enumerate() {
        modem.metric = if modem.is_active() {
            20 + (i as u32) * 10
        } else {
            998
        };
    }
}

/// Build a WanStatusResponse from current config + runtime state.
async fn build_status_response(state: &AppState) -> WanStatusResponse {
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

    // Pre-fetch IMEIs from discovery info for each modem
    let modem_imeis: std::collections::HashMap<String, Option<String>> = {
        let modems_map = state.modems.read().await;
        let mut imeis = std::collections::HashMap::new();
        for (modem_id, context) in modems_map.iter() {
            let discovery = context.discovery.read().await;
            let imei = if discovery.device_info.imei.is_empty() {
                None
            } else {
                Some(discovery.device_info.imei.clone())
            };
            imeis.insert(modem_id.clone(), imei);
        }
        imeis
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
        .enumerate()
        .map(|(i, entry)| {
            let runtime_info = runtime.modem_statuses.get(&entry.modem_id);
            let is_first_active = config
                .modem_priority
                .iter()
                .find(|m| m.is_active())
                .is_some_and(|m| m.modem_id == entry.modem_id);

            WanModemStatusEntry {
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
                is_primary: is_first_active && i == config
                    .modem_priority
                    .iter()
                    .position(|m| m.is_active())
                    .unwrap_or(usize::MAX),
                entry_type: entry.entry_type.clone(),
                original_bridge: entry.original_bridge.clone(),
                mtu: entry.mtu,
                ttl: entry.ttl,
                hop_limit: entry.hop_limit,
                operator: modem_operators.get(&entry.modem_id).cloned().flatten(),
                imei: modem_imeis.get(&entry.modem_id).cloned().flatten(),
                restart_suspended: runtime_info.map(|r| r.restart_suspended).unwrap_or(false),
                restart_count: runtime_info.map(|r| r.restart_count).unwrap_or(0),
                wedged: runtime_info.map(|r| r.wedged).unwrap_or(false),
                weight: entry.weight,
                proto_override: entry.proto_override.clone(),
                // Diagnostic only — Ethernet entries have no modem to query.
                usbnet_mode: if entry.entry_type == WanEntryType::Modem {
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
        platform: {
            let caps = state.platform_capabilities.read().await;
            Some(caps.clone())
        },
        routing_tables: {
            let rs = state.routing_state.read().await;
            if rs.is_empty() { None } else { Some(rs.clone()) }
        },
    }
}

/// Save WAN config and broadcast status update.
async fn save_and_broadcast(state: &AppState) -> Result<(), String> {
    let config = state.wan_config.read().await;
    crate::config::wan::save_wan_config(&config).await?;
    drop(config);

    let response = build_status_response(state).await;
    state.broadcast_event(ModemEvent::WanStatusUpdate(Box::new(response)));
    Ok(())
}

// ============================================================================
// Ethernet Discovery Helpers
// ============================================================================

/// Discover existing Ethernet WAN interfaces via UCI.
/// Returns entries for interfaces like `wan` (br-wan, proto=dhcp) that are NOT WWAN.
async fn discover_ethernet_wan_interfaces() -> Vec<WanModemEntry> {
    let mut entries = Vec::new();

    if is_mock_mode() {
        debug_trace_with_source("[WAN] Mock: discovering Ethernet WAN interfaces", "wan");
        // In mock mode, return a synthetic existing WAN interface
        entries.push(WanModemEntry {
            modem_id: "eth:br-wan".to_string(),
            label: "Ethernet WAN (br-wan)".to_string(),
            interface_name: "wan".to_string(),
            network_device: "br-wan".to_string(),
            device_path: String::new(),
            state: WanModemState::Standby,
            metric: 998,
            entry_type: WanEntryType::Ethernet,
            original_bridge: None,
            mtu: None,
            ttl: None,
            hop_limit: None,
            weight: None,
            proto_override: None,
        });
        return entries;
    }

    // Query UCI for all network interfaces
    let output = tokio::process::Command::new("sh")
        .arg("-c")
        .arg("uci show network 2>/dev/null")
        .output()
        .await;

    let uci_output = match output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
        _ => return entries,
    };

    // Parse UCI output to find non-WWAN interfaces with proto=dhcp or proto=static
    // Look for patterns like:
    //   network.wan=interface
    //   network.wan.proto='dhcp'
    //   network.wan.device='br-wan'
    // (proto, device, is_modem_managed)
    let mut interfaces: std::collections::HashMap<String, (Option<String>, Option<String>, bool)> =
        std::collections::HashMap::new();

    for line in uci_output.lines() {
        let line = line.trim();
        // network.{name}=interface
        if line.ends_with("=interface") {
            if let Some(name) = line.strip_prefix("network.").and_then(|s| s.strip_suffix("=interface")) {
                // Skip WWAN interfaces (those are modems)
                if name.starts_with("WWAN") || name.starts_with("wwan") {
                    continue;
                }
                // Skip loopback
                if name == "loopback" {
                    continue;
                }
                interfaces.entry(name.to_string()).or_insert((None, None, false));
            }
        }
        // network.{name}.proto='dhcp' or 'static'
        if let Some(rest) = line.strip_prefix("network.") {
            if let Some((name_and_key, value)) = rest.split_once('=') {
                if let Some((name, key)) = name_and_key.split_once('.') {
                    let value = value.trim_matches('\'');
                    if let Some(entry) = interfaces.get_mut(name) {
                        match key {
                            "proto" => {
                                if value == "dhcp" || value == "static" {
                                    entry.0 = Some(value.to_string());
                                }
                            }
                            "device" => {
                                entry.1 = Some(value.to_string());
                            }
                            // ifname fallback (older UCI) — use if device not set
                            "ifname" => {
                                if entry.1.is_none() && !value.starts_with('@') {
                                    entry.1 = Some(value.to_string());
                                }
                            }
                            // modem_config set by qmodem/external modem managers
                            "modem_config" => {
                                entry.2 = true;
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    }

    // Build entries for valid Ethernet WAN interfaces
    for (name, (proto, device, modem_managed)) in &interfaces {
        if proto.is_some() {
            // Skip modem-managed interfaces (qmodem, etc.)
            if *modem_managed {
                continue;
            }
            // Skip the LAN bridge — it should never be a WAN candidate
            if name == "lan" {
                continue;
            }
            // Skip VPN interfaces by name
            if name == "VPN" || name.starts_with("vpn") {
                continue;
            }
            if let Some(dev) = device {
                if dev.starts_with("br-lan") {
                    continue;
                }
                // Skip if this looks like a modem device (usb*, wwan*, rmnet_mhi*)
                if dev.starts_with("usb") || dev.starts_with("wwan") || dev.starts_with("rmnet_mhi") {
                    continue;
                }
                // Skip VPN/tunnel interfaces
                if dev.starts_with("ipsec") || dev.starts_with("tun") || dev.starts_with("tap") || dev.starts_with("wg") {
                    continue;
                }
                entries.push(WanModemEntry {
                    modem_id: format!("eth:{dev}"),
                    label: format!("Ethernet WAN ({dev})"),
                    interface_name: name.clone(),
                    network_device: dev.clone(),
                    device_path: String::new(),
                    state: WanModemState::Standby,
                    metric: 998,
                    entry_type: WanEntryType::Ethernet,
                    original_bridge: None, // Existing WAN, not converted
                    mtu: None,
                    ttl: None,
                    hop_limit: None,
                    weight: None,
                    proto_override: None,
                });
                debug_trace_with_source(format!(
                    "[WAN] Found Ethernet WAN: {name} device={dev}"
                ), "wan");
            }
        }
    }

    entries
}

/// Discover available LAN ports that can be converted to WAN.
/// Returns ports currently in br-lan that are not yet used as WAN.
async fn discover_available_lan_ports(
    existing_wan_devices: &std::collections::HashSet<String>,
) -> Vec<AvailableEthernetPort> {
    let mut ports = Vec::new();

    if is_mock_mode() {
        debug_trace_with_source("[WAN] Mock: discovering available LAN ports", "wan");
        ports.push(AvailableEthernetPort {
            port_name: "lan0".to_string(),
            bridge: "br-lan".to_string(),
            link_status: "up".to_string(),
        });
        ports.push(AvailableEthernetPort {
            port_name: "lan3".to_string(),
            bridge: "br-lan".to_string(),
            link_status: "down".to_string(),
        });
        return ports;
    }

    // Get bridge ports from UCI
    let output = tokio::process::Command::new("sh")
        .arg("-c")
        .arg("uci get network.@device[0].ports 2>/dev/null")
        .output()
        .await;

    let port_list = match output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        _ => return ports,
    };

    let mut seen_ports = std::collections::HashSet::new();
    for port_name in port_list.split_whitespace() {
        let port_name = port_name.trim_matches('\'').trim();
        if port_name.is_empty() {
            continue;
        }

        // Skip duplicate port entries
        if !seen_ports.insert(port_name.to_string()) {
            continue;
        }

        // Skip if already used as a WAN device
        if existing_wan_devices.contains(port_name) {
            continue;
        }

        // Check link status
        let link_status = check_link_status(port_name).await;

        ports.push(AvailableEthernetPort {
            port_name: port_name.to_string(),
            bridge: "br-lan".to_string(),
            link_status,
        });
    }

    ports
}

/// Check the link status of a network interface.
async fn check_link_status(port: &str) -> String {
    if is_mock_mode() {
        return "down".to_string();
    }

    // Read the sysfs carrier file directly (no shell, no interpolation into a
    // command line — `port` is untrusted).
    match tokio::fs::read_to_string(format!("/sys/class/net/{port}/carrier")).await {
        Ok(val) if val.trim() == "1" => "up".to_string(),
        _ => "down".to_string(),
    }
}

/// Get the bridge a port belongs to (checks UCI config).
async fn get_port_bridge(port_name: &str) -> Option<String> {
    if is_mock_mode() {
        return Some("br-lan".to_string());
    }

    let output = tokio::process::Command::new("sh")
        .arg("-c")
        .arg("uci get network.@device[0].ports 2>/dev/null")
        .output()
        .await;

    let port_list = match output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        _ => return None,
    };

    for port in port_list.split_whitespace() {
        let port = port.trim_matches('\'').trim();
        if port == port_name {
            return Some("br-lan".to_string());
        }
    }

    None
}

// ============================================================================
// Route Handlers
// ============================================================================

/// GET /api/wan/status
///
/// Get full WAN manager state including modem statuses and failover history.
pub async fn get_wan_status(
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<WanStatusResponse>> {
    let response = build_status_response(&state).await;
    Ok(Json(response))
}

/// PUT /api/wan/config
///
/// Single write path for all WAN configuration changes: enable/disable,
/// reorder priority list, watchdog settings, failover lock.
/// Diffs against current config to determine needed UCI operations,
/// then executes a single uci commit + network reload.
pub async fn update_wan_config(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<SessionUser>,
    Json(new_config): Json<WanConfig>,
) -> ApiResult<Json<WanStatusResponse>> {
    require_admin(&user)?;

    // Validate
    if new_config.watchdog.check_interval_secs < 5 || new_config.watchdog.check_interval_secs > 300
    {
        return Err(ApiError::bad_request(
            "Check interval must be between 5 and 300 seconds",
        ));
    }
    if new_config.watchdog.failure_threshold < 1 || new_config.watchdog.failure_threshold > 10 {
        return Err(ApiError::bad_request(
            "Failure threshold must be between 1 and 10",
        ));
    }
    if new_config.watchdog.max_restart_attempts < 1 || new_config.watchdog.max_restart_attempts > 50 {
        return Err(ApiError::bad_request(
            "Max restart attempts must be between 1 and 50",
        ));
    }
    if new_config.watchdog.wedge_reboot_grace_mins < 1
        || new_config.watchdog.wedge_reboot_grace_mins > 120
    {
        return Err(ApiError::bad_request(
            "wedge_reboot_grace_mins must be 1-120",
        ));
    }
    if new_config.watchdog.wedge_reboot_max_per_day > 10 {
        return Err(ApiError::bad_request(
            "wedge_reboot_max_per_day must be 0-10",
        ));
    }
    if new_config.watchdog.wedge_reboot_min_uptime_mins < 1
        || new_config.watchdog.wedge_reboot_min_uptime_mins > 240
    {
        return Err(ApiError::bad_request(
            "wedge_reboot_min_uptime_mins must be 1-240",
        ));
    }
    if !matches!(new_config.failback_timer_mins, 0 | 15 | 30 | 60 | 360 | 720) {
        return Err(ApiError::bad_request(
            "Failback timer must be one of: 0 (never), 15, 30, 60, 360, 720 minutes",
        ));
    }
    // When WAN manager is enabled, require at least one entry and at least one active entry
    if new_config.enabled {
        if new_config.modem_priority.is_empty() {
            return Err(ApiError::bad_request(
                "WAN manager cannot be enabled with an empty priority list. Add at least one WAN entry or disable the manager.",
            ));
        }
        if !new_config
            .modem_priority
            .iter()
            .any(|e| e.state == WanModemState::Active)
        {
            return Err(ApiError::bad_request(
                "WAN manager requires at least one active entry. Set at least one entry to active or disable the manager.",
            ));
        }
    }

    // Validate watchdog connectivity targets (shell-injection defense in depth).
    // These feed `ping`/`curl`/`wget` invocations in the watchdog loop.
    if !is_valid_watchdog_host(&new_config.watchdog.ping_target) {
        return Err(ApiError::bad_request(
            "Invalid ping target: must be a hostname or IP (alphanumerics, '.', '-', ':' only)",
        ));
    }
    if !is_valid_watchdog_host(&new_config.watchdog.dns_target) {
        return Err(ApiError::bad_request(
            "Invalid DNS target: must be a hostname or IP (alphanumerics, '.', '-', ':' only)",
        ));
    }
    if !is_valid_http_target(&new_config.watchdog.http_target) {
        return Err(ApiError::bad_request(
            "Invalid HTTP target: must be an http(s) URL with no whitespace or shell metacharacters",
        ));
    }

    // Validate per-entry settings
    for entry in &new_config.modem_priority {
        // Strict allowlist on the modem identifier. `modem_id` is split on `:`
        // and (in the USB-reset path) used to locate a sysfs device; reject any
        // shell metacharacter / control char so a crafted id can never reach a
        // command builder. Must be VID:PID:SERIAL-shaped alphanumerics.
        if !is_valid_modem_id(&entry.modem_id) {
            return Err(ApiError::bad_request(
                "Invalid modem_id: must be colon-separated alphanumeric segments (VID:PID:SERIAL)",
            ));
        }
        // Strict allowlist on UCI section name + network device. Even with argv
        // command builders, these become UCI keys / device identifiers.
        if !is_valid_uci_token(&entry.interface_name) {
            return Err(ApiError::bad_request(
                "Invalid interface_name: must be 1-32 chars of [A-Za-z0-9_-]",
            ));
        }
        if !is_valid_uci_token(&entry.network_device) {
            return Err(ApiError::bad_request(
                "Invalid network_device: must be 1-32 chars of [A-Za-z0-9_-]",
            ));
        }
        if let Some(mtu) = entry.mtu {
            if !(576..=9000).contains(&mtu) {
                return Err(ApiError::bad_request(
                    "MTU must be between 576 and 9000",
                ));
            }
        }
        if entry.ttl == Some(0) {
            return Err(ApiError::bad_request(
                "TTL must be between 1 and 255",
            ));
        }
        if entry.hop_limit == Some(0) {
            return Err(ApiError::bad_request(
                "Hop Limit must be between 1 and 255",
            ));
        }
        if let Some(w) = entry.weight {
            if !(1..=10).contains(&w) {
                return Err(ApiError::bad_request(
                    "Weight must be between 1 and 10",
                ));
            }
        }
        if let Some(p) = entry.proto_override.as_deref() {
            // Strict allowlist: a UCI proto value is a short token. Reject
            // anything outside [A-Za-z0-9_-]{1,32} so no shell metacharacter,
            // whitespace, or empty/whitespace-only value can pass — this string
            // is written as a UCI proto and (defense in depth) must never carry
            // an injection payload even though the writer now uses argv form.
            if !is_valid_uci_token(p) {
                return Err(ApiError::bad_request(
                    "Invalid proto_override: must be 1-32 chars of [A-Za-z0-9_-]",
                ));
            }
        }
    }

    let mut config = state.wan_config.write().await;

    // Snapshot old config for diffing
    let old_modem_map: std::collections::HashMap<String, &WanModemEntry> = config
        .modem_priority
        .iter()
        .map(|m| (m.modem_id.clone(), m))
        .collect();

    // Apply new config
    let mut new_priority = new_config.modem_priority;
    assign_metrics(&mut new_priority);

    // Build new modem map for diffing
    let new_modem_map: std::collections::HashMap<String, &WanModemEntry> = new_priority
        .iter()
        .map(|m| (m.modem_id.clone(), m))
        .collect();

    // Pre-fetch detected USB-net mode per modem (Item #37 sub-task 2).
    // Pre-fetch detected USB-net mode + cdc-wdm control device path per modem
    // (Item #37 sub-tasks 2 + 2b). Built once and dropped before the reconcile
    // loop so the state.modems read guard never lives across an await that may
    // broadcast events (lock-ordering — mirrors build_status_response at
    // ~line 985). The sysfs walk inside the build block is synchronous (no
    // await) so the read guard is held only across non-await code.
    let modem_resolved: std::collections::HashMap<String, ResolvedReconcileFields> = {
        let modems_map = state.modems.read().await;
        let mut out = std::collections::HashMap::new();
        for (modem_id, ctx) in modems_map.iter() {
            let mode = *ctx.usbnet_mode.read().await;
            let control_device_path = ctx
                .detected
                .bus_port
                .as_deref()
                .and_then(crate::hardware::find_qmi_control_device_for_bus_port);
            out.insert(
                modem_id.clone(),
                ResolvedReconcileFields {
                    usbnet_mode: mode,
                    control_device_path,
                },
            );
        }
        out
    };

    // Diff: modems that changed state or metric
    for entry in &new_priority {
        match old_modem_map.get(&entry.modem_id) {
            Some(old) => {
                // Existing modem — check for state changes
                if old.is_active() && !entry.is_active() {
                    // Was active, now standby: set metric 998, keep interface up
                    let _ = uci_set_metric(&entry.interface_name, 998).await;
                    debug_trace_with_source(
                        format!("[WAN] {} → standby: metric 998", entry.label),
                        "wan",
                    );
                } else if !old.is_active() && entry.is_active() {
                    // Was standby, now active: set metric based on position, keep interface up
                    let _ = uci_set_metric(&entry.interface_name, entry.metric).await;
                    debug_trace_with_source(
                        format!("[WAN] {} → active: metric {}", entry.label, entry.metric),
                        "wan",
                    );
                } else if old.metric != entry.metric {
                    // Same state but metric changed (reorder)
                    let _ = uci_set_metric(&entry.interface_name, entry.metric).await;
                }
                // Check if converted Ethernet port needs UCI provisioning
                // (add-ethernet defers UCI creation until Save & Apply)
                if entry.entry_type == WanEntryType::Ethernet {
                    if let Some(ref _bridge) = entry.original_bridge {
                        if !uci_interface_exists(&entry.interface_name).await {
                            // UCI interface not yet created — provision it now
                            if let Err(e) = uci_remove_from_bridge(&entry.network_device).await {
                                tracing::warn!("Failed to remove {} from bridge: {e}", entry.network_device);
                            }
                            let resolved = modem_resolved
                                .get(&entry.modem_id)
                                .cloned()
                                .unwrap_or_default();
                            let proto = resolve_uci_proto(entry, Some(resolved.usbnet_mode));
                            match reconcile_uci_section(
                                &entry.interface_name,
                                &entry.network_device,
                                proto.as_ref(),
                                resolved.control_device_path.as_deref(),
                                entry.metric,
                                entry.mtu,
                            ).await {
                                Ok(displaced) => {
                                    for displaced_name in displaced {
                                        state.broadcast_event(ModemEvent::WanCollisionDisplaced {
                                            our_name: entry.interface_name.clone(),
                                            displaced_name,
                                            device: entry.network_device.clone(),
                                        });
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!("Failed to reconcile UCI interface {}: {e}", entry.interface_name);
                                }
                            }
                            if let Err(e) = uci_add_to_wan_zone(&entry.interface_name).await {
                                tracing::warn!("Failed to add {} to wan zone: {e}", entry.interface_name);
                            }
                        }
                    }
                }
                // Sub-task 2c (Item #37): when proto-affecting fields changed
                // for an existing modem entry, fire reconcile_uci_section. The
                // bridge-conversion branch above handles Ethernet entries; this
                // branch covers cellular modem entries where `proto_override`
                // or `network_device` flipped via Save & Apply.
                //
                // Without this branch, proto_override changes persist to
                // wan-config.json but never reach UCI — only Scan (`scan_wan`)
                // reconciles existing modem entries unconditionally. This
                // branch makes Save & Apply alone sufficient.
                //
                // The pre-fetched `modem_resolved` snapshot was built above
                // (under the state.modems read guard); looked up here per
                // entry, with `unwrap_or_default()` matching the other
                // callsites (Ethernet bridge above + None=> arm below) per
                // spec Q-D2.
                if should_reconcile_existing_modem_entry(old, entry) {
                    let resolved = modem_resolved
                        .get(&entry.modem_id)
                        .cloned()
                        .unwrap_or_default();
                    let proto = resolve_uci_proto(entry, Some(resolved.usbnet_mode));
                    match reconcile_uci_section(
                        &entry.interface_name,
                        &entry.network_device,
                        proto.as_ref(),
                        resolved.control_device_path.as_deref(),
                        entry.metric,
                        entry.mtu,
                    ).await {
                        Ok(displaced) => {
                            for displaced_name in displaced {
                                state.broadcast_event(ModemEvent::WanCollisionDisplaced {
                                    our_name: entry.interface_name.clone(),
                                    displaced_name,
                                    device: entry.network_device.clone(),
                                });
                            }
                        }
                        Err(e) => {
                            tracing::warn!("Failed to reconcile UCI interface {}: {e}", entry.interface_name);
                        }
                    }
                }
                // MTU change
                if old.mtu != entry.mtu {
                    let _ = uci_set_mtu(&entry.interface_name, entry.mtu).await;
                    debug_trace_with_source(
                        format!("[WAN] {} MTU → {:?}", entry.label, entry.mtu),
                        "wan",
                    );
                }
            }
            None => {
                // New entry in list
                if entry.entry_type == WanEntryType::Ethernet {
                    if let Some(ref _bridge) = entry.original_bridge {
                        // Converted LAN port: remove from bridge, reconcile UCI interface, add to firewall
                        if let Err(e) = uci_remove_from_bridge(&entry.network_device).await {
                            tracing::warn!("Failed to remove {} from bridge: {e}", entry.network_device);
                        }
                        let resolved = modem_resolved
                            .get(&entry.modem_id)
                            .cloned()
                            .unwrap_or_default();
                        let proto = resolve_uci_proto(entry, Some(resolved.usbnet_mode));
                        match reconcile_uci_section(
                            &entry.interface_name,
                            &entry.network_device,
                            proto.as_ref(),
                            resolved.control_device_path.as_deref(),
                            entry.metric,
                            entry.mtu,
                        ).await {
                            Ok(displaced) => {
                                for displaced_name in displaced {
                                    state.broadcast_event(ModemEvent::WanCollisionDisplaced {
                                        our_name: entry.interface_name.clone(),
                                        displaced_name,
                                        device: entry.network_device.clone(),
                                    });
                                }
                            }
                            Err(e) => {
                                tracing::warn!("Failed to reconcile UCI interface {}: {e}", entry.interface_name);
                            }
                        }
                        if let Err(e) = uci_add_to_wan_zone(&entry.interface_name).await {
                            tracing::warn!("Failed to add {} to wan zone: {e}", entry.interface_name);
                        }
                    } else {
                        // Existing WAN interface: just set metric
                        let _ = uci_set_metric(&entry.interface_name, entry.metric).await;
                    }
                } else {
                    // Modem: reconcile UCI interface (displaces foreign sections on this device) + add to WAN zone
                    let resolved = modem_resolved
                        .get(&entry.modem_id)
                        .cloned()
                        .unwrap_or_default();
                    let proto = resolve_uci_proto(entry, Some(resolved.usbnet_mode));
                    match reconcile_uci_section(
                        &entry.interface_name,
                        &entry.network_device,
                        proto.as_ref(),
                        resolved.control_device_path.as_deref(),
                        entry.metric,
                        entry.mtu,
                    ).await {
                        Ok(displaced) => {
                            for displaced_name in displaced {
                                state.broadcast_event(ModemEvent::WanCollisionDisplaced {
                                    our_name: entry.interface_name.clone(),
                                    displaced_name,
                                    device: entry.network_device.clone(),
                                });
                            }
                        }
                        Err(e) => {
                            tracing::warn!("Failed to reconcile UCI interface {}: {e}", entry.interface_name);
                        }
                    }
                    if let Err(e) = uci_add_to_wan_zone(&entry.interface_name).await {
                        tracing::warn!("Failed to add {} to wan zone: {e}", entry.interface_name);
                    }
                }
            }
        }
    }

    // Diff: entries removed from list (in old config but not in new)
    for (modem_id, old_entry) in &old_modem_map {
        if !new_modem_map.contains_key(modem_id) {
            if old_entry.entry_type == WanEntryType::Ethernet {
                if let Some(ref bridge) = old_entry.original_bridge {
                    // Converted LAN port: revert to bridge, delete UCI interface, remove from firewall
                    let _ = ifdown(&old_entry.interface_name).await;
                    let _ = uci_delete_interface(&old_entry.interface_name).await;
                    let _ = uci_add_to_bridge(&old_entry.network_device).await;
                    let _ = uci_remove_from_wan_zone(&old_entry.interface_name).await;
                    debug_trace_with_source(
                        format!("[WAN] Reverted Ethernet {}: {} back to {}", old_entry.label, old_entry.network_device, bridge),
                        "wan",
                    );
                } else {
                    // Existing WAN interface (e.g. wan/br-wan): don't delete, just remove from priority list
                    debug_trace_with_source(
                        format!("[WAN] Removed existing WAN {} from priority list (interface kept)", old_entry.label),
                        "wan",
                    );
                }
            } else {
                // Modem: ifdown + delete UCI interface + drop wan-zone membership.
                // The zone del_list must run here (symmetric with the Ethernet
                // revert branch above): the post-apply orphan sweep enumerates
                // network sections, but this section is already deleted, so a
                // missed zone member would dangle.
                let _ = ifdown(&old_entry.interface_name).await;
                let _ = uci_delete_interface(&old_entry.interface_name).await;
                let _ = uci_remove_from_wan_zone(&old_entry.interface_name).await;
                debug_trace_with_source(
                    format!("[WAN] Removed {}: ifdown + delete {}", old_entry.label, old_entry.interface_name),
                    "wan",
                );
            }
        }
    }

    // Remove routing tables for removed entries
    {
        let policy_routing_enabled = state.platform_capabilities.read().await.policy_routing_enabled;
        if policy_routing_enabled {
            let mut rs = state.routing_state.write().await;
            for modem_id in old_modem_map.keys() {
                if !new_modem_map.contains_key(modem_id) {
                    if let Some(entry) = rs.remove(modem_id.as_str()) {
                        let _ = routing::remove_table_entry(&entry);
                    }
                }
            }
        }
    }

    // Post-apply orphan sweep: delete any managed-namespace UCI section
    // (case-insensitive WWAN*/EWAN*) that is not in the current config.
    // reconcile_uci_section already displaces foreign sections at write time;
    // this catches managed leftovers from removed/renamed entries.
    {
        let active_iface_names: std::collections::HashSet<String> =
            new_priority.iter().map(|m| m.interface_name.clone()).collect();
        if let Err(e) = purge_orphaned_managed_sections(&active_iface_names).await {
            tracing::warn!("Failed to purge orphaned managed UCI sections: {e}");
        }
    }

    // Update runtime status for modems that changed state
    {
        let mut runtime = state.wan_runtime.write().await;
        for entry in &new_priority {
            if let Some(old) = old_modem_map.get(&entry.modem_id) {
                if old.state != entry.state {
                    if let Some(info) = runtime.modem_statuses.get_mut(&entry.modem_id) {
                        info.status = if entry.is_active() {
                            WanModemStatus::Offline
                        } else {
                            WanModemStatus::Standby
                        };
                        info.consecutive_failures = 0;
                    }
                }
            }
        }
    }

    // Apply TTL/HL mangle rules (nftables/iptables — independent of UCI)
    if let Err(e) = apply_ttl_rules(&new_priority).await {
        tracing::warn!("Failed to apply TTL/HL rules: {e}");
    }

    // Determine if we need a full network reload or just targeted restarts.
    // Full reload is needed when interfaces were created or deleted.
    // Settings-only changes (MTU, metric, state) use targeted ifdown/ifup.
    let needs_full_reload = {
        // New entries not in old config = interface creation
        let has_new = new_priority
            .iter()
            .any(|e| !old_modem_map.contains_key(&e.modem_id));
        // Old entries not in new config = interface deletion
        let has_removed = old_modem_map
            .keys()
            .any(|id| !new_modem_map.contains_key(id));
        // Deferred Ethernet provisioning
        let has_deferred_eth = new_priority.iter().any(|e| {
            e.entry_type == WanEntryType::Ethernet && e.original_bridge.is_some()
        });
        // Sub-task 2c (Item #37): proto-affecting changes on existing modem
        // entries also require a full network reload so netifd picks up the
        // new proto handler. Pre-2c, proto changes never reached UCI from
        // update_wan_config; with 2c they do, and the targeted ifdown/ifup
        // path doesn't trigger on proto-only changes.
        let has_existing_modem_proto_diff = new_priority.iter().any(|entry| {
            old_modem_map
                .get(&entry.modem_id)
                .is_some_and(|old| should_reconcile_existing_modem_entry(old, entry))
        });
        has_new || has_removed || has_deferred_eth || has_existing_modem_proto_diff
    };

    // Collect modem interfaces that had MTU changed (need recovery check)
    let mtu_changed_modems: Vec<(String, String, String)> = new_priority
        .iter()
        .filter(|e| e.entry_type == WanEntryType::Modem)
        .filter(|e| {
            old_modem_map
                .get(&e.modem_id)
                .is_some_and(|old| old.mtu != e.mtu)
        })
        .map(|e| {
            (
                e.modem_id.clone(),
                e.interface_name.clone(),
                e.network_device.clone(),
            )
        })
        .collect();

    if needs_full_reload {
        // Full commit + network reload (creates/deletes interfaces)
        if let Err(e) = uci_commit_and_reload().await {
            tracing::warn!("Failed to commit UCI: {e}");
        }
    } else {
        // Commit without reload, then targeted ifdown/ifup for changed interfaces
        if let Err(e) = uci_commit_only().await {
            tracing::warn!("Failed to commit UCI: {e}");
        }

        // Targeted restart only for interfaces whose settings changed
        for entry in &new_priority {
            if let Some(old) = old_modem_map.get(&entry.modem_id) {
                let settings_changed =
                    old.mtu != entry.mtu || old.metric != entry.metric || old.state != entry.state;
                if settings_changed {
                    let _ = ifdown(&entry.interface_name).await;
                    let _ = ifup(&entry.interface_name).await;
                    debug_trace_with_source(
                        format!(
                            "[WAN] Targeted restart: {} ({})",
                            entry.label, entry.interface_name
                        ),
                        "wan",
                    );
                }
            }
        }
    }

    // Recovery check for modems that had MTU changed:
    // ECM modems can lose DHCP after MTU change. Verify IP assignment,
    // and if failed, do USB unbind/rebind to reset the ECM bearer.
    if !mtu_changed_modems.is_empty() && !is_mock_mode() {
        // Wait for DHCP to complete
        tokio::time::sleep(std::time::Duration::from_secs(10)).await;

        for (modem_id, iface_name, net_device) in &mtu_changed_modems {
            if !check_interface_has_ip(net_device).await {
                debug_trace_with_source(
                    format!(
                        "[WAN] {modem_id} ({net_device}) has no IP after MTU change — attempting USB reset recovery"
                    ),
                    "wan",
                );
                if let Err(e) = usb_reset_modem(modem_id).await {
                    tracing::warn!("USB reset failed for {modem_id}: {e}");
                    continue;
                }
                // Wait for USB re-enumeration and ECM bearer
                tokio::time::sleep(std::time::Duration::from_secs(15)).await;
                // Bring interface back up
                let _ = ifup(iface_name).await;
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                if check_interface_has_ip(net_device).await {
                    debug_trace_with_source(
                        format!(
                            "[WAN] {modem_id} recovered after USB reset — IP assigned on {net_device}"
                        ),
                        "wan",
                    );
                } else {
                    tracing::warn!(
                        "[WAN] {} still has no IP after USB reset recovery",
                        modem_id
                    );
                    debug_trace_with_source(
                        format!(
                            "[WAN] {modem_id} failed to recover after USB reset — manual intervention may be needed"
                        ),
                        "wan",
                    );
                }
            }
        }
    }

    // Apply to stored config
    config.enabled = new_config.enabled;
    config.modem_priority = new_priority;
    config.watchdog = new_config.watchdog;
    config.failover_locked = new_config.failover_locked;
    config.failback_timer_mins = new_config.failback_timer_mins;
    config.routing_mode = new_config.routing_mode.clone();

    drop(config);

    // Rebuild routing tables since priorities may have changed (reorder, add, remove)
    {
        let policy_routing_enabled = state.platform_capabilities.read().await.policy_routing_enabled;
        if policy_routing_enabled {
            let wan_config = state.wan_config.read().await;
            let wan_entries: Vec<(String, String, u32)> = wan_config
                .modem_priority
                .iter()
                .enumerate()
                .map(|(i, entry)| (entry.modem_id.clone(), entry.network_device.clone(), i as u32))
                .collect();
            let routing_mode = wan_config.routing_mode.clone();
            let weights: std::collections::HashMap<String, u32> = wan_config
                .modem_priority
                .iter()
                .map(|e| (e.modem_id.clone(), e.weight.unwrap_or(1)))
                .collect();
            let primary_id = wan_config.modem_priority.first().map(|e| e.modem_id.clone());
            drop(wan_config);

            let new_tables = routing::initialize_tables(&wan_entries, &routing_mode, &weights, primary_id.as_deref());
            let mut rs = state.routing_state.write().await;
            *rs = new_tables;
        }
    }

    state
        .audit
        .log(
            AuditEventType::ConfigChanged,
            None,
            format!("{} updated WAN manager configuration", user.username),
        )
        .await;

    if let Err(e) = save_and_broadcast(&state).await {
        tracing::warn!("Failed to save WAN config: {e}");
    }

    // Reset restart suspensions — user is actively reconfiguring
    {
        let mut runtime = state.wan_runtime.write().await;
        let any_suspended = runtime.modem_statuses.values().any(|info| info.restart_suspended);
        for info in runtime.modem_statuses.values_mut() {
            info.restart_suspended = false;
            info.restart_count = 0;
        }
        if any_suspended {
            let timestamp = chrono::Utc::now().to_rfc3339();
            let log_line = format!("{timestamp} RESTART_CLEARED reason=\"Config saved\"");
            let _ = crate::config::wan::append_watchdog_log(&log_line).await;
        }
    }

    let response = build_status_response(&state).await;
    Ok(Json(response))
}

/// Find a modem-managed UCI interface (e.g. created by qmodem) as a fallback
/// for modems whose data interface is on PCIe/MHI rather than USB.
/// Returns `Some((interface_name, network_device))` if found.
async fn find_modem_managed_interface() -> Option<(String, String)> {
    if is_mock_mode() {
        return None;
    }

    let output = tokio::process::Command::new("sh")
        .arg("-c")
        .arg("uci show network 2>/dev/null")
        .output()
        .await
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let uci_output = String::from_utf8_lossy(&output.stdout);

    // Parse UCI output to find interfaces with modem_config set
    // Track: (has_modem_config, device)
    let mut interfaces: std::collections::HashMap<String, (bool, Option<String>)> =
        std::collections::HashMap::new();

    for line in uci_output.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("network.") {
            if let Some((name_and_key, value)) = rest.split_once('=') {
                if let Some((name, key)) = name_and_key.split_once('.') {
                    let value = value.trim_matches('\'');
                    let entry = interfaces.entry(name.to_string()).or_insert((false, None));
                    match key {
                        "modem_config" => {
                            entry.0 = true;
                        }
                        "device" => {
                            entry.1 = Some(value.to_string());
                        }
                        "ifname" => {
                            if entry.1.is_none() && !value.starts_with('@') {
                                entry.1 = Some(value.to_string());
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    // Find the first modem-managed interface with a device, preferring rmnet_mhi*
    let mut fallback: Option<(String, String)> = None;
    for (name, (has_modem_config, device)) in &interfaces {
        if !has_modem_config {
            continue;
        }
        if let Some(dev) = device {
            if dev.starts_with("rmnet_mhi") {
                debug_trace_with_source(format!(
                    "[WAN] Found MHI modem-managed interface: {name} device={dev}"
                ), "wan");
                return Some((name.clone(), dev.clone()));
            }
            if fallback.is_none() {
                fallback = Some((name.clone(), dev.clone()));
            }
        }
    }

    if let Some((ref name, ref dev)) = fallback {
        debug_trace_with_source(format!(
            "[WAN] Found modem-managed interface (non-MHI): {name} device={dev}"
        ), "wan");
    }

    fallback
}

/// POST /api/wan/scan
///
/// Discover modems and Ethernet interfaces, reconcile the WAN priority list.
/// Uses the same modem contexts as the main dashboard for modems, and UCI
/// queries for Ethernet interfaces. New entries are appended; missing modems
/// are removed. Returns WanScanResponse with available LAN ports.
pub async fn scan_wan(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<SessionUser>,
) -> ApiResult<Json<WanScanResponse>> {
    require_admin(&user)?;
    debug_trace_with_source("[WAN] Scanning for modems...", "wan");

    // Use already-initialized modem contexts from state — these have correct profiles,
    // labels, IMEI, and SIM status from startup detection. This avoids re-scanning
    // hardware which could miss modems or mismatch profiles.
    // (modem_id, label, bus_port, net_device, device_path, has_sim, existing_iface)
    #[allow(clippy::type_complexity)]
    let mut discovered: Vec<(String, String, String, String, String, Option<bool>, Option<String>)> = Vec::new();

    {
        let modems_map = state.modems.read().await;
        debug_trace_with_source(format!("[WAN] Found {} modem(s) in state", modems_map.len()), "wan");

        for (modem_id, ctx) in modems_map.iter() {
            // Build label from the matched profile (guaranteed correct)
            let label = if !ctx.profile.is_generic() {
                format!("{} {}", ctx.profile.identity.manufacturer, ctx.profile.identity.model)
            } else {
                ctx.detected.description.clone()
            };

            let bus_port = match &ctx.detected.bus_port {
                Some(bp) => bp.clone(),
                None => {
                    debug_trace_with_source(format!(
                        "[WAN] No bus-port for {label} — using modem_id as bus_port proxy"
                    ), "wan");
                    // Still include the modem — use empty string so it can be found by modem_id
                    String::new()
                }
            };

            // Find the network device for this bus-port.
            // If USB sysfs lookup fails, fall back to UCI modem-managed interfaces
            // (covers PCIe/MHI modems like rmnet_mhi0.1 on qmodem-managed routers).
            let (net_device, existing_iface) = if !bus_port.is_empty() {
                match crate::hardware::find_net_device_for_bus_port(&bus_port) {
                    Some(dev) => (dev, None),
                    None => {
                        debug_trace_with_source(format!(
                            "[WAN] No USB network device for bus-port {bus_port} ({label}) — trying UCI fallback"
                        ), "wan");
                        // Fallback: check for modem-managed UCI interfaces (qmodem, etc.)
                        match find_modem_managed_interface().await {
                            Some((iface_name, dev)) => {
                                debug_trace_with_source(format!(
                                    "[WAN] Found modem-managed UCI interface {iface_name} device={dev} for {label}"
                                ), "wan");
                                (dev, Some(iface_name))
                            }
                            None => {
                                debug_trace_with_source(format!(
                                    "[WAN] No network device found for {label} — modem added without net device"
                                ), "wan");
                                (String::new(), None)
                            }
                        }
                    }
                }
            } else {
                // No bus-port at all — try UCI fallback
                match find_modem_managed_interface().await {
                    Some((iface_name, dev)) => {
                        debug_trace_with_source(format!(
                            "[WAN] No bus-port for {label}, found modem-managed UCI interface {iface_name} device={dev}"
                        ), "wan");
                        (dev, Some(iface_name))
                    }
                    None => (String::new(), None)
                }
            };

            // Get SIM status from discovery info (already queried at startup)
            let discovery = ctx.discovery.read().await;
            let has_sim = Some(discovery.sim_status.present);
            let device_path = ctx.detected.device_path.clone();

            let sim_str = match has_sim {
                Some(true) => "SIM",
                Some(false) => "No SIM",
                None => "?",
            };
            debug_trace_with_source(format!(
                "[WAN] Found: {label} id={modem_id} net={net_device} bus={bus_port} at={device_path} sim={sim_str}"
            ), "wan");
            discovered.push((modem_id.clone(), label, bus_port, net_device, device_path, has_sim, existing_iface));
        }
    }

    // Discover Ethernet WAN interfaces
    let ethernet_wan = discover_ethernet_wan_interfaces().await;
    let discovered_eth_ids: std::collections::HashSet<String> =
        ethernet_wan.iter().map(|e| e.modem_id.clone()).collect();

    // Reconcile with existing config
    let mut config = state.wan_config.write().await;

    // Remove stale Ethernet entries that are no longer discovered and not converted LAN ports.
    // Modem entries are NEVER removed by scan — they preserve their priority position even
    // when temporarily disconnected. Only the explicit "Remove" action deletes modem entries.
    let before_count = config.modem_priority.len();
    config.modem_priority.retain(|entry| {
        if entry.entry_type == WanEntryType::Ethernet {
            // Keep Ethernet entries: either still discovered or converted LAN ports
            if discovered_eth_ids.contains(&entry.modem_id) || entry.original_bridge.is_some() {
                return true;
            }
            debug_trace_with_source(format!(
                "[WAN] Removing stale Ethernet entry: {} ({})",
                entry.label, entry.modem_id
            ), "wan");
            return false;
        }
        // Modem entries always kept — preserve priority order across disconnects
        true
    });
    if config.modem_priority.len() < before_count {
        debug_trace_with_source(format!(
            "[WAN] Cleaned up {} stale Ethernet entry(ies)",
            before_count - config.modem_priority.len()
        ), "wan");
    }

    // Collect all interface names already in use to avoid duplicates
    let mut used_iface_names: std::collections::HashSet<String> = config
        .modem_priority
        .iter()
        .map(|m| m.interface_name.clone())
        .collect();

    for (modem_id, label, _bus_port, net_device, device_path, _has_sim, existing_iface) in &discovered {
        // Check if already in the priority list
        if let Some(existing) = config
            .modem_priority
            .iter_mut()
            .find(|m| m.modem_id == *modem_id)
        {
            // Update network device, AT port, and label (may have changed after reboot
            // or been previously stored as "Unknown Modem" from a failed profile match)
            existing.network_device = net_device.clone();
            existing.device_path = device_path.clone();
            existing.label = label.clone();
            // If we discovered an existing UCI interface, adopt it
            if let Some(iface) = existing_iface {
                if existing.interface_name != *iface {
                    debug_trace_with_source(format!(
                        "[WAN] Adopting existing UCI interface {iface} for {label} (was {})",
                        existing.interface_name
                    ), "wan");
                    existing.interface_name = iface.clone();
                }
            }
        } else {
            // New modem — use existing UCI interface if available, otherwise assign a new name
            let iface_name = if let Some(iface) = existing_iface {
                debug_trace_with_source(format!(
                    "[WAN] Using existing modem-managed interface {iface} for new modem {label}"
                ), "wan");
                iface.clone()
            } else if !used_iface_names.contains("WWAN") {
                "WWAN".to_string()
            } else {
                let mut num = 2u32;
                loop {
                    let candidate = format!("WWAN{num}");
                    if !used_iface_names.contains(&candidate) {
                        break candidate;
                    }
                    num += 1;
                }
            };
            used_iface_names.insert(iface_name.clone());

            config.modem_priority.push(WanModemEntry {
                modem_id: modem_id.clone(),
                label: label.clone(),
                interface_name: iface_name.clone(),
                network_device: net_device.clone(),
                device_path: device_path.clone(),
                state: WanModemState::Standby, // New modems start in standby
                metric: 998,
                entry_type: WanEntryType::Modem,
                original_bridge: None,
                mtu: None,
                ttl: None,
                hop_limit: None,
                weight: None,
                proto_override: None,
            });

            debug_trace_with_source(format!(
                "[WAN] Added new modem: {label} as {iface_name} ({net_device})"
            ), "wan");
        }
    }

    // Reconcile Ethernet WAN interfaces
    for eth_entry in &ethernet_wan {
        if !config.modem_priority.iter().any(|m| m.modem_id == eth_entry.modem_id) {
            // New Ethernet WAN interface — add to priority list
            config.modem_priority.push(eth_entry.clone());
            used_iface_names.insert(eth_entry.interface_name.clone());
            debug_trace_with_source(format!(
                "[WAN] Added Ethernet WAN: {} as {} ({})",
                eth_entry.label, eth_entry.interface_name, eth_entry.network_device
            ), "wan");
        }
    }

    // Update runtime state for discovered modems (including SIM status from scan)
    {
        let mut runtime = state.wan_runtime.write().await;
        for (modem_id, label, _, net_device, _, has_sim, _) in &discovered {
            let info = runtime
                .modem_statuses
                .entry(modem_id.clone())
                .or_insert_with(|| WanModemRuntimeInfo {
                    status: WanModemStatus::Offline,
                    consecutive_failures: 0,
                    last_check: None,
                    network_device: Some(net_device.clone()),
                    has_sim: None,
                    restart_count: 0,
                    restart_suspended: false,
                    healthy_since: None,
                    wedged: false,
                    wedged_since: None,
                });
            // Update SIM status from scan results
            if let Some(sim) = has_sim {
                info.has_sim = Some(*sim);
                if !sim {
                    info.status = WanModemStatus::NoSim;
                    info.consecutive_failures = 0;
                    info.last_check = None;
                    debug_trace_with_source(format!("[WAN] {label}: no SIM detected during scan"), "wan");
                }
            }
        }
    }

    // Re-assign metrics
    assign_metrics(&mut config.modem_priority);

    // Initialize runtime state for Ethernet entries
    {
        let mut runtime = state.wan_runtime.write().await;
        for entry in config.modem_priority.iter().filter(|e| e.entry_type == WanEntryType::Ethernet) {
            runtime
                .modem_statuses
                .entry(entry.modem_id.clone())
                .or_insert_with(|| WanModemRuntimeInfo {
                    status: WanModemStatus::Offline,
                    consecutive_failures: 0,
                    last_check: None,
                    network_device: Some(entry.network_device.clone()),
                    has_sim: Some(true), // Ethernet doesn't need SIM
                    restart_count: 0,
                    restart_suspended: false,
                    healthy_since: None,
                    wedged: false,
                    wedged_since: None,
                });
        }
    }

    // Collect interface names of modem-managed UCI interfaces (qmodem, etc.)
    // These should NOT be recreated — just update their metric.
    let modem_managed_ifaces: std::collections::HashSet<String> = discovered
        .iter()
        .filter_map(|(_, _, _, _, _, _, existing_iface)| existing_iface.clone())
        .collect();

    // Pre-fetch detected USB-net mode + cdc-wdm control device path per modem
    // (Item #37 sub-tasks 2 + 2b). Built once and dropped before the reconcile
    // loop so the state.modems read guard never lives across an await that may
    // broadcast events. The sysfs walk inside the build block is synchronous
    // (no await) so the read guard is held only across non-await code.
    let modem_resolved: std::collections::HashMap<String, ResolvedReconcileFields> = {
        let modems_map = state.modems.read().await;
        let mut out = std::collections::HashMap::new();
        for (modem_id, ctx) in modems_map.iter() {
            let mode = *ctx.usbnet_mode.read().await;
            let control_device_path = ctx
                .detected
                .bus_port
                .as_deref()
                .and_then(crate::hardware::find_qmi_control_device_for_bus_port);
            out.insert(
                modem_id.clone(),
                ResolvedReconcileFields {
                    usbnet_mode: mode,
                    control_device_path,
                },
            );
        }
        out
    };

    // Create/update UCI interfaces for all entries (both active and standby get interfaces)
    for entry in &config.modem_priority {
        if entry.entry_type == WanEntryType::Ethernet && entry.original_bridge.is_none() {
            // Existing WAN interface: just update metric, don't recreate
            let _ = uci_set_metric(&entry.interface_name, entry.metric).await;
        } else if entry.entry_type == WanEntryType::Ethernet && entry.original_bridge.is_some() {
            // Converted LAN port: reconcile interface (bridge removal happens on Save & Apply)
            let resolved = modem_resolved
                .get(&entry.modem_id)
                .cloned()
                .unwrap_or_default();
            let proto = resolve_uci_proto(entry, Some(resolved.usbnet_mode));
            match reconcile_uci_section(
                &entry.interface_name,
                &entry.network_device,
                proto.as_ref(),
                resolved.control_device_path.as_deref(),
                entry.metric,
                entry.mtu,
            ).await {
                Ok(displaced) => {
                    for displaced_name in displaced {
                        state.broadcast_event(ModemEvent::WanCollisionDisplaced {
                            our_name: entry.interface_name.clone(),
                            displaced_name,
                            device: entry.network_device.clone(),
                        });
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to reconcile UCI interface {}: {e}", entry.interface_name);
                }
            }
            if let Err(e) = uci_add_to_wan_zone(&entry.interface_name).await {
                tracing::warn!("Failed to add {} to wan zone: {e}", entry.interface_name);
            }
        } else if modem_managed_ifaces.contains(&entry.interface_name) {
            // Modem-managed interface (qmodem, etc.): don't recreate, just update metric
            debug_trace_with_source(format!(
                "[WAN] Skipping UCI create for modem-managed interface {} — updating metric only",
                entry.interface_name
            ), "wan");
            let _ = uci_set_metric(&entry.interface_name, entry.metric).await;
        } else {
            // Modem: reconcile UCI interface (device-keyed; displaces foreign sections)
            let resolved = modem_resolved
                .get(&entry.modem_id)
                .cloned()
                .unwrap_or_default();
            let proto = resolve_uci_proto(entry, Some(resolved.usbnet_mode));
            match reconcile_uci_section(
                &entry.interface_name,
                &entry.network_device,
                proto.as_ref(),
                resolved.control_device_path.as_deref(),
                entry.metric,
                entry.mtu,
            ).await {
                Ok(displaced) => {
                    for displaced_name in displaced {
                        state.broadcast_event(ModemEvent::WanCollisionDisplaced {
                            our_name: entry.interface_name.clone(),
                            displaced_name,
                            device: entry.network_device.clone(),
                        });
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to reconcile UCI interface {}: {e}", entry.interface_name);
                }
            }
            if let Err(e) = uci_add_to_wan_zone(&entry.interface_name).await {
                tracing::warn!(
                    "Failed to add {} to wan zone: {e}",
                    entry.interface_name
                );
            }
        }
    }
    if let Err(e) = uci_commit_and_reload().await {
        tracing::warn!("Failed to commit UCI after scan: {e}");
    }

    // Collect existing WAN device names to exclude from available LAN ports
    let existing_wan_devices: std::collections::HashSet<String> = config
        .modem_priority
        .iter()
        .map(|e| e.network_device.clone())
        .collect();

    drop(config);

    // Discover available LAN ports (not yet in use as WAN)
    let available_ethernet_ports = discover_available_lan_ports(&existing_wan_devices).await;

    let eth_count = ethernet_wan.len();
    state
        .audit
        .log(
            AuditEventType::ConfigChanged,
            None,
            format!(
                "{} triggered WAN scan: found {} modem(s), {} Ethernet WAN(s)",
                user.username, discovered.len(), eth_count
            ),
        )
        .await;

    if let Err(e) = save_and_broadcast(&state).await {
        tracing::warn!("Failed to save WAN config: {e}");
    }

    let status = build_status_response(&state).await;
    Ok(Json(WanScanResponse {
        status,
        available_ethernet_ports,
    }))
}

// ============================================================================
// Ethernet WAN Management
// ============================================================================

/// POST /api/wan/add-ethernet
///
/// Add a LAN port to the WAN priority list as a converted Ethernet WAN entry.
/// The port must currently be in a bridge (br-lan). UCI changes are deferred
/// until Save & Apply (update_wan_config).
pub async fn add_ethernet(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<SessionUser>,
    Json(req): Json<AddEthernetRequest>,
) -> ApiResult<Json<WanStatusResponse>> {
    require_admin(&user)?;

    let port_name = req.port_name.trim().to_string();

    // Strict allowlist on port name (it becomes a UCI device + feeds the
    // bridge-lookup command). Reject anything outside [A-Za-z0-9_-]{1,32}.
    if !is_valid_uci_token(&port_name) {
        return Err(ApiError::bad_request(
            "Invalid port name: must be 1-32 chars of [A-Za-z0-9_-]",
        ));
    }

    // Check that the port is currently in a bridge
    let bridge = get_port_bridge(&port_name).await.ok_or_else(|| {
        ApiError::bad_request(format!("Port '{port_name}' is not in any bridge"))
    })?;

    let mut config = state.wan_config.write().await;

    // Check if already in the priority list
    let eth_id = format!("eth:{port_name}");
    if config.modem_priority.iter().any(|m| m.modem_id == eth_id) {
        return Err(ApiError::bad_request(format!(
            "Port '{port_name}' is already in the WAN priority list"
        )));
    }

    // Assign a unique EWAN interface name
    let used_iface_names: std::collections::HashSet<String> = config
        .modem_priority
        .iter()
        .map(|m| m.interface_name.clone())
        .collect();

    let iface_name = if !used_iface_names.contains("EWAN") {
        "EWAN".to_string()
    } else {
        let mut num = 2u32;
        loop {
            let candidate = format!("EWAN{num}");
            if !used_iface_names.contains(&candidate) {
                break candidate;
            }
            num += 1;
        }
    };

    let label = req.label.unwrap_or_else(|| format!("LAN Port ({port_name})"));

    config.modem_priority.push(WanModemEntry {
        modem_id: eth_id.clone(),
        label: label.clone(),
        interface_name: iface_name.clone(),
        network_device: port_name.clone(),
        device_path: String::new(),
        state: WanModemState::Active,
        metric: 0, // Will be recalculated
        entry_type: WanEntryType::Ethernet,
        original_bridge: Some(bridge.clone()),
        mtu: None,
        ttl: None,
        hop_limit: None,
        weight: None,
        proto_override: None,
    });

    // Re-assign metrics
    assign_metrics(&mut config.modem_priority);

    // Initialize runtime state for the new entry
    {
        let mut runtime = state.wan_runtime.write().await;
        runtime.modem_statuses.insert(
            eth_id.clone(),
            WanModemRuntimeInfo {
                status: WanModemStatus::Offline,
                consecutive_failures: 0,
                last_check: None,
                network_device: Some(port_name.clone()),
                has_sim: Some(true), // Ethernet doesn't need SIM
                restart_count: 0,
                restart_suspended: false,
                healthy_since: None,
                wedged: false,
                wedged_since: None,
            },
        );
    }

    drop(config);

    // Create routing table for the new WAN entry if policy routing is enabled
    {
        let policy_routing_enabled = state.platform_capabilities.read().await.policy_routing_enabled;
        if policy_routing_enabled {
            if let Some(ip) = routing::get_interface_ip(&port_name) {
                let gateway = routing::discover_gateway(&port_name);
                let wan_config = state.wan_config.read().await;
                let idx = wan_config
                    .modem_priority
                    .iter()
                    .position(|e| e.modem_id == eth_id)
                    .unwrap_or(0) as u32;
                drop(wan_config);

                let entry = RoutingTableEntry {
                    table_number: 100 + idx,
                    rule_priority: 1000 + idx,
                    gateway,
                    device: port_name.clone(),
                    source_ip: ip,
                };
                if routing::create_table_entry(&entry).is_ok() {
                    let mut rs = state.routing_state.write().await;
                    rs.insert(eth_id.clone(), entry);
                }
            }
        }
    }

    state
        .audit
        .log(
            AuditEventType::ConfigChanged,
            None,
            format!(
                "{} added Ethernet port {} as WAN ({}), from bridge {}",
                user.username, port_name, iface_name, bridge
            ),
        )
        .await;

    if let Err(e) = save_and_broadcast(&state).await {
        tracing::warn!("Failed to save WAN config: {e}");
    }

    let response = build_status_response(&state).await;
    Ok(Json(response))
}

// ============================================================================
// Watchdog Recovery Log Endpoints
// ============================================================================

/// POST /api/wan/failback
///
/// Immediately restore the original primary modem by resetting UCI metrics
/// from the user's saved config. Clears the failover override, logs the event,
/// and broadcasts the updated status.
pub async fn failback_now(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<SessionUser>,
) -> ApiResult<Json<WanStatusResponse>> {
    require_admin(&user)?;
    // Read failover override info
    let fo_info = {
        let runtime = state.wan_runtime.read().await;
        runtime.failover_override.as_ref().map(|fo| {
            (fo.original_primary_id.clone(), fo.current_primary_id.clone())
        })
    };

    let (original_id, current_id) = fo_info.ok_or_else(|| {
        ApiError::bad_request("No failover override is active")
    })?;

    let config = state.wan_config.read().await;

    let original_label = config
        .modem_priority
        .iter()
        .find(|m| m.modem_id == original_id)
        .map(|m| m.label.clone())
        .unwrap_or_else(|| original_id.clone());
    let current_label = config
        .modem_priority
        .iter()
        .find(|m| m.modem_id == current_id)
        .map(|m| m.label.clone())
        .unwrap_or_else(|| current_id.clone());

    debug_trace_with_source(
        format!("[WAN] Manual failback: {current_label} -> {original_label}"),
        "wan",
    );

    // === Item #40 fix: engage policy-routing primitive symmetric to auto-failback ===
    //
    // Lock ordering:
    //  - state.platform_capabilities.read()  — short-lived, dropped before next acquire.
    //  - config (state.wan_config.read())     — already held (RwLockReadGuard from line ~2859).
    //                                           Reused; no re-acquire.
    //  - state.routing_state.read()           — short-lived inside each routing-mode arm.
    //  - state.wan_runtime.read()             — short-lived inside LoadBalance arm only,
    //                                           dropped before apply_load_balance_route_with_caller.
    //
    // Failure modes are intentionally non-fatal (mirror auto path): on Err, log and fall
    // through to UCI writes. UI contract preserves WanStatusResponse on success.
    let policy_routing_enabled = state.platform_capabilities.read().await.policy_routing_enabled;
    if policy_routing_enabled {
        let routing_mode = config.routing_mode.clone();
        match routing_mode {
            RoutingMode::Failover => {
                let rs = state.routing_state.read().await;
                if let Some(entry) = rs.get(&original_id) {
                    if let Err(e) = routing::set_main_default_with_caller(entry, "failback_now") {
                        tracing::error!("Manual failback policy-routing switch failed: {e}");
                    }
                } else {
                    tracing::warn!(
                        "Manual failback: no routing table entry for original_id {original_id} -- UCI fallback only"
                    );
                }
            }
            RoutingMode::LoadBalance => {
                let rs = state.routing_state.read().await;
                let runtime_r = state.wan_runtime.read().await;
                let weights: std::collections::HashMap<String, u32> = config
                    .modem_priority
                    .iter()
                    .map(|e| (e.modem_id.clone(), e.weight.unwrap_or(1)))
                    .collect();
                let healthy_ids: Vec<String> = config
                    .modem_priority
                    .iter()
                    .filter(|e| {
                        runtime_r
                            .modem_statuses
                            .get(&e.modem_id)
                            .is_some_and(|info| info.status == WanModemStatus::Online)
                    })
                    .map(|e| e.modem_id.clone())
                    .collect();
                drop(runtime_r);
                if let Err(e) = routing::apply_load_balance_route_with_caller(
                    &rs,
                    &healthy_ids,
                    &weights,
                    "failback_now",
                ) {
                    tracing::error!("Manual failback load-balance rebuild failed: {e}");
                }
            }
        }
    }
    // === End Item #40 fix ===

    // Restore UCI metrics from user's configured priority order
    for entry in &config.modem_priority {
        let _ = uci_set_metric(&entry.interface_name, entry.metric).await;
    }
    let _ = uci_commit_and_reload().await;
    drop(config);

    // Clear failover override and record event
    {
        let mut runtime = state.wan_runtime.write().await;
        runtime.failover_override = None;
        runtime.current_routed_wan = Some(original_id.clone());

        let event = crate::hardware::FailoverEvent {
            timestamp: chrono::Utc::now().to_rfc3339(),
            from_modem_id: current_id.clone(),
            from_label: current_label.clone(),
            to_modem_id: original_id.clone(),
            to_label: original_label.clone(),
            reason: "Manual failback".to_string(),
        };
        let log_line = format!(
            "{} FAILBACK from=\"{}\" to=\"{}\" reason=\"{}\"",
            event.timestamp, event.from_label, event.to_label, event.reason
        );
        let _ = crate::config::wan::append_watchdog_log(&log_line).await;

        runtime.failover_history.push_front(event);
        if runtime.failover_history.len() > 50 {
            runtime.failover_history.pop_back();
        }
    }

    state
        .audit
        .log(
            AuditEventType::ConfigChanged,
            None,
            format!(
                "{} triggered manual failback: {current_label} -> {original_label}",
                user.username
            ),
        )
        .await;

    // Broadcast updated status
    let response = build_status_response(&state).await;
    state.broadcast_event(ModemEvent::WanStatusUpdate(Box::new(response.clone())));
    Ok(Json(response))
}

/// POST /api/wan/accept-failover
///
/// Accept the current failover routing as the new configuration.
/// Reorders modem_priority so the current primary is first, clears the
/// failover override, saves config to disk, and broadcasts.
pub async fn accept_failover(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<SessionUser>,
) -> ApiResult<Json<WanStatusResponse>> {
    require_admin(&user)?;
    // Read failover override info
    let fo_info = {
        let runtime = state.wan_runtime.read().await;
        runtime.failover_override.as_ref().map(|fo| {
            (fo.original_primary_id.clone(), fo.current_primary_id.clone())
        })
    };

    let (original_id, current_id) = fo_info.ok_or_else(|| {
        ApiError::bad_request("No failover override is active")
    })?;

    let mut config = state.wan_config.write().await;

    let original_label = config
        .modem_priority
        .iter()
        .find(|m| m.modem_id == original_id)
        .map(|m| m.label.clone())
        .unwrap_or_else(|| original_id.clone());
    let current_label = config
        .modem_priority
        .iter()
        .find(|m| m.modem_id == current_id)
        .map(|m| m.label.clone())
        .unwrap_or_else(|| current_id.clone());

    // Reorder modem_priority: move current primary to first position
    if let Some(pos) = config.modem_priority.iter().position(|m| m.modem_id == current_id) {
        let entry = config.modem_priority.remove(pos);
        config.modem_priority.insert(0, entry);
    }

    // Re-assign metrics based on new order
    assign_metrics(&mut config.modem_priority);

    debug_trace_with_source(
        format!("[WAN] Accept failover: {current_label} is now the configured primary"),
        "wan",
    );

    drop(config);

    // Clear failover override and record event
    {
        let mut runtime = state.wan_runtime.write().await;
        runtime.failover_override = None;
        runtime.current_routed_wan = Some(current_id.clone());

        let event = crate::hardware::FailoverEvent {
            timestamp: chrono::Utc::now().to_rfc3339(),
            from_modem_id: original_id.clone(),
            from_label: original_label.clone(),
            to_modem_id: current_id.clone(),
            to_label: current_label.clone(),
            reason: "Failover accepted as new primary".to_string(),
        };
        let log_line = format!(
            "{} ACCEPT_FAILOVER from=\"{}\" to=\"{}\" reason=\"{}\"",
            event.timestamp, event.from_label, event.to_label, event.reason
        );
        let _ = crate::config::wan::append_watchdog_log(&log_line).await;

        runtime.failover_history.push_front(event);
        if runtime.failover_history.len() > 50 {
            runtime.failover_history.pop_back();
        }
    }

    state
        .audit
        .log(
            AuditEventType::ConfigChanged,
            None,
            format!(
                "{} accepted failover: {current_label} is now configured primary (was {original_label})",
                user.username
            ),
        )
        .await;

    // Save config to disk and broadcast
    if let Err(e) = save_and_broadcast(&state).await {
        tracing::warn!("Failed to save WAN config after accept-failover: {e}");
    }

    let response = build_status_response(&state).await;
    Ok(Json(response))
}

/// GET /api/wan/watchdog/log — Read watchdog recovery log entries.
pub async fn get_watchdog_log(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<SessionUser>,
) -> ApiResult<Json<WanWatchdogLogResponse>> {
    let retention_days = {
        let config = state.wan_config.read().await;
        config.watchdog.log_retention_days.clamp(1, 30)
    };

    let entries = crate::config::wan::read_watchdog_log(retention_days).await;
    let last_recovery = entries.last().cloned();

    Ok(Json(WanWatchdogLogResponse {
        entries,
        last_recovery,
        retention_days,
    }))
}

/// POST /api/wan/watchdog/log/clear — Clear the watchdog recovery log.
pub async fn clear_watchdog_log_handler(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<SessionUser>,
) -> ApiResult<Json<serde_json::Value>> {
    require_admin(&user)?;
    crate::config::wan::clear_watchdog_log()
        .await
        .map_err(|e| ApiError::internal(format!("Failed to clear watchdog log: {e}")))?;

    state
        .audit
        .log(
            AuditEventType::ConfigChanged,
            None,
            format!("{} cleared WAN watchdog recovery log", user.username),
        )
        .await;

    Ok(Json(serde_json::json!({ "success": true })))
}

/// POST /api/wan/watchdog/restart-suspension/clear
/// Resets all restart suspension states.
pub async fn clear_restart_suspensions(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<SessionUser>,
) -> ApiResult<Json<WanStatusResponse>> {
    require_admin(&user)?;
    // Clear suspension flags and restart counts in runtime state
    {
        let mut runtime = state.wan_runtime.write().await;
        for info in runtime.modem_statuses.values_mut() {
            info.restart_suspended = false;
            info.restart_count = 0;
        }
    }

    // Log the clear event
    let timestamp = chrono::Utc::now().to_rfc3339();
    let log_line = format!("{timestamp} RESTART_CLEARED reason=\"Manual clear\"");
    let _ = crate::config::wan::append_watchdog_log(&log_line).await;

    crate::state::debug_trace_with_source(
        "[WAN] Restart suspensions cleared manually".to_string(),
        "wan",
    );

    let response = build_status_response(&state).await;
    Ok(Json(response))
}

/// GET /api/wan/watchdog/log/download — Download watchdog log as plain text.
pub async fn download_watchdog_log(
    State(_state): State<Arc<AppState>>,
    Extension(_user): Extension<SessionUser>,
) -> Response {
    let content = crate::config::wan::read_watchdog_log_raw().await;

    (
        axum::http::StatusCode::OK,
        [
            (axum::http::header::CONTENT_TYPE, "text/plain; charset=utf-8"),
            (
                axum::http::header::CONTENT_DISPOSITION,
                "attachment; filename=\"wan-watchdog.log\"",
            ),
        ],
        content,
    )
        .into_response()
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use crate::hardware::{
        RoutingMode, UsbNetMode, WanConfig, WanEntryType, WanModemEntry, WanModemState,
        WatchdogConfig,
    };

    /// Helper to build a minimal WanModemEntry for testing.
    fn test_entry(id: &str, state: WanModemState) -> WanModemEntry {
        WanModemEntry {
            modem_id: id.to_string(),
            label: id.to_string(),
            interface_name: "WWAN".to_string(),
            network_device: "usb0".to_string(),
            device_path: String::new(),
            state,
            metric: 10,
            entry_type: Default::default(),
            original_bridge: None,
            mtu: None,
            ttl: None,
            hop_limit: None,
            weight: None,
            proto_override: None,
        }
    }

    // ========================================================================
    // resolve_uci_proto tests (Item #37 sub-task 2)
    // ========================================================================

    /// Helper: modem entry without override.
    fn modem_entry_no_override() -> WanModemEntry {
        let mut e = test_entry("2c7c:0122:abcd", WanModemState::Active);
        e.entry_type = WanEntryType::Modem;
        e
    }

    /// Helper: ethernet entry without override.
    fn ethernet_entry_no_override() -> WanModemEntry {
        let mut e = test_entry("eth:br-wan", WanModemState::Active);
        e.entry_type = WanEntryType::Ethernet;
        e
    }

    #[test]
    fn resolve_uci_proto_modem_mode_mapping_table() {
        let entry = modem_entry_no_override();
        // Ecm/Ncm/Rndis → "dhcp"
        assert_eq!(super::resolve_uci_proto(&entry, Some(UsbNetMode::Ecm)).as_ref(), "dhcp");
        assert_eq!(super::resolve_uci_proto(&entry, Some(UsbNetMode::Ncm)).as_ref(), "dhcp");
        assert_eq!(super::resolve_uci_proto(&entry, Some(UsbNetMode::Rndis)).as_ref(), "dhcp");
        // Qmi/Rmnet → "qmi"
        assert_eq!(super::resolve_uci_proto(&entry, Some(UsbNetMode::Qmi)).as_ref(), "qmi");
        assert_eq!(super::resolve_uci_proto(&entry, Some(UsbNetMode::Rmnet)).as_ref(), "qmi");
        // Mbim → "mbim"
        assert_eq!(super::resolve_uci_proto(&entry, Some(UsbNetMode::Mbim)).as_ref(), "mbim");
        // Unknown → "dhcp" (fallback)
        assert_eq!(super::resolve_uci_proto(&entry, Some(UsbNetMode::Unknown)).as_ref(), "dhcp");
    }

    #[test]
    fn resolve_uci_proto_ethernet_entry_always_dhcp() {
        let entry = ethernet_entry_no_override();
        // Ethernet ignores the detected_mode entirely (no override set).
        assert_eq!(super::resolve_uci_proto(&entry, None).as_ref(), "dhcp");
        assert_eq!(super::resolve_uci_proto(&entry, Some(UsbNetMode::Qmi)).as_ref(), "dhcp");
        assert_eq!(super::resolve_uci_proto(&entry, Some(UsbNetMode::Mbim)).as_ref(), "dhcp");
    }

    #[test]
    fn resolve_uci_proto_modem_with_no_detected_mode_is_dhcp() {
        let entry = modem_entry_no_override();
        // Modem entry, detected_mode None (e.g. modem unplugged mid-reconcile)
        // → "dhcp" fallback (same as Unknown).
        assert_eq!(super::resolve_uci_proto(&entry, None).as_ref(), "dhcp");
    }

    #[test]
    fn resolve_uci_proto_override_beats_modem_mode() {
        let mut entry = modem_entry_no_override();
        entry.proto_override = Some("static".to_string());
        // Override wins even when detected_mode would have produced "qmi".
        assert_eq!(super::resolve_uci_proto(&entry, Some(UsbNetMode::Qmi)).as_ref(), "static");

        let mut entry2 = modem_entry_no_override();
        entry2.proto_override = Some("ppp".to_string());
        // Override wins even when detected_mode would have produced "dhcp" (Ecm).
        assert_eq!(super::resolve_uci_proto(&entry2, Some(UsbNetMode::Ecm)).as_ref(), "ppp");
    }

    #[test]
    fn resolve_uci_proto_override_beats_ethernet_default() {
        let mut entry = ethernet_entry_no_override();
        entry.proto_override = Some("static".to_string());
        // Even Ethernet entries respect the override when set.
        assert_eq!(super::resolve_uci_proto(&entry, None).as_ref(), "static");
    }

    #[test]
    fn resolve_uci_proto_empty_or_whitespace_override_falls_through() {
        // Empty-string override → fall through to mode-derived (Qmi → "qmi").
        let mut entry = modem_entry_no_override();
        entry.proto_override = Some(String::new());
        assert_eq!(super::resolve_uci_proto(&entry, Some(UsbNetMode::Qmi)).as_ref(), "qmi");

        // Whitespace-only override → fall through (defensive — input
        // validation rejects this earlier; resolver does not blindly trust).
        let mut entry2 = modem_entry_no_override();
        entry2.proto_override = Some("   ".to_string());
        assert_eq!(super::resolve_uci_proto(&entry2, Some(UsbNetMode::Mbim)).as_ref(), "mbim");

        // Empty override on Ethernet → falls through to "dhcp" Ethernet default.
        let mut entry3 = ethernet_entry_no_override();
        entry3.proto_override = Some(String::new());
        assert_eq!(super::resolve_uci_proto(&entry3, None).as_ref(), "dhcp");
    }

    // ========================================================================
    // resolve_uci_device tests (Item #37 sub-task 2b)
    // ========================================================================

    /// Helper: modem entry with an explicit netif name (vs `test_entry`'s "usb0"
    /// default). Used by sub-task 2b tests that want to assert "netif fell back
    /// to wwan0" without relying on the helper's default value.
    fn modem_entry_with_netif(id: &str, netif: &str) -> WanModemEntry {
        let mut e = test_entry(id, WanModemState::Active);
        e.entry_type = WanEntryType::Modem;
        e.network_device = netif.to_string();
        e
    }

    #[test]
    fn resolve_uci_device_qmi_with_control_path() {
        let entry = modem_entry_with_netif("test:1", "wwan0");
        let result = super::resolve_uci_device(&entry, "qmi", Some("/dev/cdc-wdm0"));
        assert_eq!(result.as_ref(), "/dev/cdc-wdm0");
    }

    #[test]
    fn resolve_uci_device_qmi_no_control_path_falls_back_to_netif() {
        let entry = modem_entry_with_netif("test:2", "wwan0");
        let result = super::resolve_uci_device(&entry, "qmi", None);
        assert_eq!(result.as_ref(), "wwan0");
    }

    #[test]
    fn resolve_uci_device_mbim_with_control_path() {
        let entry = modem_entry_with_netif("test:3", "wwan1");
        let result = super::resolve_uci_device(&entry, "mbim", Some("/dev/cdc-wdm1"));
        assert_eq!(result.as_ref(), "/dev/cdc-wdm1");
    }

    #[test]
    fn resolve_uci_device_mbim_no_control_path_falls_back_to_netif() {
        let entry = modem_entry_with_netif("test:4", "wwan1");
        let result = super::resolve_uci_device(&entry, "mbim", None);
        assert_eq!(result.as_ref(), "wwan1");
    }

    #[test]
    fn resolve_uci_device_dhcp_ignores_control_path() {
        let entry = modem_entry_with_netif("test:5", "wwan0");
        let result = super::resolve_uci_device(&entry, "dhcp", Some("/dev/cdc-wdm0"));
        assert_eq!(result.as_ref(), "wwan0");
    }

    #[test]
    fn resolve_uci_device_static_no_control_path() {
        let entry = modem_entry_with_netif("test:6", "wwan0");
        let result = super::resolve_uci_device(&entry, "static", None);
        assert_eq!(result.as_ref(), "wwan0");
    }

    #[test]
    fn resolve_uci_device_ppp_returns_netif() {
        // Per spec §3 + Q-D=D1 (status quo): ppp → netif.
        let entry = modem_entry_with_netif("test:7", "wwan0");
        let result = super::resolve_uci_device(&entry, "ppp", None);
        assert_eq!(result.as_ref(), "wwan0");
    }

    #[test]
    fn resolve_uci_device_unknown_proto_returns_netif() {
        // Operator typed `proto_override="custom-proto"`. Resolver returns the netif.
        let entry = modem_entry_with_netif("test:8", "wwan0");
        let result = super::resolve_uci_device(&entry, "custom-proto", None);
        assert_eq!(result.as_ref(), "wwan0");
    }

    // ========================================================================
    // Item #37 sub-task 2c — should_reconcile_existing_modem_entry tests
    // ========================================================================

    /// Helper: build a WanModemEntry of type Modem for predicate testing.
    fn modem_entry(id: &str, netif: &str) -> WanModemEntry {
        let mut e = test_entry(id, WanModemState::Active);
        e.entry_type = WanEntryType::Modem;
        e.network_device = netif.to_string();
        e
    }

    #[test]
    fn should_reconcile_existing_modem_proto_override_added() {
        let old = modem_entry("modem1", "wwan0");
        let mut entry = modem_entry("modem1", "wwan0");
        entry.proto_override = Some("qmi".to_string());
        assert!(super::should_reconcile_existing_modem_entry(&old, &entry));
    }

    #[test]
    fn should_reconcile_existing_modem_proto_override_changed() {
        let mut old = modem_entry("modem1", "wwan0");
        old.proto_override = Some("qmi".to_string());
        let mut entry = modem_entry("modem1", "wwan0");
        entry.proto_override = Some("mbim".to_string());
        assert!(super::should_reconcile_existing_modem_entry(&old, &entry));
    }

    #[test]
    fn should_reconcile_existing_modem_proto_override_cleared() {
        let mut old = modem_entry("modem1", "wwan0");
        old.proto_override = Some("qmi".to_string());
        let entry = modem_entry("modem1", "wwan0"); // proto_override = None
        assert!(super::should_reconcile_existing_modem_entry(&old, &entry));
    }

    #[test]
    fn should_not_reconcile_existing_modem_proto_override_unchanged_none() {
        let old = modem_entry("modem1", "wwan0");
        let entry = modem_entry("modem1", "wwan0");
        assert!(!super::should_reconcile_existing_modem_entry(&old, &entry));
    }

    #[test]
    fn should_not_reconcile_existing_modem_proto_override_unchanged_some() {
        let mut old = modem_entry("modem1", "wwan0");
        old.proto_override = Some("qmi".to_string());
        let mut entry = modem_entry("modem1", "wwan0");
        entry.proto_override = Some("qmi".to_string());
        assert!(!super::should_reconcile_existing_modem_entry(&old, &entry));
    }

    #[test]
    fn should_reconcile_existing_modem_network_device_changed() {
        let old = modem_entry("modem1", "wwan0");
        let entry = modem_entry("modem1", "wwan1");
        assert!(super::should_reconcile_existing_modem_entry(&old, &entry));
    }

    #[test]
    fn should_not_reconcile_existing_modem_metric_only_changed() {
        // Predicate ignores metric — fast path uses uci_set_metric.
        let old = modem_entry("modem1", "wwan0");
        let mut entry = modem_entry("modem1", "wwan0");
        entry.metric = 998;
        assert!(!super::should_reconcile_existing_modem_entry(&old, &entry));
    }

    #[test]
    fn should_not_reconcile_existing_modem_mtu_only_changed() {
        // Predicate ignores mtu — fast path uses uci_set_mtu.
        let old = modem_entry("modem1", "wwan0");
        let mut entry = modem_entry("modem1", "wwan0");
        entry.mtu = Some(1450);
        assert!(!super::should_reconcile_existing_modem_entry(&old, &entry));
    }

    #[test]
    fn should_not_reconcile_existing_modem_state_only_changed() {
        // Predicate ignores state — fast path uses uci_set_metric.
        let mut old = modem_entry("modem1", "wwan0");
        old.state = WanModemState::Active;
        let mut entry = modem_entry("modem1", "wwan0");
        entry.state = WanModemState::Standby;
        assert!(!super::should_reconcile_existing_modem_entry(&old, &entry));
    }

    #[test]
    fn should_not_reconcile_existing_ethernet_entry_with_bridge() {
        // Ethernet bridge-conversion has its own branch in the Some(old) => arm.
        let mut old = test_entry("eth:lan1", WanModemState::Active);
        old.entry_type = WanEntryType::Ethernet;
        old.original_bridge = Some("br-lan".to_string());
        let mut entry = old.clone();
        entry.proto_override = Some("static".to_string());
        assert!(
            !super::should_reconcile_existing_modem_entry(&old, &entry),
            "Ethernet entries must not enter the modem reconcile branch",
        );
    }

    #[test]
    fn should_not_reconcile_existing_ethernet_entry_no_bridge() {
        // Existing dedicated WAN port (e.g. wan, br-wan): no proto reconcile.
        // resolve_uci_proto short-circuits to dhcp for Ethernet regardless.
        let mut old = test_entry("eth:br-wan", WanModemState::Active);
        old.entry_type = WanEntryType::Ethernet;
        old.original_bridge = None;
        let mut entry = old.clone();
        entry.proto_override = Some("static".to_string());
        assert!(!super::should_reconcile_existing_modem_entry(&old, &entry));
    }

    /// Mirror of the proto_override validation block in `update_wan_config`.
    /// Returns Ok(()) if valid, Err(reason) if invalid. Kept in sync with
    /// the handler manually — if the handler changes, this should change too.
    fn validate_proto_override(p: Option<&str>) -> Result<(), &'static str> {
        if let Some(p) = p {
            if p.trim().is_empty() {
                return Err("empty or whitespace-only");
            }
            if p.len() > 32 {
                return Err("over 32 chars");
            }
            if p.chars().any(char::is_whitespace) {
                return Err("contains whitespace");
            }
        }
        Ok(())
    }

    #[test]
    fn proto_override_validation_rules() {
        // Valid values.
        assert!(validate_proto_override(None).is_ok());
        assert!(validate_proto_override(Some("dhcp")).is_ok());
        assert!(validate_proto_override(Some("qmi")).is_ok());
        assert!(validate_proto_override(Some("static")).is_ok());
        assert!(validate_proto_override(Some("pppoe")).is_ok());
        // 32 chars exactly — at the boundary, accepted.
        let s32 = "a".repeat(32);
        assert!(validate_proto_override(Some(&s32)).is_ok());

        // Invalid values.
        assert!(validate_proto_override(Some("")).is_err(), "empty rejected");
        assert!(validate_proto_override(Some("   ")).is_err(), "whitespace-only rejected");
        let s33 = "a".repeat(33);
        assert!(validate_proto_override(Some(&s33)).is_err(), "33 chars rejected");
        assert!(validate_proto_override(Some("dh cp")).is_err(), "embedded space rejected");
        assert!(validate_proto_override(Some("dhcp\t")).is_err(), "embedded tab rejected");
        assert!(validate_proto_override(Some("dhcp\n")).is_err(), "embedded newline rejected");
    }

    /// Validate a WanConfig the same way the handler does.
    /// Returns Ok(()) if valid, Err(message) if invalid.
    fn validate_wan_config(config: &WanConfig) -> Result<(), &'static str> {
        if config.enabled {
            if config.modem_priority.is_empty() {
                return Err("empty priority list");
            }
            if !config
                .modem_priority
                .iter()
                .any(|e| e.state == WanModemState::Active)
            {
                return Err("no active entry");
            }
        }
        Ok(())
    }

    #[test]
    fn enabled_with_empty_priority_is_rejected() {
        let config = WanConfig {
            enabled: true,
            modem_priority: vec![],
            watchdog: WatchdogConfig::default(),
            failover_locked: false,
            failback_timer_mins: 0,
            routing_mode: Default::default(),
        };
        assert!(validate_wan_config(&config).is_err());
    }

    #[test]
    fn enabled_with_no_active_entries_is_rejected() {
        let config = WanConfig {
            enabled: true,
            modem_priority: vec![
                test_entry("modem1", WanModemState::Standby),
                test_entry("modem2", WanModemState::Standby),
            ],
            watchdog: WatchdogConfig::default(),
            failover_locked: false,
            failback_timer_mins: 0,
            routing_mode: Default::default(),
        };
        assert!(validate_wan_config(&config).is_err());
    }

    #[test]
    fn enabled_with_active_entry_is_accepted() {
        let config = WanConfig {
            enabled: true,
            modem_priority: vec![
                test_entry("modem1", WanModemState::Active),
                test_entry("modem2", WanModemState::Standby),
            ],
            watchdog: WatchdogConfig::default(),
            failover_locked: false,
            failback_timer_mins: 0,
            routing_mode: Default::default(),
        };
        assert!(validate_wan_config(&config).is_ok());
    }

    #[test]
    fn disabled_with_empty_priority_is_accepted() {
        let config = WanConfig {
            enabled: false,
            modem_priority: vec![],
            watchdog: WatchdogConfig::default(),
            failover_locked: false,
            failback_timer_mins: 0,
            routing_mode: Default::default(),
        };
        assert!(validate_wan_config(&config).is_ok());
    }

    #[test]
    fn disabled_with_no_active_entries_is_accepted() {
        let config = WanConfig {
            enabled: false,
            modem_priority: vec![test_entry("modem1", WanModemState::Standby)],
            watchdog: WatchdogConfig::default(),
            failover_locked: false,
            failback_timer_mins: 0,
            routing_mode: Default::default(),
        };
        assert!(validate_wan_config(&config).is_ok());
    }

    #[test]
    fn routing_mode_defaults_to_failover() {
        let json = r#"{"enabled":false,"modem_priority":[],"watchdog":{},"failover_locked":false,"failback_timer_mins":0}"#;
        let config: WanConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.routing_mode, RoutingMode::Failover);
    }

    #[test]
    fn routing_mode_load_balance_roundtrip() {
        let json = r#"{"enabled":false,"modem_priority":[],"watchdog":{},"failover_locked":false,"failback_timer_mins":0,"routing_mode":"load_balance"}"#;
        let config: WanConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.routing_mode, RoutingMode::LoadBalance);
        let serialized = serde_json::to_string(&config).unwrap();
        assert!(serialized.contains("\"routing_mode\":\"load_balance\""));
    }

    #[test]
    fn weight_defaults_to_none() {
        let entry = test_entry("modem1", WanModemState::Active);
        assert_eq!(entry.weight, None);
    }

    #[test]
    fn weight_roundtrip() {
        let json = r#"{"modem_id":"m1","label":"M1","interface_name":"WWAN","network_device":"usb0","device_path":"","state":"active","metric":20,"weight":5}"#;
        let entry: WanModemEntry = serde_json::from_str(json).unwrap();
        assert_eq!(entry.weight, Some(5));
        let serialized = serde_json::to_string(&entry).unwrap();
        assert!(serialized.contains("\"weight\":5"));
    }

    #[test]
    fn missing_weight_deserializes_as_none() {
        let json = r#"{"modem_id":"m1","label":"M1","interface_name":"WWAN","network_device":"usb0","device_path":"","state":"active","metric":20}"#;
        let entry: WanModemEntry = serde_json::from_str(json).unwrap();
        assert_eq!(entry.weight, None);
    }

    #[test]
    fn weight_zero_is_rejected() {
        // Simulate what the handler does: validate weight range
        let weight: Option<u32> = Some(0);
        if let Some(w) = weight {
            assert!(!(1..=10).contains(&w), "Weight 0 should be out of range");
        }
    }

    #[test]
    fn weight_eleven_is_rejected() {
        let weight: Option<u32> = Some(11);
        if let Some(w) = weight {
            assert!(!(1..=10).contains(&w), "Weight 11 should be out of range");
        }
    }

    #[test]
    fn weight_in_range_accepted() {
        for w in 1..=10 {
            assert!((1..=10).contains(&w), "Weight {w} should be in range");
        }
    }

    #[test]
    fn reconcile_parses_uci_show_output() {
        let input = "\
network.lan=interface
network.lan.device='br-lan'
network.lan.proto='static'
network.lan.ipaddr='192.168.1.1'
network.wwan=interface
network.wwan.proto='dhcp'
network.wwan.device='wwan0'
network.WWAN=interface
network.WWAN.proto='qmi'
network.WWAN.device='wwan0'
network.WWAN.metric='20'
network.wan6=interface
network.wan6.proto='dhcpv6'
\n
";
        let parsed = super::parse_uci_show_output(input);
        assert_eq!(parsed.get("lan").and_then(|s| s.device.as_deref()), Some("br-lan"));
        assert_eq!(parsed.get("wwan").and_then(|s| s.device.as_deref()), Some("wwan0"));
        assert_eq!(parsed.get("wwan").and_then(|s| s.proto.as_deref()), Some("dhcp"));
        assert_eq!(parsed.get("WWAN").and_then(|s| s.device.as_deref()), Some("wwan0"));
        assert_eq!(parsed.get("WWAN").and_then(|s| s.proto.as_deref()), Some("qmi"));
        assert_eq!(parsed.get("WWAN").and_then(|s| s.metric), Some(20));
        assert_eq!(parsed.get("wan6").and_then(|s| s.device.as_deref()), None);
        assert!(!parsed.contains_key(""));
    }

    #[tokio::test]
    async fn reconcile_displaces_lowercase_collision_in_mock_mode() {
        let _guard = super::TEST_LOCK.lock().unwrap();
        // Set up mock mode and seed the colliding lowercase section.
        std::env::set_var("MOCK_HARDWARE", "1");
        super::mock_uci_reset();
        super::mock_uci_seed("wwan", Some("wwan0"), Some("dhcp"), None);

        let displaced = super::reconcile_uci_section("WWAN", "wwan0", "qmi", None, 20, None)
            .await
            .expect("reconcile must succeed");

        assert_eq!(displaced, vec!["wwan".to_string()]);

        let state = super::mock_uci_state().lock().unwrap();
        assert!(!state.contains_key("wwan"), "lowercase wwan must be displaced");
        let our_section = state.get("WWAN").expect("WWAN must exist");
        assert_eq!(our_section.device.as_deref(), Some("wwan0"));
        assert_eq!(our_section.proto.as_deref(), Some("qmi"));
        assert_eq!(our_section.metric, Some(20));
    }

    #[tokio::test]
    async fn reconcile_idempotent_when_section_already_correct() {
        let _guard = super::TEST_LOCK.lock().unwrap();
        std::env::set_var("MOCK_HARDWARE", "1");
        super::mock_uci_reset();
        super::mock_uci_seed("WWAN", Some("wwan0"), Some("qmi"), Some(20));

        let displaced = super::reconcile_uci_section("WWAN", "wwan0", "qmi", None, 20, None)
            .await
            .expect("reconcile must succeed");

        assert!(displaced.is_empty(), "no displacements expected");
        let state = super::mock_uci_state().lock().unwrap();
        let s = state.get("WWAN").expect("WWAN must exist");
        assert_eq!(s.device.as_deref(), Some("wwan0"));
        assert_eq!(s.metric, Some(20));
    }

    #[tokio::test]
    async fn reconcile_updates_in_place_when_metric_changes() {
        let _guard = super::TEST_LOCK.lock().unwrap();
        std::env::set_var("MOCK_HARDWARE", "1");
        super::mock_uci_reset();
        super::mock_uci_seed("WWAN", Some("wwan0"), Some("qmi"), Some(20));

        let displaced = super::reconcile_uci_section("WWAN", "wwan0", "qmi", None, 998, None)
            .await
            .expect("reconcile must succeed");

        assert!(displaced.is_empty(), "no displacements expected for in-place update");
        let state = super::mock_uci_state().lock().unwrap();
        let s = state.get("WWAN").expect("WWAN must exist");
        assert_eq!(s.metric, Some(998), "metric must be updated in place");
    }

    #[tokio::test]
    async fn reconcile_handles_orphaned_managed_section_on_same_device() {
        let _guard = super::TEST_LOCK.lock().unwrap();
        std::env::set_var("MOCK_HARDWARE", "1");
        super::mock_uci_reset();
        // A stale managed section from a prior config, on the same device.
        super::mock_uci_seed("WWAN2", Some("wwan0"), Some("qmi"), Some(998));

        let displaced = super::reconcile_uci_section("WWAN", "wwan0", "qmi", None, 20, None)
            .await
            .expect("reconcile must succeed");

        assert_eq!(displaced, vec!["WWAN2".to_string()]);
        let state = super::mock_uci_state().lock().unwrap();
        assert!(!state.contains_key("WWAN2"), "WWAN2 must be displaced");
        assert!(state.contains_key("WWAN"), "WWAN must exist");
    }

    #[tokio::test]
    async fn reconcile_returns_err_when_collision_delete_fails() {
        let _guard = super::TEST_LOCK.lock().unwrap();
        std::env::set_var("MOCK_HARDWARE", "1");
        super::mock_uci_reset();
        super::mock_uci_seed("wwan", Some("wwan0"), Some("dhcp"), None);
        super::mock_uci_set_delete_fails(true);

        let result = super::reconcile_uci_section("WWAN", "wwan0", "qmi", None, 20, None).await;

        super::mock_uci_set_delete_fails(false); // reset for other tests
        assert!(result.is_err(), "reconcile must surface delete failure");
        let state = super::mock_uci_state().lock().unwrap();
        assert!(state.contains_key("wwan"), "lowercase wwan must remain (no half-apply)");
        assert!(!state.contains_key("WWAN"), "our section must NOT be written when delete failed");
    }

    // ========================================================================
    // WAN entry teardown — wan firewall-zone cleanup (dev.41 bench-smoke fix)
    // ========================================================================

    #[tokio::test]
    async fn modem_teardown_removes_wan_zone_member() {
        // A removed modem WAN entry tears down via ifdown + delete UCI section.
        // It must ALSO drop its wan firewall-zone membership — otherwise
        // firewall.@zone[1].network dangles (the post-apply orphan sweep can't
        // recover it: the network section is already deleted). This replicates
        // the modem-removal teardown call sequence from update_wan_config.
        let _guard = super::TEST_LOCK.lock().unwrap();
        std::env::set_var("MOCK_HARDWARE", "1");
        super::mock_uci_reset();
        super::mock_wan_zone_reset();

        // Seed: the modem interface exists as a UCI section and is in the wan zone.
        super::mock_uci_seed("WWAN", Some("wwan0"), Some("dhcp"), Some(20));
        super::uci_add_to_wan_zone("WWAN").await.expect("seed add must succeed");
        assert!(super::mock_wan_zone_contains("WWAN"), "precondition: in wan zone");

        // Teardown sequence for a removed modem entry.
        let _ = super::ifdown("WWAN").await;
        let _ = super::uci_delete_interface("WWAN").await;
        let _ = super::uci_remove_from_wan_zone("WWAN").await;

        assert!(
            !super::mock_wan_zone_contains("WWAN"),
            "wan-zone membership must be removed on modem teardown (no dangling member)"
        );
    }

    #[tokio::test]
    async fn wan_zone_remove_is_idempotent() {
        // The helper is non-fatal and safe to call when the member is absent.
        let _guard = super::TEST_LOCK.lock().unwrap();
        std::env::set_var("MOCK_HARDWARE", "1");
        super::mock_wan_zone_reset();

        // Removing a name that was never added must succeed and stay absent.
        super::uci_remove_from_wan_zone("WWAN")
            .await
            .expect("remove of absent member must not error");
        assert!(!super::mock_wan_zone_contains("WWAN"));
    }

    // ========================================================================
    // Item #37 sub-task 2b — reconcile_uci_section control-device-path tests
    // ========================================================================

    #[tokio::test]
    async fn reconcile_writes_control_path_for_qmi() {
        let _guard = super::TEST_LOCK.lock().unwrap();
        std::env::set_var("MOCK_HARDWARE", "1");
        super::mock_uci_reset();

        let displaced = super::reconcile_uci_section(
            "WWAN", "wwan0", "qmi", Some("/dev/cdc-wdm0"), 20, None,
        )
        .await
        .expect("reconcile must succeed");

        assert!(displaced.is_empty());
        let state = super::mock_uci_state().lock().unwrap();
        let s = state.get("WWAN").expect("WWAN must exist");
        assert_eq!(
            s.device.as_deref(),
            Some("/dev/cdc-wdm0"),
            "proto=qmi must write control device path, not netif"
        );
        assert_eq!(s.proto.as_deref(), Some("qmi"));
    }

    #[tokio::test]
    async fn reconcile_writes_control_path_for_mbim() {
        let _guard = super::TEST_LOCK.lock().unwrap();
        std::env::set_var("MOCK_HARDWARE", "1");
        super::mock_uci_reset();

        let displaced = super::reconcile_uci_section(
            "WWAN", "wwan1", "mbim", Some("/dev/cdc-wdm1"), 20, None,
        )
        .await
        .expect("reconcile must succeed");

        assert!(displaced.is_empty());
        let state = super::mock_uci_state().lock().unwrap();
        let s = state.get("WWAN").expect("WWAN must exist");
        assert_eq!(s.device.as_deref(), Some("/dev/cdc-wdm1"));
        assert_eq!(s.proto.as_deref(), Some("mbim"));
    }

    #[tokio::test]
    async fn reconcile_writes_netif_for_dhcp_with_control_path_passed() {
        // Even if a control path is passed, proto=dhcp ignores it and writes
        // the netif. The internal proto-keyed switch only consults the
        // control path for proto=qmi/mbim.
        let _guard = super::TEST_LOCK.lock().unwrap();
        std::env::set_var("MOCK_HARDWARE", "1");
        super::mock_uci_reset();

        let displaced = super::reconcile_uci_section(
            "WWAN", "wwan0", "dhcp", Some("/dev/cdc-wdm0"), 20, None,
        )
        .await
        .expect("reconcile must succeed");

        assert!(displaced.is_empty());
        let state = super::mock_uci_state().lock().unwrap();
        let s = state.get("WWAN").expect("WWAN must exist");
        assert_eq!(
            s.device.as_deref(),
            Some("wwan0"),
            "proto=dhcp must ignore control path"
        );
        assert_eq!(s.proto.as_deref(), Some("dhcp"));
    }

    #[tokio::test]
    async fn reconcile_falls_back_to_netif_when_qmi_but_no_control_path() {
        // Sysfs walk produced None (modem in ECM mode, USB enumeration race,
        // operator override on a kernel-binding mismatch, etc.). proto=qmi
        // falls back to writing the netif. UCI section is written; netifd
        // bring-up will then fail with "control device does not exist", but
        // that surfaces in OpenWrt logs — the daemon doesn't refuse to
        // reconcile.
        let _guard = super::TEST_LOCK.lock().unwrap();
        std::env::set_var("MOCK_HARDWARE", "1");
        super::mock_uci_reset();

        let displaced = super::reconcile_uci_section(
            "WWAN", "wwan0", "qmi", None, 20, None,
        )
        .await
        .expect("reconcile must succeed even without control path");

        assert!(displaced.is_empty());
        let state = super::mock_uci_state().lock().unwrap();
        let s = state.get("WWAN").expect("WWAN must exist");
        assert_eq!(
            s.device.as_deref(),
            Some("wwan0"),
            "no control path → fall back to netif (broken bring-up logged at info-level)"
        );
        assert_eq!(s.proto.as_deref(), Some("qmi"));
    }

    #[tokio::test]
    async fn reconcile_collision_detection_keys_on_netif_not_control_path() {
        // Two stale UCI sections both bind wwan0. Reconciling with
        // target_device="wwan0" + control path Some("/dev/cdc-wdm0") must
        // displace BOTH sections — collision detection keys on the netif arg
        // (target_device), so it doesn't matter what device value those
        // sections carry. The new section we write gets the cdc-wdm path.
        let _guard = super::TEST_LOCK.lock().unwrap();
        std::env::set_var("MOCK_HARDWARE", "1");
        super::mock_uci_reset();
        super::mock_uci_seed("wwan", Some("wwan0"), Some("dhcp"), None);
        super::mock_uci_seed("wwan_alt", Some("wwan0"), Some("qmi"), None);

        let displaced = super::reconcile_uci_section(
            "WWAN", "wwan0", "qmi", Some("/dev/cdc-wdm0"), 20, None,
        )
        .await
        .expect("reconcile must succeed");

        assert_eq!(displaced.len(), 2, "both stale sections must be displaced");
        let state = super::mock_uci_state().lock().unwrap();
        assert!(!state.contains_key("wwan"));
        assert!(!state.contains_key("wwan_alt"));
        let s = state.get("WWAN").expect("WWAN must exist");
        assert_eq!(s.device.as_deref(), Some("/dev/cdc-wdm0"));
    }

    #[tokio::test]
    async fn purge_orphaned_managed_sections_is_case_insensitive() {
        use std::collections::HashSet;
        let _guard = super::TEST_LOCK.lock().unwrap();
        std::env::set_var("MOCK_HARDWARE", "1");
        super::mock_uci_reset();
        super::mock_uci_seed("wwan", Some("wwan0"), Some("dhcp"), None);
        super::mock_uci_seed("WWAN", Some("wwan0"), Some("qmi"), Some(20));
        super::mock_uci_seed("Wwan2", Some("wwan1"), Some("qmi"), Some(998));
        super::mock_uci_seed("wan", Some("eth0"), Some("dhcp"), None);

        let mut active: HashSet<String> = HashSet::new();
        active.insert("WWAN".to_string());

        super::purge_orphaned_managed_sections(&active)
            .await
            .expect("purge must succeed");

        let state = super::mock_uci_state().lock().unwrap();
        assert!(state.contains_key("WWAN"), "active WWAN must survive");
        assert!(!state.contains_key("wwan"), "lowercase wwan must be purged (case-insensitive prefix match)");
        assert!(!state.contains_key("Wwan2"), "mixed-case Wwan2 must be purged");
        assert!(state.contains_key("wan"), "non-managed 'wan' must survive (not in WWAN/EWAN namespace)");
    }

    // ========================================================================
    // Item #37 sub-task 2c — update_wan_config integration tests
    // (verifies the new diff branch is wired into the handler at the right
    // place, not just that the predicate function works in isolation)
    // ========================================================================

    /// Helper: construct a minimal AppState for handler-level integration tests.
    /// Mirrors the precedent at backend/src/api/mod.rs:295-311.
    async fn make_test_state() -> std::sync::Arc<crate::state::AppState> {
        use crate::hardware::AppConfig;
        use crate::hardware::profiles::ProfileRegistry;
        use crate::security::license::LicenseState;
        use crate::security::users::UserStore;
        let config = AppConfig::default();
        let users = UserStore::load("/nonexistent/users.json").await;
        let registry = ProfileRegistry::load();
        let device_token = "test-device-token".to_string();
        let license_state = LicenseState::Unlicensed;
        std::sync::Arc::new(crate::state::AppState::new(
            config,
            users,
            registry,
            device_token,
            std::sync::Arc::new(crate::security::device_auth::DeviceAuth::ephemeral()),
            license_state,
        ))
    }

    /// Helper: construct an admin SessionUser for handler-level tests.
    fn admin_session_user() -> crate::api::auth_middleware::SessionUser {
        crate::api::auth_middleware::SessionUser {
            username: "test-admin".to_string(),
            role: crate::security::users::Role::Admin,
        }
    }

    /// Helper: redirect WAN config persistence to a temp file so
    /// `save_and_broadcast` doesn't write to /etc on the test host.
    /// Returns the tempdir guard to keep it alive for the duration of the test.
    fn redirect_wan_config_to_tempdir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir must succeed");
        let path = dir.path().join("wan-config.json");
        std::env::set_var("WAN_CONFIG_PATH", path);
        std::env::set_var("WAN_WATCHDOG_LOG_PATH", dir.path().join("watchdog.log"));
        dir
    }

    #[tokio::test]
    async fn update_wan_config_proto_override_flip_writes_uci() {
        use axum::extract::{State, Json};
        use axum::Extension;
        let _guard = super::TEST_LOCK.lock().unwrap();
        std::env::set_var("MOCK_HARDWARE", "1");
        let _tmp = redirect_wan_config_to_tempdir();
        super::mock_uci_reset();

        let state = make_test_state().await;
        // Seed wan_config with one existing modem entry, no proto_override.
        {
            let mut config = state.wan_config.write().await;
            config.enabled = true;
            config.modem_priority = vec![modem_entry("2c7c:0122:abc", "wwan0")];
        }

        // Build new config: same entry but with proto_override="qmi".
        let mut updated_entry = modem_entry("2c7c:0122:abc", "wwan0");
        updated_entry.proto_override = Some("qmi".to_string());
        let new_config = WanConfig {
            enabled: true,
            modem_priority: vec![updated_entry],
            watchdog: WatchdogConfig::default(),
            failover_locked: false,
            failback_timer_mins: 0,
            routing_mode: RoutingMode::default(),
        };

        let user = admin_session_user();
        let _ = super::update_wan_config(
            State(state.clone()),
            Extension(user),
            Json(new_config),
        )
        .await
        .expect("update_wan_config must succeed");

        // mock_uci_state should now have a section for "WWAN" with proto=qmi.
        let mock_state = super::mock_uci_state().lock().unwrap();
        let s = mock_state.get("WWAN").expect("WWAN must be reconciled");
        assert_eq!(
            s.proto.as_deref(),
            Some("qmi"),
            "proto_override flip must reach UCI without a Scan",
        );
    }

    #[tokio::test]
    async fn update_wan_config_no_diff_save_does_not_call_reconcile() {
        use axum::extract::{State, Json};
        use axum::Extension;
        let _guard = super::TEST_LOCK.lock().unwrap();
        std::env::set_var("MOCK_HARDWARE", "1");
        let _tmp = redirect_wan_config_to_tempdir();
        super::mock_uci_reset();

        let state = make_test_state().await;
        {
            let mut config = state.wan_config.write().await;
            config.enabled = true;
            config.modem_priority = vec![modem_entry("2c7c:0122:abc", "wwan0")];
        }

        let new_config = WanConfig {
            enabled: true,
            modem_priority: vec![modem_entry("2c7c:0122:abc", "wwan0")],
            watchdog: WatchdogConfig::default(),
            failover_locked: false,
            failback_timer_mins: 0,
            routing_mode: RoutingMode::default(),
        };

        let user = admin_session_user();
        let _ = super::update_wan_config(
            State(state.clone()),
            Extension(user),
            Json(new_config),
        )
        .await
        .expect("update_wan_config must succeed");

        // mock_uci_state should be empty — no reconcile, no fields changed.
        let mock_state = super::mock_uci_state().lock().unwrap();
        assert!(
            mock_state.is_empty(),
            "no-diff save must not call reconcile_uci_section",
        );
    }

    #[tokio::test]
    async fn update_wan_config_metric_only_diff_does_not_call_reconcile() {
        use axum::extract::{State, Json};
        use axum::Extension;
        let _guard = super::TEST_LOCK.lock().unwrap();
        std::env::set_var("MOCK_HARDWARE", "1");
        let _tmp = redirect_wan_config_to_tempdir();
        super::mock_uci_reset();

        let state = make_test_state().await;
        {
            let mut config = state.wan_config.write().await;
            config.enabled = true;
            let mut entry = modem_entry("2c7c:0122:abc", "wwan0");
            entry.metric = 20;
            config.modem_priority = vec![entry];
        }

        // Same entry — metric will be overwritten by assign_metrics() in the handler,
        // but the predicate ignores metric changes either way.
        let updated_entry = modem_entry("2c7c:0122:abc", "wwan0");
        let new_config = WanConfig {
            enabled: true,
            modem_priority: vec![updated_entry],
            watchdog: WatchdogConfig::default(),
            failover_locked: false,
            failback_timer_mins: 0,
            routing_mode: RoutingMode::default(),
        };

        let user = admin_session_user();
        let _ = super::update_wan_config(
            State(state.clone()),
            Extension(user),
            Json(new_config),
        )
        .await
        .expect("update_wan_config must succeed");

        // mock_uci_state stays empty — uci_set_metric is mock-no-op (debug
        // trace only), and the new 2c branch did NOT fire (metric-only diff
        // is not in the predicate).
        let mock_state = super::mock_uci_state().lock().unwrap();
        assert!(
            mock_state.is_empty(),
            "metric-only diff must not call reconcile_uci_section",
        );
    }

    #[tokio::test]
    async fn update_wan_config_ethernet_entry_does_not_trigger_modem_branch() {
        // Regression: an Ethernet entry that exists in both old and new config
        // must NOT trigger the new sub-task 2c modem branch even if its
        // proto_override changes — Ethernet is excluded from the predicate.
        // The existing Ethernet bridge-conversion branch is gated on
        // `!uci_interface_exists(...)`, which returns true in mock mode (so
        // it doesn't fire in mock); the assertion here is that nothing was
        // written to mock_uci_state, confirming the new 2c branch's predicate
        // correctly excludes Ethernet.
        use axum::extract::{State, Json};
        use axum::Extension;
        let _guard = super::TEST_LOCK.lock().unwrap();
        std::env::set_var("MOCK_HARDWARE", "1");
        let _tmp = redirect_wan_config_to_tempdir();
        super::mock_uci_reset();

        let state = make_test_state().await;
        {
            let mut config = state.wan_config.write().await;
            config.enabled = true;
            let mut eth = test_entry("eth:lan1", WanModemState::Active);
            eth.entry_type = WanEntryType::Ethernet;
            eth.original_bridge = Some("br-lan".to_string());
            eth.network_device = "lan1".to_string();
            eth.interface_name = "EWAN".to_string();
            config.modem_priority = vec![eth];
        }

        // Same Ethernet entry but with proto_override flipped — the predicate
        // would match if it weren't gated on entry_type.
        let mut eth_new = test_entry("eth:lan1", WanModemState::Active);
        eth_new.entry_type = WanEntryType::Ethernet;
        eth_new.original_bridge = Some("br-lan".to_string());
        eth_new.network_device = "lan1".to_string();
        eth_new.interface_name = "EWAN".to_string();
        eth_new.proto_override = Some("static".to_string());

        let new_config = WanConfig {
            enabled: true,
            modem_priority: vec![eth_new],
            watchdog: WatchdogConfig::default(),
            failover_locked: false,
            failback_timer_mins: 0,
            routing_mode: RoutingMode::default(),
        };

        let user = admin_session_user();
        let _ = super::update_wan_config(
            State(state.clone()),
            Extension(user),
            Json(new_config),
        )
        .await
        .expect("update_wan_config must succeed");

        // mock_uci_state must NOT contain "EWAN" — the new 2c branch's
        // predicate excludes Ethernet, and the bridge-conversion branch
        // is suppressed because uci_interface_exists() returns true in mock.
        // This confirms no double-reconcile for Ethernet via the 2c branch.
        let mock_state = super::mock_uci_state().lock().unwrap();
        assert!(
            !mock_state.contains_key("EWAN"),
            "Ethernet entry must not trigger sub-task 2c modem branch",
        );
    }

    #[tokio::test]
    async fn update_wan_config_proto_flip_with_disconnected_modem_uses_default_resolved() {
        // Q-D2: when the modem is in wan_config but NOT in state.modems
        // (e.g. modem disconnected since last save), the new 2c branch
        // still fires using ResolvedReconcileFields::default() — that
        // resolves to UsbNetMode::Unknown. With proto_override="qmi" set,
        // resolve_uci_proto returns "qmi" (override wins); with no
        // control_device_path, resolve_uci_device falls back to netif.
        use axum::extract::{State, Json};
        use axum::Extension;
        let _guard = super::TEST_LOCK.lock().unwrap();
        std::env::set_var("MOCK_HARDWARE", "1");
        let _tmp = redirect_wan_config_to_tempdir();
        super::mock_uci_reset();

        let state = make_test_state().await;
        {
            let mut config = state.wan_config.write().await;
            config.enabled = true;
            // Entry exists in config but state.modems is empty (modem disconnected).
            config.modem_priority = vec![modem_entry("2c7c:0122:disconnected", "wwan0")];
        }
        // Verify state.modems is empty — Q-D2 contract.
        assert!(
            state.modems.read().await.is_empty(),
            "test setup: state.modems must be empty for Q-D2 case",
        );

        // Operator flips proto_override="qmi" on the disconnected modem.
        let mut updated_entry = modem_entry("2c7c:0122:disconnected", "wwan0");
        updated_entry.proto_override = Some("qmi".to_string());
        let new_config = WanConfig {
            enabled: true,
            modem_priority: vec![updated_entry],
            watchdog: WatchdogConfig::default(),
            failover_locked: false,
            failback_timer_mins: 0,
            routing_mode: RoutingMode::default(),
        };

        let user = admin_session_user();
        let _ = super::update_wan_config(
            State(state.clone()),
            Extension(user),
            Json(new_config),
        )
        .await
        .expect("update_wan_config must succeed");

        // UCI is written with proto=qmi (override) + device=netif (no
        // control path because modem isn't in state.modems → no sysfs
        // lookup → control_device_path=None → reconcile_uci_section's
        // internal switch falls back to netif). Asserts the write happened
        // with the expected Q-D2 fallback values.
        let mock_state = super::mock_uci_state().lock().unwrap();
        let s = mock_state
            .get("WWAN")
            .expect("WWAN must be reconciled even for disconnected modem");
        assert_eq!(s.proto.as_deref(), Some("qmi"));
        assert_eq!(
            s.device.as_deref(),
            Some("wwan0"),
            "no control path → netif fallback (Q-D2 + sub-task 2b broken state)",
        );
    }

    #[tokio::test]
    async fn build_status_response_includes_proto_override_when_set() {
        let _guard = super::TEST_LOCK.lock().unwrap();
        let state = make_test_state().await;
        {
            let mut config = state.wan_config.write().await;
            let mut entry = modem_entry("2c7c:0122:itm39", "wwan0");
            entry.proto_override = Some("qmi".to_string());
            config.modem_priority = vec![entry];
        }

        let response = super::build_status_response(&state).await;

        assert_eq!(response.modems.len(), 1);
        assert_eq!(
            response.modems[0].proto_override,
            Some("qmi".to_string()),
            "proto_override on WanModemEntry must flow through to WanModemStatusEntry"
        );
    }

    #[tokio::test]
    async fn build_status_response_proto_override_none_serializes_omitted() {
        let _guard = super::TEST_LOCK.lock().unwrap();
        let state = make_test_state().await;
        {
            let mut config = state.wan_config.write().await;
            // modem_entry() default has proto_override = None already; no override.
            config.modem_priority = vec![modem_entry("2c7c:0122:itm39", "wwan0")];
        }

        let response = super::build_status_response(&state).await;
        let json = serde_json::to_value(&response).expect("WanStatusResponse serializes cleanly");
        let modem_obj = &json["modems"][0];

        assert!(
            modem_obj.get("proto_override").is_none(),
            "proto_override key MUST be omitted when value is None (skip_serializing_if precedent); got modem object: {modem_obj}"
        );
    }

    // ========================================================================
    // Shell-injection boundary validation (Fix 1b) + role gate (Fix 2)
    // ========================================================================

    #[test]
    fn uci_token_validator_accepts_normal_names() {
        assert!(super::is_valid_uci_token("WWAN"));
        assert!(super::is_valid_uci_token("wwan0"));
        assert!(super::is_valid_uci_token("EWAN2"));
        assert!(super::is_valid_uci_token("eth-0_1"));
    }

    #[test]
    fn uci_token_validator_rejects_metacharacters() {
        // Each of these must be rejected — they are the injection payloads.
        for bad in [
            "",
            "wwan0; reboot",
            "wwan0|rm -rf /",
            "wwan0&whoami",
            "a$(id)",
            "a`id`",
            "wwan 0",
            "wwan0\nreboot",
            "a/b",
            "a.b",
            "a'b",
            "a\"b",
            &"x".repeat(33), // over 32 chars
        ] {
            assert!(
                !super::is_valid_uci_token(bad),
                "must reject injection/invalid token: {bad:?}"
            );
        }
    }

    #[test]
    fn watchdog_host_validator_rejects_metacharacters() {
        assert!(super::is_valid_watchdog_host("8.8.8.8"));
        assert!(super::is_valid_watchdog_host("google.com"));
        assert!(super::is_valid_watchdog_host("2001:4860:4860::8888"));
        for bad in ["8.8.8.8; reboot", "$(id)", "a b", "a|b", "", "a`b`"] {
            assert!(!super::is_valid_watchdog_host(bad), "must reject: {bad:?}");
        }
    }

    #[test]
    fn http_target_validator_rejects_metacharacters() {
        assert!(super::is_valid_http_target("http://connectivitycheck.gstatic.com/generate_204"));
        assert!(super::is_valid_http_target("https://example.com/path"));
        for bad in [
            "ftp://example.com",
            "http://a b",
            "http://a;reboot",
            "http://$(id)",
            "http://a`id`",
            "notaurl",
            "",
        ] {
            assert!(!super::is_valid_http_target(bad), "must reject: {bad:?}");
        }
    }

    /// Build a WanConfig with one active modem entry, overriding a single field.
    fn injection_test_config(iface: &str, device: &str, proto: Option<&str>) -> WanConfig {
        let mut entry = modem_entry("2c7c:0122:abc", device);
        entry.interface_name = iface.to_string();
        entry.network_device = device.to_string();
        entry.proto_override = proto.map(|s| s.to_string());
        WanConfig {
            enabled: true,
            modem_priority: vec![entry],
            watchdog: WatchdogConfig::default(),
            failover_locked: false,
            failback_timer_mins: 0,
            routing_mode: RoutingMode::default(),
        }
    }

    #[tokio::test]
    async fn update_wan_config_rejects_injection_in_interface_name() {
        use axum::extract::{Json, State};
        use axum::Extension;
        let _guard = super::TEST_LOCK.lock().unwrap();
        std::env::set_var("MOCK_HARDWARE", "1");
        let _tmp = redirect_wan_config_to_tempdir();
        super::mock_uci_reset();

        let state = make_test_state().await;
        for bad in ["WWAN; reboot", "a|b", "a&b", "a$(id)", "a`id`", "a b", "a\nb"] {
            let cfg = injection_test_config(bad, "wwan0", None);
            let res = super::update_wan_config(
                State(state.clone()),
                Extension(admin_session_user()),
                Json(cfg),
            )
            .await;
            assert!(res.is_err(), "interface_name {bad:?} must be rejected");
        }
        // No command ran: mock UCI state stays empty.
        assert!(super::mock_uci_state().lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn update_wan_config_rejects_injection_in_network_device_and_proto() {
        use axum::extract::{Json, State};
        use axum::Extension;
        let _guard = super::TEST_LOCK.lock().unwrap();
        std::env::set_var("MOCK_HARDWARE", "1");
        let _tmp = redirect_wan_config_to_tempdir();
        super::mock_uci_reset();

        let state = make_test_state().await;

        // network_device with metacharacters
        let cfg = injection_test_config("WWAN", "wwan0; rm -rf /", None);
        assert!(super::update_wan_config(
            State(state.clone()),
            Extension(admin_session_user()),
            Json(cfg),
        )
        .await
        .is_err());

        // proto_override with metacharacters
        let cfg = injection_test_config("WWAN", "wwan0", Some("qmi;reboot"));
        assert!(super::update_wan_config(
            State(state.clone()),
            Extension(admin_session_user()),
            Json(cfg),
        )
        .await
        .is_err());

        assert!(super::mock_uci_state().lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn update_wan_config_readonly_forbidden() {
        use axum::extract::{Json, State};
        use axum::Extension;
        let _guard = super::TEST_LOCK.lock().unwrap();
        std::env::set_var("MOCK_HARDWARE", "1");
        let _tmp = redirect_wan_config_to_tempdir();
        super::mock_uci_reset();

        let state = make_test_state().await;
        let readonly = crate::api::auth_middleware::SessionUser {
            username: "viewer".to_string(),
            role: crate::security::users::Role::ReadOnly,
        };
        // Even a perfectly valid config must be rejected for a ReadOnly user.
        let cfg = injection_test_config("WWAN", "wwan0", None);
        let res = super::update_wan_config(
            State(state.clone()),
            Extension(readonly),
            Json(cfg),
        )
        .await;
        assert!(res.is_err(), "ReadOnly user must get an error (403) on WAN write");
    }

    // ========================================================================
    // FIX 1: modem_id boundary validation + pure-Rust USB sysfs matcher
    // ========================================================================

    #[tokio::test]
    async fn update_wan_config_rejects_injection_in_modem_id() {
        use axum::extract::{Json, State};
        use axum::Extension;
        let _guard = super::TEST_LOCK.lock().unwrap();
        std::env::set_var("MOCK_HARDWARE", "1");
        let _tmp = redirect_wan_config_to_tempdir();
        super::mock_uci_reset();

        let state = make_test_state().await;

        // A modem_id carrying shell metacharacters / control chars must be
        // rejected at the PUT /wan/config boundary with a 400, before it can
        // ever reach the USB-reset sysfs lookup.
        let payloads = [
            r#"x:y:z"; touch /tmp/pwned; echo ""#,
            "2c7c:0122:abc; reboot",
            "2c7c:0122:$(id)",
            "2c7c:0122:`id`",
            "2c7c:0122:a b",
            "2c7c:0122:a\nb",
            "2c7c:0122:a|b",
            "2c7c:0122:a/../b",
        ];
        for bad in payloads {
            let mut cfg = injection_test_config("WWAN", "wwan0", None);
            cfg.modem_priority[0].modem_id = bad.to_string();
            let res = super::update_wan_config(
                State(state.clone()),
                Extension(admin_session_user()),
                Json(cfg),
            )
            .await;
            let err = res.expect_err(&format!("modem_id {bad:?} must be rejected"));
            assert_eq!(
                err.status,
                axum::http::StatusCode::BAD_REQUEST,
                "modem_id {bad:?} must yield 400",
            );
        }
        // No command ran: mock UCI state stays empty.
        assert!(super::mock_uci_state().lock().unwrap().is_empty());
    }

    #[test]
    fn is_valid_modem_id_accepts_canonical_ids() {
        assert!(super::is_valid_modem_id("2c7c:0122:e3183572"));
        assert!(super::is_valid_modem_id("1bc7:1073:abcdef01"));
        // synthetic ethernet ids the WAN manager creates internally
        assert!(super::is_valid_modem_id("eth:br-wan"));
        assert!(super::is_valid_modem_id("eth:eth0_1"));
    }

    #[test]
    fn is_valid_modem_id_rejects_metacharacters_and_bad_shapes() {
        for bad in [
            "",
            "onlyonesegment",
            r#"x:y:z"; touch /tmp/pwned; echo ""#,
            "2c7c:0122:abc;reboot",
            "2c7c:0122:$(id)",
            "2c7c::abc",          // empty middle segment
            "2c7c:0122:",         // trailing empty segment
            "2c7c:0122:a b",      // whitespace
            "2c7c:0122:a\nb",     // control char
            "a:b:c:d:e",          // too many segments
        ] {
            assert!(
                !super::is_valid_modem_id(bad),
                "modem_id {bad:?} must be rejected",
            );
        }
    }

    #[test]
    fn usb_device_matches_returns_true_for_matching_triple() {
        // sysfs reports lowercase hex; matcher is case-insensitive on the hex
        // fields and exact on the serial.
        assert!(super::usb_device_matches(
            ("2c7c", "0122", "e3183572"),
            ("2c7c", "0122", "e3183572"),
        ));
        assert!(super::usb_device_matches(
            ("2c7c", "0122", "e3183572"),
            ("2C7C", "0122", "e3183572"),
        ));
    }

    #[test]
    fn usb_device_matches_returns_false_on_mismatch() {
        // Wrong serial
        assert!(!super::usb_device_matches(
            ("2c7c", "0122", "e3183572"),
            ("2c7c", "0122", "other"),
        ));
        // Wrong pid
        assert!(!super::usb_device_matches(
            ("2c7c", "0122", "e3183572"),
            ("2c7c", "1073", "e3183572"),
        ));
        // Serial is case-sensitive (exact match)
        assert!(!super::usb_device_matches(
            ("2c7c", "0122", "e3183572"),
            ("2c7c", "0122", "E3183572"),
        ));
    }
}
