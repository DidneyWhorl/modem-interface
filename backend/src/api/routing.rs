//! CTRL-WAN Policy-Based Routing Engine
//!
//! Manages per-interface routing tables via iproute2 commands.
//! Falls back to UCI metric-based routing when policy routing is unavailable
//! (mwan3 detected, iproute2 missing).

use std::collections::HashMap;
use std::process::Command;
use tracing::{info, warn, error, debug};

use crate::hardware::{
    FirewallBackend, PlatformCapabilities, RoutingMode, RoutingTableEntry,
};

/// Base routing table number. Each WAN gets table BASE + index.
const TABLE_BASE: u32 = 100;
/// Maximum number of WAN routing tables.
const TABLE_MAX_COUNT: u32 = 10;
/// Base ip rule priority. Each WAN gets PRIORITY_BASE + index.
const RULE_PRIORITY_BASE: u32 = 1000;

// ── Platform Detection ──────────────────────────────────────────────

/// Probe the system to determine routing and firewall capabilities.
/// Called once at startup. Result stored in AppState.
pub fn detect_platform() -> PlatformCapabilities {
    let ip_rule_works = check_iproute2();
    let fw_backend = detect_firewall_backend();
    let mwan3 = detect_mwan3();
    let openwrt_ver = detect_openwrt_version();

    let enabled = ip_rule_works && !mwan3;

    if mwan3 {
        warn!(
            "mwan3 detected — CTRL-WAN policy routing disabled. \
             Disable mwan3 to enable policy-based routing."
        );
    }
    if !ip_rule_works {
        warn!("iproute2 `ip rule` not functional — falling back to metric-based routing");
    }

    let caps = PlatformCapabilities {
        policy_routing_available: ip_rule_works,
        policy_routing_enabled: enabled,
        firewall_backend: fw_backend,
        mwan3_detected: mwan3,
        openwrt_version: openwrt_ver,
    };

    info!(
        "Platform detection complete: policy_routing={}, fw={:?}, mwan3={}, openwrt={:?}",
        caps.policy_routing_enabled,
        caps.firewall_backend,
        caps.mwan3_detected,
        caps.openwrt_version,
    );

    caps
}

/// Check if `ip rule show` executes successfully.
fn check_iproute2() -> bool {
    Command::new("ip")
        .args(["rule", "show"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Detect firewall backend: fw4 (nftables) vs fw3 (iptables) vs unknown.
fn detect_firewall_backend() -> FirewallBackend {
    let has_nft = Command::new("which")
        .arg("nft")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if has_nft {
        return FirewallBackend::Fw4;
    }

    let has_iptables = Command::new("which")
        .arg("iptables")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if has_iptables {
        return FirewallBackend::Fw3;
    }

    FirewallBackend::Unknown
}

/// Detect active mwan3 by checking for its rules in ip rule output
/// and the existence of its init script.
fn detect_mwan3() -> bool {
    let init_exists = std::path::Path::new("/etc/init.d/mwan3").exists();

    if !init_exists {
        return false;
    }

    let output = Command::new("ip")
        .args(["rule", "show"])
        .output();

    match output {
        Ok(o) if o.status.success() => {
            let rules = String::from_utf8_lossy(&o.stdout);
            rules.lines().any(|line| {
                line.trim()
                    .split(':')
                    .next()
                    .and_then(|p| p.trim().parse::<u32>().ok())
                    .map(|p| p >= 2000)
                    .unwrap_or(false)
            })
        }
        _ => false,
    }
}

/// Read OpenWRT version via ubus if available.
fn detect_openwrt_version() -> Option<String> {
    let output = Command::new("ubus")
        .args(["call", "system", "board"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let json_str = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str::<serde_json::Value>(&json_str)
        .ok()
        .and_then(|v| {
            v.get("release")
                .and_then(|r| r.get("version"))
                .and_then(|ver| ver.as_str())
                .map(|s| s.to_string())
        })
}

/// Parse the priority number from an `ip rule show` output line.
/// Format: "1000:\tfrom 192.168.225.45 lookup 100"
fn parse_rule_priority(line: &str) -> Option<u32> {
    line.trim()
        .split(':')
        .next()
        .and_then(|p| p.trim().parse::<u32>().ok())
}

// ── Command Helpers ─────────────────────────────────────────────────

/// Run an `ip` command, returning stdout on success or an error string.
fn run_ip_cmd(args: &[&str]) -> Result<String, String> {
    let output = Command::new("ip")
        .args(args)
        .output()
        .map_err(|e| format!("Failed to execute ip {}: {}", args.join(" "), e))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("ip {} failed: {}", args.join(" "), stderr.trim()))
    }
}

// ── Gateway Discovery ───────────────────────────────────────────────

/// Discover the default gateway for a network device from its routing table.
pub fn discover_gateway(device: &str) -> Option<String> {
    let output = run_ip_cmd(&["route", "show", "dev", device, "default"]).ok()?;

    for line in output.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 3 && parts[0] == "default" && parts[1] == "via" {
            return Some(parts[2].to_string());
        }
    }
    None
}

/// Get the first IPv4 address assigned to a device.
pub fn get_interface_ip(device: &str) -> Option<String> {
    let output = Command::new("ip")
        .args(["-4", "addr", "show", "dev", device])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("inet ") {
            return trimmed
                .split_whitespace()
                .nth(1)
                .and_then(|addr| addr.split('/').next())
                .map(|s| s.to_string());
        }
    }
    None
}

/// Parse the current main-table default route and return the device name.
/// Parses output of `ip route show default` for lines like:
///   "default via 192.168.215.1 dev br-wan"
///   "default dev usb0 scope link"
/// Returns the first default route's device (metric-less routes sort first).
pub fn get_main_default_device() -> Option<String> {
    let output = run_ip_cmd(&["route", "show", "default"]).ok()?;
    for line in output.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if let Some(dev_idx) = parts.iter().position(|&p| p == "dev") {
            if dev_idx + 1 < parts.len() {
                return Some(parts[dev_idx + 1].to_string());
            }
        }
    }
    None
}

// ── Table Lifecycle ─────────────────────────────────────────────────

/// Flush all CTRL-WAN managed ip rules (priority 1000-1099) and
/// routing tables (100-109). Called at startup for clean-slate and
/// at shutdown for cleanup.
pub fn flush_all_tables() {
    info!("Flushing CTRL-WAN routing tables and rules");

    if let Ok(output) = run_ip_cmd(&["rule", "show"]) {
        for line in output.lines() {
            if let Some(priority) = parse_rule_priority(line) {
                if (RULE_PRIORITY_BASE..RULE_PRIORITY_BASE + TABLE_MAX_COUNT).contains(&priority)
                {
                    let prio_str = priority.to_string();
                    if let Err(e) = run_ip_cmd(&["rule", "del", "priority", &prio_str]) {
                        debug!("Failed to delete rule priority {}: {}", priority, e);
                    }
                }
            }
        }
    }

    for i in 0..TABLE_MAX_COUNT {
        let table = TABLE_BASE + i;
        let table_str = table.to_string();
        let _ = run_ip_cmd(&["route", "flush", "table", &table_str]);
    }
}

/// Create a routing table entry for a WAN interface.
pub fn create_table_entry(entry: &RoutingTableEntry) -> Result<(), String> {
    let table_str = entry.table_number.to_string();
    let prio_str = entry.rule_priority.to_string();

    run_ip_cmd(&[
        "rule", "add",
        "from", &entry.source_ip,
        "lookup", &table_str,
        "priority", &prio_str,
    ])?;

    match &entry.gateway {
        Some(gw) => {
            run_ip_cmd(&[
                "route", "replace",
                "default", "via", gw,
                "dev", &entry.device,
                "table", &table_str,
            ])?;
        }
        None => {
            run_ip_cmd(&[
                "route", "replace",
                "default",
                "dev", &entry.device,
                "table", &table_str,
            ])?;
        }
    }

    info!(
        "Created routing table {}: from {} lookup {} via {:?} dev {}",
        entry.table_number, entry.source_ip, entry.table_number,
        entry.gateway, entry.device
    );

    Ok(())
}

/// Remove a routing table entry: delete the ip rule and flush the table.
pub fn remove_table_entry(entry: &RoutingTableEntry) -> Result<(), String> {
    let prio_str = entry.rule_priority.to_string();
    let table_str = entry.table_number.to_string();

    let _ = run_ip_cmd(&["rule", "del", "priority", &prio_str]);
    let _ = run_ip_cmd(&["route", "flush", "table", &table_str]);

    info!("Removed routing table {} for dev {}", entry.table_number, entry.device);
    Ok(())
}

// ── Initialization ──────────────────────────────────────────────────

/// Initialize routing tables from WAN config. Called at startup after
/// platform detection confirms policy routing is available.
///
/// 1. Flush any stale rules/tables from previous runs
/// 2. For each WAN entry with an IP, create a routing table
/// 3. Set main table default route to the configured primary
pub fn initialize_tables(
    wan_entries: &[(String, String, u32)], // Vec of (modem_id, network_device, config_index)
    routing_mode: &RoutingMode,
    weights: &HashMap<String, u32>,
    primary_id: Option<&str>,
) -> HashMap<String, RoutingTableEntry> {
    set_multipath_hash_sysctl();
    flush_all_tables();

    let mut state = HashMap::new();

    for (modem_id, device, idx) in wan_entries {
        let ip = match get_interface_ip(device) {
            Some(ip) => ip,
            None => {
                debug!("No IP on {} for {} — skipping table creation", device, modem_id);
                continue;
            }
        };

        let gateway = discover_gateway(device);
        let table_number = TABLE_BASE + idx;
        let rule_priority = RULE_PRIORITY_BASE + idx;

        let entry = RoutingTableEntry {
            table_number,
            rule_priority,
            gateway,
            device: device.clone(),
            source_ip: ip,
        };

        match create_table_entry(&entry) {
            Ok(()) => {
                state.insert(modem_id.clone(), entry);
            }
            Err(e) => {
                error!("Failed to create routing table for {}: {}", modem_id, e);
            }
        }
    }

    match routing_mode {
        RoutingMode::LoadBalance => {
            let multipath_entries: Vec<(RoutingTableEntry, u32)> = state
                .iter()
                .map(|(id, entry)| {
                    let w = weights.get(id).copied().unwrap_or(1).clamp(1, 10);
                    (entry.clone(), w)
                })
                .collect();
            if multipath_entries.len() > 1 {
                if let Err(e) = set_main_default_multipath(&multipath_entries) {
                    error!("Failed to set multipath default: {}", e);
                }
            } else if let Some((_id, entry)) = state.iter().next() {
                if let Err(e) = set_main_default(entry) {
                    error!("Failed to set main table default: {}", e);
                }
            }
        }
        RoutingMode::Failover => {
            let primary_entry = primary_id
                .and_then(|id| state.get(id))
                .or_else(|| state.values().next());
            if let Some(entry) = primary_entry {
                if let Err(e) = set_main_default(entry) {
                    error!("Failed to set main table default: {}", e);
                }
            }
        }
    }

    info!("Routing tables initialized: {} entries", state.len());
    state
}

/// Existing 1-arg API preserved (backward compat for all current callers).
pub fn set_main_default(entry: &RoutingTableEntry) -> Result<(), String> {
    set_main_default_with_caller(entry, "set_main_default")
}

/// Set the main routing table's default route to a specific WAN interface,
/// then defensively sweep any other metric-0 main-table defaults whose
/// nexthop dev set differs from `[entry.device]`. (Item #40.)
pub fn set_main_default_with_caller(
    entry: &RoutingTableEntry,
    caller: &str,
) -> Result<(), String> {
    match &entry.gateway {
        Some(gw) => {
            run_ip_cmd(&[
                "route", "replace", "default",
                "via", gw,
                "dev", &entry.device,
            ])?;
        }
        None => {
            run_ip_cmd(&[
                "route", "replace", "default",
                "dev", &entry.device,
            ])?;
        }
    }

    info!("Main table default route set to dev {} via {:?}", entry.device, entry.gateway);

    // Defensive sweep — clear any other metric=0 main-table defaults that
    // don't match the just-installed entry's dev set.
    let _ = clear_stale_main_metric_zero_defaults(&[entry.device.as_str()], caller);

    Ok(())
}

/// Build the argument list for a multipath default route.
/// Each entry is (RoutingTableEntry, weight). Returns empty vec if no entries.
pub fn build_multipath_args(entries: &[(RoutingTableEntry, u32)]) -> Vec<String> {
    if entries.is_empty() {
        return Vec::new();
    }

    let mut args = vec![
        "route".to_string(),
        "replace".to_string(),
        "default".to_string(),
    ];

    for (entry, weight) in entries {
        args.push("nexthop".to_string());
        if let Some(gw) = &entry.gateway {
            args.push("via".to_string());
            args.push(gw.clone());
        }
        args.push("dev".to_string());
        args.push(entry.device.clone());
        args.push("weight".to_string());
        args.push(weight.to_string());
    }

    args
}

/// Existing 1-arg API preserved.
pub fn apply_load_balance_route(
    routing_state: &HashMap<String, RoutingTableEntry>,
    active_healthy_ids: &[String],
    weights: &HashMap<String, u32>,
) -> Result<usize, String> {
    apply_load_balance_route_with_caller(routing_state, active_healthy_ids, weights, "apply_load_balance_route")
}

/// Caller-aware sibling. The transitive sweep runs inside the
/// `set_main_default*_with_caller` calls below, threading caller through.
pub fn apply_load_balance_route_with_caller(
    routing_state: &HashMap<String, RoutingTableEntry>,
    active_healthy_ids: &[String],
    weights: &HashMap<String, u32>,
    caller: &str,
) -> Result<usize, String> {
    let entries: Vec<(RoutingTableEntry, u32)> = active_healthy_ids
        .iter()
        .filter_map(|id| {
            routing_state.get(id).map(|entry| {
                let weight = weights.get(id).copied().unwrap_or(1).clamp(1, 10);
                (entry.clone(), weight)
            })
        })
        .collect();

    if entries.is_empty() {
        return Err("No healthy WANs with routing tables for multipath".to_string());
    }

    if entries.len() == 1 {
        set_main_default_with_caller(&entries[0].0, caller)?;
        return Ok(1);
    }

    set_main_default_multipath_with_caller(&entries, caller)?;
    Ok(entries.len())
}

/// Existing 1-arg API preserved (backward compat for all current callers).
pub fn set_main_default_multipath(entries: &[(RoutingTableEntry, u32)]) -> Result<(), String> {
    set_main_default_multipath_with_caller(entries, "set_main_default_multipath")
}

/// Set the main routing table's default route as a multipath route, then
/// defensively sweep any other metric-0 main-table defaults whose nexthop
/// dev set differs from the multipath dev set. (Item #40.)
pub fn set_main_default_multipath_with_caller(
    entries: &[(RoutingTableEntry, u32)],
    caller: &str,
) -> Result<(), String> {
    if entries.is_empty() {
        return Err("No entries for multipath route".to_string());
    }

    let args = build_multipath_args(entries);
    let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    run_ip_cmd(&arg_refs)?;

    let summary: Vec<String> = entries
        .iter()
        .map(|(e, w)| format!("dev {} via {:?} weight {}", e.device, e.gateway, w))
        .collect();
    info!("Multipath default route set: {}", summary.join(", "));

    // Defensive sweep, keeping the multipath nexthop dev set.
    let keep_devs: Vec<&str> = entries.iter().map(|(e, _)| e.device.as_str()).collect();
    let _ = clear_stale_main_metric_zero_defaults(&keep_devs, caller);

    Ok(())
}

// ── Stale Default Route Sweep (Item #40) ────────────────────────────

/// Identifies a single default route entry as parsed from
/// `ip route show default table main`. Single-nexthop routes have
/// nexthops.len() == 1; multipath routes have len() >= 2.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MainDefaultEntry {
    /// Explicit metric value. Implicit/missing metric in `ip route show`
    /// output -> 0.
    pub metric: u32,
    /// (gateway, dev) per nexthop. Gateway is None for direct-attached
    /// default (rare: `default dev wan` with no gateway).
    pub nexthops: Vec<(Option<String>, String)>,
    /// Verbatim `ip route del ...` arg list to remove this exact route.
    /// Preserved from input rather than reconstructed to avoid round-trip
    /// bugs (kernel is finicky about exact arg matching for multipath del).
    pub del_args: Vec<String>,
}

/// Parse `ip route show default table main` output into structured entries.
/// Skips non-default lines defensively. Pure -- testable with fixture strings.
///
/// Multipath continuation: lines starting with whitespace + `nexthop …`
/// are appended to the previous default entry's `nexthops` and `del_args`.
pub(crate) fn parse_main_default_routes(output: &str) -> Vec<MainDefaultEntry> {
    let mut entries: Vec<MainDefaultEntry> = Vec::new();

    for raw in output.lines() {
        let trimmed_for_classify = raw.trim_start();
        if trimmed_for_classify.is_empty() {
            continue;
        }

        let starts_with_default = trimmed_for_classify.starts_with("default")
            && raw.chars().next().map(|c| !c.is_whitespace()).unwrap_or(false);
        let is_continuation = raw.starts_with(char::is_whitespace)
            && trimmed_for_classify.starts_with("nexthop");

        if starts_with_default {
            let tokens: Vec<&str> = trimmed_for_classify.split_whitespace().collect();
            let metric = extract_metric_token(&tokens);
            let nexthop = extract_inline_nexthop(&tokens);

            // del_args = ["route", "del", <verbatim everything-after-`default`>]
            // Build the del prefix verbatim from the trimmed line.
            let mut del_args: Vec<String> = vec!["route".to_string(), "del".to_string()];
            del_args.extend(tokens.iter().map(|s| s.to_string()));

            let nexthops = match nexthop {
                Some(nh) => vec![nh],
                None => Vec::new(),
            };

            entries.push(MainDefaultEntry {
                metric,
                nexthops,
                del_args,
            });
            continue;
        }

        if is_continuation {
            if let Some(last) = entries.last_mut() {
                let tokens: Vec<&str> = trimmed_for_classify.split_whitespace().collect();
                if let Some(nh) = extract_inline_nexthop(&tokens) {
                    last.nexthops.push(nh);
                }
                // Append continuation tokens verbatim to del_args.
                last.del_args.extend(tokens.iter().map(|s| s.to_string()));
            }
            // Continuation with no preceding default: silently skip.
            continue;
        }

        // Non-default, non-continuation line — silently skip.
    }

    entries
}

/// Helper: extract `metric N` from a default line's tokens.
/// Implicit metric (no `metric` keyword) -> 0.
fn extract_metric_token(tokens: &[&str]) -> u32 {
    let mut iter = tokens.iter();
    while let Some(&tok) = iter.next() {
        if tok == "metric" {
            if let Some(&val) = iter.next() {
                return val.parse::<u32>().unwrap_or(0);
            }
        }
    }
    0
}

/// Helper: extract a single (gateway, dev) tuple from a token slice.
/// Looks for `via <gw>` and `dev <dev>` keyword pairs. Returns None if
/// no `dev` keyword is present (degenerate; shouldn't happen for
/// well-formed kernel output, but defensive).
fn extract_inline_nexthop(tokens: &[&str]) -> Option<(Option<String>, String)> {
    let mut gateway: Option<String> = None;
    let mut device: Option<String> = None;
    let mut iter = tokens.iter();
    while let Some(&tok) = iter.next() {
        match tok {
            "via" => {
                if let Some(&val) = iter.next() {
                    gateway = Some(val.to_string());
                }
            }
            "dev" => {
                if let Some(&val) = iter.next() {
                    device = Some(val.to_string());
                }
            }
            _ => {}
        }
    }
    device.map(|d| (gateway, d))
}

/// Decide whether a route is "stale" given the desired set of kept devices.
/// True iff: route.metric == 0 AND the set of nexthop devs in route !=
/// keep_devs (as a set). Strict set-equality semantics (see spec §3.3).
/// Pure.
pub(crate) fn should_delete(route: &MainDefaultEntry, keep_devs: &[&str]) -> bool {
    if route.metric != 0 {
        return false;
    }
    let route_devs: std::collections::HashSet<&str> =
        route.nexthops.iter().map(|(_, d)| d.as_str()).collect();
    let keep_set: std::collections::HashSet<&str> = keep_devs.iter().copied().collect();
    route_devs != keep_set
}

/// Composed shim. Runs `ip route show default table main`, parses, filters via
/// should_delete, issues `ip route del ...` for each, returns count deleted.
/// Idempotent. Defensive: on `ip route show` failure returns 0 (graceful
/// degradation -- the sweep is best-effort defense in depth).
///
/// Telemetry per spec §3.7:
/// - count >= 1 -> watchdog log line + tracing::info!
/// - count == 0 -> tracing::debug! only (default-filtered out at info level)
/// - individual `ip route del` failures -> tracing::warn!, continue
pub fn clear_stale_main_metric_zero_defaults(keep_devs: &[&str], caller: &str) -> usize {
    let output = match run_ip_cmd(&["route", "show", "default", "table", "main"]) {
        Ok(s) => s,
        Err(e) => {
            warn!(
                target: "wan_routing",
                caller = %caller,
                error = %e,
                "ip route show default table main failed; sweep degrades to no-op"
            );
            return 0;
        }
    };

    let entries = parse_main_default_routes(&output);
    let mut deleted: usize = 0;
    let mut removed_summaries: Vec<String> = Vec::new();

    for entry in &entries {
        if !should_delete(entry, keep_devs) {
            continue;
        }
        // Build &str slice for run_ip_cmd from the verbatim del_args.
        let arg_refs: Vec<&str> = entry.del_args.iter().map(|s| s.as_str()).collect();
        match run_ip_cmd(&arg_refs) {
            Ok(_) => {
                deleted += 1;
                // Build a compact summary "<gw> dev <dev>" or
                // "<gw1> dev <dev1>+<gw2> dev <dev2>" for multipath.
                let summary = entry
                    .nexthops
                    .iter()
                    .map(|(gw, dev)| match gw {
                        Some(g) => format!("{g} dev {dev}"),
                        None => format!("(no-gw) dev {dev}"),
                    })
                    .collect::<Vec<_>>()
                    .join("+");
                removed_summaries.push(summary);
            }
            Err(e) => {
                warn!(
                    target: "wan_routing",
                    caller = %caller,
                    error = %e,
                    del_args = ?entry.del_args,
                    "ip route del failed; race with concurrent change is acceptable"
                );
                // Continue — race-window failures are not fatal.
            }
        }
    }

    if deleted >= 1 {
        // Watchdog log line (operator-facing, mode-agnostic — only devs and gws).
        let timestamp = chrono::Utc::now().to_rfc3339();
        let kept_str = keep_devs.join(",");
        let removed_str = removed_summaries.join(";");
        let log_line = format!(
            "{timestamp} STALE_DEFAULT_CLEARED count={deleted} kept=[{kept_str}] removed=[{removed_str}] caller={caller}"
        );
        // append_watchdog_log is async; spawn-detach to avoid making this
        // function async (callers in routing.rs are sync).
        let log_line_owned = log_line.clone();
        tokio::spawn(async move {
            let _ = crate::config::wan::append_watchdog_log(&log_line_owned).await;
        });

        info!(
            target: "wan_routing",
            count = deleted,
            kept_devs = ?keep_devs,
            removed_routes = ?removed_summaries,
            caller = %caller,
            "Cleared stale main-table metric-0 defaults"
        );
    } else {
        debug!(
            target: "wan_routing",
            count = 0,
            kept_devs = ?keep_devs,
            caller = %caller,
            "Sweep no-op"
        );
    }

    deleted
}

/// Set the L4 multipath hash policy sysctl.
/// Enables 5-tuple hashing for multipath routes.
/// Idempotent — safe to call in both routing modes.
pub fn set_multipath_hash_sysctl() {
    match Command::new("sysctl")
        .args(["-w", "net.ipv4.fib_multipath_hash_policy=1"])
        .output()
    {
        Ok(output) if output.status.success() => {
            info!("Set fib_multipath_hash_policy=1 (L4 hash)");
        }
        Ok(output) => {
            warn!(
                "sysctl fib_multipath_hash_policy failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Err(e) => {
            warn!("Failed to run sysctl: {e}");
        }
    }
}

// ── Reconciliation ──────────────────────────────────────────────────

/// Compare desired routing state to actual system state and fix drift.
/// Called every watchdog cycle (30s).
pub fn reconcile(
    current_state: &mut HashMap<String, RoutingTableEntry>,
    wan_entries: &[(String, String, u32)],
    expected_device: Option<&str>,
) -> Vec<String> {
    let mut changes = Vec::new();

    let system_rules = get_system_rules();

    for (modem_id, device, idx) in wan_entries {
        let ip = get_interface_ip(device);
        let gateway = discover_gateway(device);
        let table_number = TABLE_BASE + idx;
        let rule_priority = RULE_PRIORITY_BASE + idx;

        match (current_state.get(modem_id), &ip) {
            (None, Some(ip_addr)) => {
                let entry = RoutingTableEntry {
                    table_number,
                    rule_priority,
                    gateway,
                    device: device.clone(),
                    source_ip: ip_addr.clone(),
                };
                if create_table_entry(&entry).is_ok() {
                    changes.push(format!("Created table {table_number} for {modem_id}"));
                    current_state.insert(modem_id.clone(), entry);
                }
            }

            (Some(existing), Some(ip_addr)) => {
                let mut needs_rebuild = false;

                if &existing.source_ip != ip_addr {
                    needs_rebuild = true;
                    changes.push(format!(
                        "IP changed for {modem_id} ({} -> {ip_addr})", existing.source_ip
                    ));
                }

                if !system_rules.contains(&existing.rule_priority) {
                    needs_rebuild = true;
                    let rule_prio = existing.rule_priority;
                    changes.push(format!("Rule {rule_prio} missing, recreating"));
                }

                if needs_rebuild {
                    let _ = remove_table_entry(existing);
                    let new_entry = RoutingTableEntry {
                        table_number,
                        rule_priority,
                        gateway,
                        device: device.clone(),
                        source_ip: ip_addr.clone(),
                    };
                    if create_table_entry(&new_entry).is_ok() {
                        current_state.insert(modem_id.clone(), new_entry);
                    }
                }
            }

            (Some(existing), None) => {
                let _ = remove_table_entry(existing);
                current_state.remove(modem_id);
                changes.push(format!("Removed table for {modem_id} (interface down)"));
            }

            (None, None) => {}
        }
    }

    let wan_ids: Vec<&String> = wan_entries.iter().map(|(id, _, _)| id).collect();
    let stale_ids: Vec<String> = current_state
        .keys()
        .filter(|id| !wan_ids.contains(id))
        .cloned()
        .collect();

    for stale_id in stale_ids {
        if let Some(entry) = current_state.remove(&stale_id) {
            let _ = remove_table_entry(&entry);
            changes.push(format!("Removed stale table for {stale_id}"));
        }
    }

    // Validate main default route points to expected device
    if let Some(expected) = expected_device {
        if let Some(actual) = get_main_default_device() {
            if actual != expected {
                if let Some(entry) = current_state.values().find(|e| e.device == expected) {
                    if set_main_default(entry).is_ok() {
                        changes.push(format!(
                            "Main default route drifted ({actual} -> {expected}), corrected"
                        ));
                    }
                }
            }
        }
    }

    if !changes.is_empty() {
        info!("Routing reconciliation: {}", changes.join("; "));
    }

    changes
}

/// Read current system rules and return the set of priorities in our range.
fn get_system_rules() -> Vec<u32> {
    let output = match run_ip_cmd(&["rule", "show"]) {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };

    output
        .lines()
        .filter_map(parse_rule_priority)
        .filter(|&p| (RULE_PRIORITY_BASE..RULE_PRIORITY_BASE + TABLE_MAX_COUNT).contains(&p))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_platform_capabilities_disables_routing() {
        let caps = PlatformCapabilities::default();
        assert!(!caps.policy_routing_available);
        assert!(!caps.policy_routing_enabled);
        assert!(!caps.mwan3_detected);
        assert_eq!(caps.firewall_backend, FirewallBackend::Unknown);
    }

    #[test]
    fn platform_enabled_requires_available_and_no_mwan3() {
        let caps = PlatformCapabilities {
            policy_routing_available: true,
            policy_routing_enabled: true,
            firewall_backend: FirewallBackend::Fw4,
            mwan3_detected: false,
            openwrt_version: Some("23.05.0".to_string()),
        };
        assert!(caps.policy_routing_enabled);

        let caps_with_mwan3 = PlatformCapabilities {
            policy_routing_available: true,
            policy_routing_enabled: false,
            mwan3_detected: true,
            ..caps.clone()
        };
        assert!(!caps_with_mwan3.policy_routing_enabled);
    }

    #[test]
    fn parse_rule_priority_standard_format() {
        assert_eq!(parse_rule_priority("1000:\tfrom 192.168.225.45 lookup 100"), Some(1000));
        assert_eq!(parse_rule_priority("1001:  from 10.0.0.1 lookup 101"), Some(1001));
        assert_eq!(parse_rule_priority("32766:\tfrom all lookup main"), Some(32766));
    }

    #[test]
    fn parse_rule_priority_invalid_lines() {
        assert_eq!(parse_rule_priority(""), None);
        assert_eq!(parse_rule_priority("not a rule"), None);
    }

    #[test]
    fn table_entry_numbering_within_range() {
        for i in 0..TABLE_MAX_COUNT {
            let table = TABLE_BASE + i;
            let priority = RULE_PRIORITY_BASE + i;
            assert!(table >= 100 && table <= 109);
            assert!(priority >= 1000 && priority <= 1009);
        }
    }

    #[test]
    fn routing_table_entry_serializes() {
        let entry = RoutingTableEntry {
            table_number: 100,
            rule_priority: 1000,
            gateway: Some("192.168.225.1".to_string()),
            device: "usb0".to_string(),
            source_ip: "192.168.225.45".to_string(),
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("\"table_number\":100"));
        assert!(json.contains("\"source_ip\":\"192.168.225.45\""));
    }

    #[test]
    fn table_numbering_from_config_index() {
        assert_eq!(TABLE_BASE + 0, 100);
        assert_eq!(RULE_PRIORITY_BASE + 0, 1000);
        assert_eq!(TABLE_BASE + 2, 102);
        assert_eq!(RULE_PRIORITY_BASE + 2, 1002);
        assert_eq!(TABLE_BASE + 9, 109);
        assert_eq!(RULE_PRIORITY_BASE + 9, 1009);
    }

    fn test_routing_entry(device: &str, gateway: Option<&str>) -> RoutingTableEntry {
        RoutingTableEntry {
            table_number: 100,
            rule_priority: 1000,
            gateway: gateway.map(|s| s.to_string()),
            device: device.to_string(),
            source_ip: "10.0.0.1".to_string(),
        }
    }

    fn s(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn multipath_args_two_wans_with_gateways() {
        let entries = vec![
            (test_routing_entry("usb0", Some("10.0.0.1")), 3),
            (test_routing_entry("usb1", Some("10.0.1.1")), 2),
        ];
        assert_eq!(
            build_multipath_args(&entries),
            s(&[
                "route", "replace", "default",
                "nexthop", "via", "10.0.0.1", "dev", "usb0", "weight", "3",
                "nexthop", "via", "10.0.1.1", "dev", "usb1", "weight", "2",
            ])
        );
    }

    #[test]
    fn multipath_args_single_wan() {
        let entries = vec![(test_routing_entry("usb0", Some("10.0.0.1")), 1)];
        assert_eq!(
            build_multipath_args(&entries),
            s(&[
                "route", "replace", "default",
                "nexthop", "via", "10.0.0.1", "dev", "usb0", "weight", "1",
            ])
        );
    }

    #[test]
    fn multipath_args_no_gateway_p2p() {
        let entries = vec![
            (test_routing_entry("usb0", None), 3),
            (test_routing_entry("usb1", Some("10.0.1.1")), 2),
        ];
        assert_eq!(
            build_multipath_args(&entries),
            s(&[
                "route", "replace", "default",
                "nexthop", "dev", "usb0", "weight", "3",
                "nexthop", "via", "10.0.1.1", "dev", "usb1", "weight", "2",
            ])
        );
    }

    #[test]
    fn multipath_args_empty_returns_empty() {
        let entries: Vec<(RoutingTableEntry, u32)> = vec![];
        assert!(build_multipath_args(&entries).is_empty());
    }

    #[test]
    fn set_multipath_empty_returns_error() {
        let entries: Vec<(RoutingTableEntry, u32)> = vec![];
        assert!(set_main_default_multipath(&entries).is_err());
    }

    #[test]
    fn apply_load_balance_no_healthy_wans_errors() {
        let routing_state = HashMap::new();
        let result = apply_load_balance_route(&routing_state, &[], &HashMap::new());
        assert!(result.is_err());
    }

    #[test]
    fn apply_load_balance_filters_missing_routing_entries() {
        let routing_state = HashMap::new();
        let ids = vec!["modem1".to_string()];
        let result = apply_load_balance_route(&routing_state, &ids, &HashMap::new());
        assert!(result.is_err());
    }

    #[test]
    fn get_system_rules_parses_mixed_output() {
        let lines = vec![
            "0:\tfrom all lookup local",
            "1000:\tfrom 192.168.225.45 lookup 100",
            "1001:\tfrom 10.0.0.1 lookup 101",
            "32766:\tfrom all lookup main",
            "32767:\tfrom all lookup default",
        ];
        let priorities: Vec<u32> = lines
            .iter()
            .filter_map(|line| parse_rule_priority(line))
            .filter(|&p| p >= RULE_PRIORITY_BASE && p < RULE_PRIORITY_BASE + TABLE_MAX_COUNT)
            .collect();
        assert_eq!(priorities, vec![1000, 1001]);
    }

    #[test]
    fn parse_main_default_device_from_route_output() {
        fn parse_default_device(output: &str) -> Option<String> {
            for line in output.lines() {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if let Some(dev_idx) = parts.iter().position(|&p| p == "dev") {
                    if dev_idx + 1 < parts.len() {
                        return Some(parts[dev_idx + 1].to_string());
                    }
                }
            }
            None
        }

        assert_eq!(
            parse_default_device("default via 192.168.215.1 dev br-wan"),
            Some("br-wan".to_string())
        );
        assert_eq!(
            parse_default_device("default via 192.168.215.1 dev br-wan proto static src 192.168.215.216 metric 20\ndefault via 192.168.224.1 dev usb0 metric 30"),
            Some("br-wan".to_string())
        );
        assert_eq!(
            parse_default_device("default dev usb0 scope link"),
            Some("usb0".to_string())
        );
        assert_eq!(parse_default_device(""), None);
        assert_eq!(
            parse_default_device("192.168.1.0/24 dev br-lan proto static scope link"),
            Some("br-lan".to_string())
        );
    }

    // ====================================================================
    // Item #40 — parser tests (T1-T10)
    // ====================================================================

    fn entry_devs(e: &MainDefaultEntry) -> Vec<&str> {
        e.nexthops.iter().map(|(_, d)| d.as_str()).collect()
    }

    #[test]
    fn t1_parse_empty_returns_empty() {
        assert!(parse_main_default_routes("").is_empty());
    }

    #[test]
    fn t2_parse_single_nexthop_with_explicit_metric() {
        let out = "default via 192.168.215.1 dev wan metric 30\n";
        let entries = parse_main_default_routes(out);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].metric, 30);
        assert_eq!(entries[0].nexthops, vec![(Some("192.168.215.1".to_string()), "wan".to_string())]);
    }

    #[test]
    fn t3_parse_single_nexthop_implicit_metric_is_zero() {
        let out = "default via 192.0.0.1 dev wwan0\n";
        let entries = parse_main_default_routes(out);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].metric, 0);
    }

    #[test]
    fn t4_parse_single_nexthop_no_gateway() {
        let out = "default dev wan\n";
        let entries = parse_main_default_routes(out);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].nexthops, vec![(None, "wan".to_string())]);
    }

    #[test]
    fn t5_parse_multipath_two_nexthops() {
        let out = "default proto static metric 0\n\
                   \tnexthop via 192.0.0.1 dev wwan0 weight 5\n\
                   \tnexthop via 192.168.215.1 dev wan weight 5\n";
        let entries = parse_main_default_routes(out);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].metric, 0);
        assert_eq!(entry_devs(&entries[0]), vec!["wwan0", "wan"]);
    }

    #[test]
    fn t6_parse_multipath_three_nexthops() {
        let out = "default proto static metric 0\n\
                   \tnexthop via 10.0.0.1 dev wwan0 weight 5\n\
                   \tnexthop via 10.0.1.1 dev wwan1 weight 5\n\
                   \tnexthop via 192.168.215.1 dev wan weight 5\n";
        let entries = parse_main_default_routes(out);
        assert_eq!(entries.len(), 1);
        assert_eq!(entry_devs(&entries[0]), vec!["wwan0", "wwan1", "wan"]);
        // Lock the del_args concatenation contract (Q-B): header tokens
        // followed by each continuation's tokens, in line order.
        assert!(entries[0].del_args.starts_with(&[
            "route".to_string(), "del".to_string(),
            "default".to_string(), "proto".to_string(), "static".to_string(),
            "metric".to_string(), "0".to_string(),
        ]));
        // Three "nexthop" tokens follow — count them.
        let nexthop_count = entries[0].del_args.iter().filter(|t| *t == "nexthop").count();
        assert_eq!(nexthop_count, 3);
    }

    #[test]
    fn t7_parse_steady_state_three_defaults() {
        // Bench post-fix Failover-mode steady state.
        let out = "default via 192.0.0.1 dev wwan0\n\
                   default via 192.0.0.1 dev wwan0 metric 20\n\
                   default via 192.168.215.1 dev wan metric 30\n";
        let entries = parse_main_default_routes(out);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].metric, 0);
        assert_eq!(entries[0].nexthops[0].1, "wwan0");
        assert_eq!(entries[1].metric, 20);
        assert_eq!(entries[2].metric, 30);
    }

    #[test]
    fn t8_parse_drift_state_item_40() {
        // Bench's Item #40 drift fixture — entry 0 dev=wan (the bug).
        let out = "default via 192.168.215.1 dev wan\n\
                   default via 192.0.0.1 dev wwan0 metric 20\n\
                   default via 192.168.215.1 dev wan metric 30\n";
        let entries = parse_main_default_routes(out);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].metric, 0);
        assert_eq!(entries[0].nexthops[0].1, "wan");
    }

    #[test]
    fn t9_parse_skips_non_default_lines() {
        let out = "default via 192.0.0.1 dev wwan0\n\
                   192.168.0.0/24 dev br-lan proto kernel scope link\n\
                   10.0.0.0/8 via 10.255.255.1 dev tun0\n\
                   default via 192.168.215.1 dev wan metric 30\n";
        let entries = parse_main_default_routes(out);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].nexthops[0].1, "wwan0");
        assert_eq!(entries[1].nexthops[0].1, "wan");
    }

    #[test]
    fn t10_parse_preserves_extra_keywords_in_del_args() {
        let out = "default via 192.168.215.1 dev wan proto static scope link\n";
        let entries = parse_main_default_routes(out);
        assert_eq!(entries.len(), 1);
        // Verify del_args round-trips the extra keywords verbatim.
        assert!(entries[0].del_args.contains(&"proto".to_string()));
        assert!(entries[0].del_args.contains(&"static".to_string()));
        assert!(entries[0].del_args.contains(&"scope".to_string()));
        assert!(entries[0].del_args.contains(&"link".to_string()));
    }

    // ====================================================================
    // Item #40 — predicate truth-table tests (T11-T19)
    // ====================================================================

    fn route(metric: u32, devs: &[&str]) -> MainDefaultEntry {
        MainDefaultEntry {
            metric,
            nexthops: devs.iter().map(|d| (None, d.to_string())).collect(),
            del_args: Vec::new(),
        }
    }

    #[test]
    fn t11_should_delete_metric_zero_match_keep_dev_false() {
        assert!(!should_delete(&route(0, &["wwan0"]), &["wwan0"]));
    }

    #[test]
    fn t12_should_delete_metric_zero_wrong_dev_true() {
        assert!(should_delete(&route(0, &["wan"]), &["wwan0"]));
    }

    #[test]
    fn t13_should_delete_metric_zero_multipath_leftover_true() {
        assert!(should_delete(&route(0, &["wwan0", "wan"]), &["wwan0"]));
    }

    #[test]
    fn t14_should_delete_metric_zero_subset_after_multipath_true() {
        assert!(should_delete(&route(0, &["wwan0"]), &["wwan0", "wan"]));
    }

    #[test]
    fn t15_should_delete_metric_zero_multipath_set_equal_false() {
        assert!(!should_delete(&route(0, &["wwan0", "wan"]), &["wwan0", "wan"]));
    }

    #[test]
    fn t16_should_delete_metric_zero_no_nexthops_true() {
        assert!(should_delete(&route(0, &[]), &["wwan0"]));
    }

    #[test]
    fn t17_should_delete_metric_20_same_dev_false() {
        assert!(!should_delete(&route(20, &["wwan0"]), &["wwan0"]));
    }

    #[test]
    fn t18_should_delete_metric_20_diff_dev_false() {
        assert!(!should_delete(&route(20, &["wan"]), &["wwan0"]));
    }

    #[test]
    fn t19_should_delete_metric_1_diff_dev_false() {
        assert!(!should_delete(&route(1, &["wan"]), &["wwan0"]));
    }

    // ====================================================================
    // Item #40 — composed parser+predicate cross-cutting (T20-T23)
    // ====================================================================

    #[test]
    fn t20_bench_drift_state_predicate_isolates_stale() {
        let out = "default via 192.168.215.1 dev wan\n\
                   default via 192.0.0.1 dev wwan0 metric 20\n\
                   default via 192.168.215.1 dev wan metric 30\n";
        let entries = parse_main_default_routes(out);
        let keep = ["wwan0"];
        let marks: Vec<bool> = entries.iter().map(|e| should_delete(e, &keep)).collect();
        assert_eq!(marks, vec![true, false, false],
                   "exactly entry 0 (metric=0 dev=wan) marked stale");
    }

    #[test]
    fn t21_bench_steady_state_predicate_no_op() {
        let out = "default via 192.0.0.1 dev wwan0\n\
                   default via 192.0.0.1 dev wwan0 metric 20\n\
                   default via 192.168.215.1 dev wan metric 30\n";
        let entries = parse_main_default_routes(out);
        let keep = ["wwan0"];
        let any_stale = entries.iter().any(|e| should_delete(e, &keep));
        assert!(!any_stale, "post-fix Failover steady state must yield no deletions");
    }

    #[test]
    fn t22_loadbalance_steady_state_predicate_no_op() {
        let out = "default proto static metric 0\n\
                   \tnexthop via 192.0.0.1 dev wwan0 weight 5\n\
                   \tnexthop via 192.168.215.1 dev wan weight 5\n";
        let entries = parse_main_default_routes(out);
        let keep = ["wwan0", "wan"];
        let any_stale = entries.iter().any(|e| should_delete(e, &keep));
        assert!(!any_stale, "LoadBalance steady state set-equal must yield no deletions");
    }

    #[test]
    fn t23_loadbalance_post_failover_drift_predicate() {
        // Multipath leftover + single-nexthop metric=0 via wan exists.
        let out = "default via 192.168.215.1 dev wan\n\
                   default proto static metric 0\n\
                   \tnexthop via 192.0.0.1 dev wwan0 weight 5\n\
                   \tnexthop via 192.168.215.1 dev wan weight 5\n";
        let entries = parse_main_default_routes(out);
        let keep = ["wwan0", "wan"];
        let marks: Vec<bool> = entries.iter().map(|e| should_delete(e, &keep)).collect();
        assert_eq!(marks, vec![true, false],
                   "single-nexthop leftover deletes; multipath set-equal kept");
    }
}
