//! CTRL-WAN Traffic Steering Rules — Types, Persistence, Firewall Rule Generation,
//! Validation, and System Command Execution
//!
//! Core data types, JSON file I/O, firewall command builders, rule validation,
//! and system command execution for Level 2 traffic steering rules.
//!
//! These types are consumed by API route handlers (Task 5) and AppState
//! integration (Task 4).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::net::IpAddr;
use std::path::Path;
use std::process::Command;
use tracing::{info, warn};

use crate::hardware::{FirewallBackend, RoutingTableEntry};

// ── Constants ──────────────────────────────────────────────────────────

/// Base fwmark value for steering rules. Rule N gets mark BASE + N.
pub const STEERING_MARK_BASE: u32 = 100;
/// Base ip rule priority for steering rules. Rule N gets priority BASE + N.
pub const STEERING_PRIORITY_BASE: u32 = 900;
/// Maximum number of steering rules allowed.
pub const STEERING_MAX_RULES: u32 = 50;
/// Default config file path on the router.
pub const STEERING_CONFIG_PATH: &str = "/etc/modem-interface/steering-rules.json";

// ── Enums ──────────────────────────────────────────────────────────────

/// IP protocol matcher for steering rules.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    Tcp,
    Udp,
    Icmp,
}

/// Port match — single port or inclusive range.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum PortMatch {
    Single(u16),
    Range(u16, u16),
}

/// What happens when the target WAN goes down.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum FailoverMode {
    /// Traffic routes through next available WAN automatically.
    #[default]
    Automatic,
    /// Traffic routes through fallback_wan if set, else next available.
    PreferredFallback,
    /// Traffic is dropped if target WAN is unavailable.
    Strict,
}

/// Runtime status of a steering rule.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RuleStatus {
    /// Rule is active and enforced.
    #[default]
    Active,
    /// Rule is enabled but target WAN is down.
    Dormant,
    /// Rule is disabled or blocked by conflict.
    Blocked,
}

// ── Structs ────────────────────────────────────────────────────────────

/// A single traffic steering rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SteeringRule {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    #[serde(default)]
    pub priority: u32,
    pub source_ip: Option<Vec<String>>,
    pub destination_ip: Option<Vec<String>>,
    pub protocol: Option<Protocol>,
    pub destination_port: Option<PortMatch>,
    pub source_port: Option<PortMatch>,
    pub target_wan: String,
    #[serde(default)]
    pub failover_mode: FailoverMode,
    pub fallback_wan: Option<String>,
    /// Runtime status — not persisted to disk.
    #[serde(default)]
    pub status: RuleStatus,
    /// Runtime fwmark — not persisted to disk.
    #[serde(default)]
    pub fwmark: u32,
}

/// Top-level config wrapper for steering rules.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SteeringConfig {
    pub rules: Vec<SteeringRule>,
}

// ── Persistence ────────────────────────────────────────────────────────

/// Load steering rules from a JSON file.
/// Returns empty vec if the file does not exist or cannot be parsed.
pub fn load_rules(path: &str) -> Vec<SteeringRule> {
    let p = Path::new(path);
    if !p.exists() {
        info!("Steering config not found at {path}, starting with empty ruleset");
        return Vec::new();
    }

    match fs::read_to_string(p) {
        Ok(contents) => match serde_json::from_str::<SteeringConfig>(&contents) {
            Ok(config) => {
                info!("Loaded {} steering rules from {path}", config.rules.len());
                config.rules
            }
            Err(e) => {
                warn!("Failed to parse steering config at {path}: {e}");
                Vec::new()
            }
        },
        Err(e) => {
            warn!("Failed to read steering config at {path}: {e}");
            Vec::new()
        }
    }
}

/// Save steering rules to a JSON file.
/// Resets runtime fields (status, fwmark) before writing.
/// Creates parent directories if needed.
pub fn save_rules(path: &str, rules: &[SteeringRule]) -> Result<(), String> {
    // Clone and reset runtime fields before saving
    let clean_rules: Vec<SteeringRule> = rules
        .iter()
        .map(|r| SteeringRule {
            status: RuleStatus::Active,
            fwmark: 0,
            ..r.clone()
        })
        .collect();

    let config = SteeringConfig {
        rules: clean_rules,
    };

    let json = serde_json::to_string_pretty(&config)
        .map_err(|e| format!("Failed to serialize steering config: {e}"))?;

    // Ensure parent directory exists
    if let Some(parent) = Path::new(path).parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create directory {}: {e}", parent.display()))?;
    }

    crate::config::write_secret_file_blocking(path, json)
        .map_err(|e| format!("Failed to write steering config to {path}: {e}"))?;

    info!("Saved {} steering rules to {path}", rules.len());
    Ok(())
}

// ── Priority Assignment ────────────────────────────────────────────────

/// Assign priorities and fwmarks to rules based on their position.
/// First rule gets priority 900 / fwmark 100, second gets 901 / 101, etc.
pub fn assign_priorities(rules: &mut [SteeringRule]) {
    for (i, rule) in rules.iter_mut().enumerate() {
        let idx = i as u32;
        rule.priority = STEERING_PRIORITY_BASE + idx;
        rule.fwmark = STEERING_MARK_BASE + idx;
    }
}

// ── Firewall Constants ────────────────────────────────────────────────

/// nftables table name for steering rules.
pub const NFT_TABLE: &str = "inet ctrl_wan";
/// nftables chain name for steering rules.
pub const NFT_CHAIN: &str = "steering";
/// iptables chain name for steering rules (in mangle table).
pub const IPT_CHAIN: &str = "ctrl_wan_steering";

// ── Firewall Rule Generators ──────────────────────────────────────────

/// Generate an nftables expression string for a steering rule.
///
/// Builds match criteria from the rule's fields (omitted fields match all),
/// ending with `mark set 0x{fwmark}`. Returns a vec with one expression string.
pub fn generate_nft_rule(rule: &SteeringRule) -> Vec<String> {
    let mut parts: Vec<String> = Vec::new();

    if let Some(ref saddrs) = rule.source_ip {
        if saddrs.len() == 1 {
            parts.push(format!("ip saddr {}", saddrs[0]));
        } else if saddrs.len() > 1 {
            parts.push(format!("ip saddr {{ {} }}", saddrs.join(", ")));
        }
    }
    if let Some(ref daddrs) = rule.destination_ip {
        if daddrs.len() == 1 {
            parts.push(format!("ip daddr {}", daddrs[0]));
        } else if daddrs.len() > 1 {
            parts.push(format!("ip daddr {{ {} }}", daddrs.join(", ")));
        }
    }

    match rule.protocol {
        Some(Protocol::Icmp) => {
            parts.push("meta l4proto icmp".to_string());
            // ICMP has no port matching
        }
        Some(ref proto) => {
            let proto_str = match proto {
                Protocol::Tcp => "tcp",
                Protocol::Udp => "udp",
                Protocol::Icmp => unreachable!(),
            };
            parts.push(format!("meta l4proto {proto_str}"));

            if let Some(ref port) = rule.source_port {
                match port {
                    PortMatch::Single(p) => parts.push(format!("{proto_str} sport {p}")),
                    PortMatch::Range(lo, hi) => parts.push(format!("{proto_str} sport {lo}-{hi}")),
                }
            }
            if let Some(ref port) = rule.destination_port {
                match port {
                    PortMatch::Single(p) => parts.push(format!("{proto_str} dport {p}")),
                    PortMatch::Range(lo, hi) => parts.push(format!("{proto_str} dport {lo}-{hi}")),
                }
            }
        }
        None => {
            // No protocol — skip port matching entirely
        }
    }

    parts.push(format!("mark set 0x{:x}", rule.fwmark));

    vec![parts.join(" ")]
}

/// Generate iptables-mangle arguments for a steering rule.
///
/// Returns a vec of argument sets to append after `iptables -t mangle -A ctrl_wan_steering`.
/// Multiple entries are generated when multiple source/destination IPs are present,
/// since iptables does not support set syntax. Each inner vec is one complete rule.
pub fn generate_iptables_rules(rule: &SteeringRule) -> Vec<Vec<String>> {
    // Build the list of source and destination IPs to iterate over.
    // An empty vec means "no filter on this dimension".
    let sources: Vec<Option<&str>> = match rule.source_ip {
        Some(ref addrs) if !addrs.is_empty() => addrs.iter().map(|s| Some(s.as_str())).collect(),
        _ => vec![None],
    };
    let destinations: Vec<Option<&str>> = match rule.destination_ip {
        Some(ref addrs) if !addrs.is_empty() => addrs.iter().map(|s| Some(s.as_str())).collect(),
        _ => vec![None],
    };

    // Build shared protocol/port/mark suffix
    let mut suffix: Vec<String> = Vec::new();
    match rule.protocol {
        Some(Protocol::Icmp) => {
            suffix.push("-p".to_string());
            suffix.push("icmp".to_string());
        }
        Some(ref proto) => {
            let proto_str = match proto {
                Protocol::Tcp => "tcp",
                Protocol::Udp => "udp",
                Protocol::Icmp => unreachable!(),
            };
            suffix.push("-p".to_string());
            suffix.push(proto_str.to_string());

            if let Some(ref port) = rule.source_port {
                suffix.push("--sport".to_string());
                match port {
                    PortMatch::Single(p) => suffix.push(p.to_string()),
                    PortMatch::Range(lo, hi) => suffix.push(format!("{lo}:{hi}")),
                }
            }
            if let Some(ref port) = rule.destination_port {
                suffix.push("--dport".to_string());
                match port {
                    PortMatch::Single(p) => suffix.push(p.to_string()),
                    PortMatch::Range(lo, hi) => suffix.push(format!("{lo}:{hi}")),
                }
            }
        }
        None => {}
    }
    suffix.push("-j".to_string());
    suffix.push("MARK".to_string());
    suffix.push("--set-mark".to_string());
    suffix.push(rule.fwmark.to_string());

    // Generate one rule per source/destination combination
    let mut result = Vec::new();
    for src in &sources {
        for dst in &destinations {
            let mut args: Vec<String> = Vec::new();
            if let Some(s) = src {
                args.push("-s".to_string());
                args.push(s.to_string());
            }
            if let Some(d) = dst {
                args.push("-d".to_string());
                args.push(d.to_string());
            }
            args.extend(suffix.clone());
            result.push(args);
        }
    }

    result
}

/// Generate `ip rule add` arguments for fwmark-based policy routing.
///
/// Returns args to pass after `ip`: `["rule", "add", "fwmark", ...]`
pub fn fwmark_ip_rule_add_args(fwmark: u32, table: u32, priority: u32) -> Vec<String> {
    vec![
        "rule".to_string(),
        "add".to_string(),
        "fwmark".to_string(),
        fwmark.to_string(),
        "lookup".to_string(),
        table.to_string(),
        "priority".to_string(),
        priority.to_string(),
    ]
}

/// Generate `ip rule del` arguments to remove a rule by priority.
///
/// Returns args to pass after `ip`: `["rule", "del", "priority", ...]`
pub fn fwmark_ip_rule_del_args(priority: u32) -> Vec<String> {
    vec![
        "rule".to_string(),
        "del".to_string(),
        "priority".to_string(),
        priority.to_string(),
    ]
}

// ── Validation ────────────────────────────────────────────────────────

/// Validate an IP address or CIDR notation string (IPv4 only).
fn validate_ip_or_cidr(s: &str) -> Result<(), String> {
    if let Some(slash_pos) = s.find('/') {
        let addr_part = &s[..slash_pos];
        let prefix_part = &s[slash_pos + 1..];

        addr_part
            .parse::<IpAddr>()
            .map_err(|_| format!("Invalid IP address: {addr_part}"))?;

        let prefix: u32 = prefix_part
            .parse()
            .map_err(|_| format!("Invalid CIDR prefix: {prefix_part}"))?;

        if prefix > 32 {
            return Err(format!("CIDR prefix must be 0-32, got {prefix}"));
        }
    } else {
        s.parse::<IpAddr>()
            .map_err(|_| format!("Invalid IP address: {s}"))?;
    }
    Ok(())
}

/// Validate a port match value.
fn validate_port(port: &PortMatch) -> Result<(), String> {
    match port {
        PortMatch::Single(p) => {
            if *p == 0 {
                return Err("Port value must be 1-65535".to_string());
            }
        }
        PortMatch::Range(lo, hi) => {
            if *lo == 0 || *hi == 0 {
                return Err("Port value must be 1-65535".to_string());
            }
            if lo >= hi {
                return Err(format!(
                    "Port range start ({lo}) must be less than end ({hi})"
                ));
            }
        }
    }
    Ok(())
}

/// Validate a steering rule against the available WAN interfaces.
pub fn validate_rule(rule: &SteeringRule, wan_ids: &[String]) -> Result<(), String> {
    // Name must be non-empty (trimmed)
    if rule.name.trim().is_empty() {
        return Err("Rule name must not be empty".to_string());
    }

    // target_wan must exist in wan_ids
    if !wan_ids.contains(&rule.target_wan) {
        return Err(format!(
            "Target WAN '{}' does not exist",
            rule.target_wan
        ));
    }

    // PreferredFallback requires fallback_wan
    if rule.failover_mode == FailoverMode::PreferredFallback {
        match &rule.fallback_wan {
            None => {
                return Err(
                    "PreferredFallback mode requires a fallback_wan".to_string(),
                );
            }
            Some(fb) => {
                if !wan_ids.contains(fb) {
                    return Err(format!("Fallback WAN '{fb}' does not exist"));
                }
                if fb == &rule.target_wan {
                    return Err(
                        "Fallback WAN must differ from target WAN".to_string(),
                    );
                }
            }
        }
    }

    // Port requires protocol
    let has_port = rule.destination_port.is_some() || rule.source_port.is_some();
    if has_port && rule.protocol.is_none() {
        return Err("Port matching requires a protocol to be set".to_string());
    }

    // ICMP cannot have ports
    if rule.protocol == Some(Protocol::Icmp) && has_port {
        return Err("ICMP protocol cannot have port matching".to_string());
    }

    // Validate port values
    if let Some(ref port) = rule.source_port {
        validate_port(port)?;
    }
    if let Some(ref port) = rule.destination_port {
        validate_port(port)?;
    }

    // Validate IP addresses
    if let Some(ref ips) = rule.source_ip {
        for ip in ips {
            validate_ip_or_cidr(ip)?;
        }
    }
    if let Some(ref ips) = rule.destination_ip {
        for ip in ips {
            validate_ip_or_cidr(ip)?;
        }
    }

    Ok(())
}

// ── Command Execution Helpers ─────────────────────────────────────────

/// Run an `ip` command with owned String args, returning stdout on success.
fn run_ip_cmd(args: &[String]) -> Result<String, String> {
    let str_args: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let output = Command::new("ip")
        .args(&str_args)
        .output()
        .map_err(|e| format!("Failed to execute ip {}: {e}", str_args.join(" ")))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!(
            "ip {} failed: {}",
            str_args.join(" "),
            stderr.trim()
        ))
    }
}

/// Run an `nft` command, splitting the command string on whitespace.
fn run_nft_cmd(cmd: &str) -> Result<String, String> {
    let args: Vec<&str> = cmd.split_whitespace().collect();
    let output = Command::new("nft")
        .args(&args)
        .output()
        .map_err(|e| format!("Failed to execute nft {cmd}: {e}"))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("nft {cmd} failed: {}", stderr.trim()))
    }
}

/// Run an `iptables` command with the given arguments.
fn run_iptables_cmd(args: &[&str]) -> Result<String, String> {
    let output = Command::new("iptables")
        .args(args)
        .output()
        .map_err(|e| format!("Failed to execute iptables {}: {e}", args.join(" ")))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!(
            "iptables {} failed: {}",
            args.join(" "),
            stderr.trim()
        ))
    }
}

// ── Rule Application ──────────────────────────────────────────────────

/// Apply a steering rule to the system using the appropriate firewall backend.
///
/// Creates the firewall mark rule and the corresponding ip rule for
/// policy routing to the given table.
pub fn apply_rule(
    rule: &SteeringRule,
    target_table: u32,
    fw_backend: &FirewallBackend,
) -> Result<(), String> {
    match fw_backend {
        FirewallBackend::Fw4 => {
            let expressions = generate_nft_rule(rule);
            for expr in &expressions {
                run_nft_cmd(&format!(
                    "add rule {NFT_TABLE} {NFT_CHAIN} {expr}"
                ))?;
            }
        }
        FirewallBackend::Fw3 => {
            let rule_sets = generate_iptables_rules(rule);
            for args in &rule_sets {
                let mut full_args = vec!["-t", "mangle", "-A", IPT_CHAIN];
                let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
                full_args.extend(arg_refs);
                run_iptables_cmd(&full_args)?;
            }
        }
        FirewallBackend::Unknown => {
            return Err("Cannot apply steering rule: unknown firewall backend".to_string());
        }
    }

    // Create ip rule for fwmark → table lookup
    let ip_args = fwmark_ip_rule_add_args(rule.fwmark, target_table, rule.priority);
    run_ip_cmd(&ip_args)?;

    Ok(())
}

/// Remove an ip rule by its priority number.
pub fn remove_ip_rule(priority: u32) -> Result<(), String> {
    let args = fwmark_ip_rule_del_args(priority);
    run_ip_cmd(&args)
        .map(|_| ())
}

// ── Chain Management ──────────────────────────────────────────────────

/// Create the steering firewall chain for the detected backend.
pub fn create_steering_chain(fw_backend: &FirewallBackend) -> Result<(), String> {
    match fw_backend {
        FirewallBackend::Fw4 => {
            run_nft_cmd("add table inet ctrl_wan")?;
            // The semicolon and braces need to be passed as a single nft command string
            // but run_nft_cmd splits on whitespace. Use Command directly for this.
            let output = Command::new("nft")
                .args([
                    "add", "chain", "inet", "ctrl_wan", "steering",
                    "{", "type", "filter", "hook", "prerouting", "priority", "-150", ";", "}",
                ])
                .output()
                .map_err(|e| format!("Failed to create nft chain: {e}"))?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(format!("nft add chain failed: {}", stderr.trim()));
            }
            Ok(())
        }
        FirewallBackend::Fw3 => {
            run_iptables_cmd(&["-t", "mangle", "-N", IPT_CHAIN])?;
            run_iptables_cmd(&[
                "-t", "mangle", "-A", "PREROUTING", "-j", IPT_CHAIN,
            ])?;
            Ok(())
        }
        FirewallBackend::Unknown => {
            Err("Cannot create steering chain: unknown firewall backend".to_string())
        }
    }
}

/// Flush all steering rules and remove the chain. Errors are ignored
/// (best-effort cleanup for crash recovery / clean-slate startup).
pub fn flush_steering(fw_backend: &FirewallBackend) {
    match fw_backend {
        FirewallBackend::Fw4 => {
            let _ = run_nft_cmd(&format!("flush chain {NFT_TABLE} {NFT_CHAIN}"));
            let _ = run_nft_cmd(&format!("delete chain {NFT_TABLE} {NFT_CHAIN}"));
            let _ = run_nft_cmd("delete table inet ctrl_wan");
        }
        FirewallBackend::Fw3 => {
            let _ = run_iptables_cmd(&[
                "-t", "mangle", "-D", "PREROUTING", "-j", IPT_CHAIN,
            ]);
            let _ = run_iptables_cmd(&["-t", "mangle", "-F", IPT_CHAIN]);
            let _ = run_iptables_cmd(&["-t", "mangle", "-X", IPT_CHAIN]);
        }
        FirewallBackend::Unknown => {}
    }

    // Flush all fwmark ip rules in steering priority range 900-949
    for priority in STEERING_PRIORITY_BASE..STEERING_PRIORITY_BASE + STEERING_MAX_RULES {
        let _ = remove_ip_rule(priority);
    }
}

// ── Initialization ────────────────────────────────────────────────────

/// Initialize steering rules from disk configuration.
///
/// Loads rules, creates the firewall chain, and applies each enabled rule.
/// Rules whose target WAN is not present in routing_state are marked Dormant.
/// Returns the rules with updated status and fwmark values.
pub fn initialize(
    config_path: &str,
    fw_backend: &FirewallBackend,
    routing_state: &HashMap<String, RoutingTableEntry>,
) -> Vec<SteeringRule> {
    let mut rules = load_rules(config_path);

    if rules.is_empty() {
        info!("No steering rules to initialize");
        return rules;
    }

    if *fw_backend == FirewallBackend::Unknown {
        warn!("Unknown firewall backend — steering rules will not be applied");
        for rule in &mut rules {
            rule.status = RuleStatus::Dormant;
        }
        return rules;
    }

    // Clean slate
    flush_steering(fw_backend);

    // Assign priorities and fwmarks
    assign_priorities(&mut rules);

    // Create the chain
    if let Err(e) = create_steering_chain(fw_backend) {
        warn!("Failed to create steering chain: {e}");
        for rule in &mut rules {
            rule.status = RuleStatus::Dormant;
        }
        return rules;
    }

    // Apply each enabled rule
    for rule in &mut rules {
        if !rule.enabled {
            rule.status = RuleStatus::Blocked;
            continue;
        }

        // Look up the target WAN's routing table
        match routing_state.get(&rule.target_wan) {
            Some(entry) => {
                match apply_rule(rule, entry.table_number, fw_backend) {
                    Ok(()) => {
                        rule.status = RuleStatus::Active;
                        info!(
                            "Steering rule '{}' applied: fwmark {} → table {}",
                            rule.name, rule.fwmark, entry.table_number
                        );
                    }
                    Err(e) => {
                        warn!(
                            "Failed to apply steering rule '{}': {e}",
                            rule.name
                        );
                        rule.status = RuleStatus::Dormant;
                    }
                }
            }
            None => {
                warn!(
                    "Steering rule '{}' target WAN '{}' not in routing state — marking dormant",
                    rule.name, rule.target_wan
                );
                rule.status = RuleStatus::Dormant;
            }
        }
    }

    info!(
        "Initialized {} steering rules ({} active)",
        rules.len(),
        rules.iter().filter(|r| r.status == RuleStatus::Active).count()
    );

    rules
}

// ── Watchdog Reconciliation ───────────────────────────────────────────

/// Reconcile steering rule statuses based on WAN health and routing state.
///
/// For each enabled rule, checks whether its target WAN is healthy and has a
/// routing table. Updates the rule's status and adjusts ip rules accordingly.
/// Returns a list of human-readable status change descriptions for logging.
pub fn reconcile_statuses(
    rules: &mut [SteeringRule],
    routing_state: &HashMap<String, RoutingTableEntry>,
    healthy_wans: &[String],
    _fw_backend: &FirewallBackend,
) -> Vec<String> {
    let mut changes = Vec::new();

    for rule in rules.iter_mut() {
        if !rule.enabled {
            continue;
        }

        let target_healthy = healthy_wans.contains(&rule.target_wan)
            && routing_state.contains_key(&rule.target_wan);

        let new_status = if target_healthy {
            RuleStatus::Active
        } else {
            match rule.failover_mode {
                FailoverMode::Automatic => RuleStatus::Dormant,
                FailoverMode::PreferredFallback => RuleStatus::Dormant,
                FailoverMode::Strict => RuleStatus::Blocked,
            }
        };

        if new_status == rule.status {
            continue;
        }

        let old_status = rule.status.clone();

        // Take action based on failover mode and transition
        match (&rule.failover_mode, &new_status) {
            (FailoverMode::Automatic, RuleStatus::Dormant) => {
                // Remove ip rule so traffic falls through to default route
                if let Err(e) = remove_ip_rule(rule.priority) {
                    warn!(
                        "Failed to remove ip rule for dormant steering rule '{}': {e}",
                        rule.name
                    );
                }
            }
            (FailoverMode::Automatic, RuleStatus::Active) => {
                // Recreate ip rule pointing to target WAN's table
                if let Some(entry) = routing_state.get(&rule.target_wan) {
                    let args =
                        fwmark_ip_rule_add_args(rule.fwmark, entry.table_number, rule.priority);
                    if let Err(e) = run_ip_cmd(&args) {
                        warn!(
                            "Failed to recreate ip rule for active steering rule '{}': {e}",
                            rule.name
                        );
                    }
                }
            }
            (FailoverMode::PreferredFallback, RuleStatus::Dormant) => {
                // Remove original ip rule
                let _ = remove_ip_rule(rule.priority);

                // Add new one pointing to fallback WAN's table if available
                if let Some(ref fallback_id) = rule.fallback_wan {
                    if let Some(fb_entry) = routing_state.get(fallback_id) {
                        let args = fwmark_ip_rule_add_args(
                            rule.fwmark,
                            fb_entry.table_number,
                            rule.priority,
                        );
                        if let Err(e) = run_ip_cmd(&args) {
                            warn!(
                                "Failed to add fallback ip rule for steering rule '{}': {e}",
                                rule.name
                            );
                        }
                    }
                }
            }
            (FailoverMode::PreferredFallback, RuleStatus::Active) => {
                // Remove fallback ip rule, restore to original target WAN's table
                let _ = remove_ip_rule(rule.priority);

                if let Some(entry) = routing_state.get(&rule.target_wan) {
                    let args =
                        fwmark_ip_rule_add_args(rule.fwmark, entry.table_number, rule.priority);
                    if let Err(e) = run_ip_cmd(&args) {
                        warn!(
                            "Failed to restore ip rule for steering rule '{}': {e}",
                            rule.name
                        );
                    }
                }
            }
            (FailoverMode::Strict, _) => {
                // No ip rule changes — intentional blackhole when target is down
            }
            _ => {}
        }

        rule.status = new_status.clone();

        let msg = format!(
            "Steering rule '{}': {old_status:?} -> {new_status:?}",
            rule.name
        );
        info!("{msg}");
        changes.push(msg);
    }

    changes
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_rule(id: &str, name: &str, target_wan: &str) -> SteeringRule {
        SteeringRule {
            id: id.to_string(),
            name: name.to_string(),
            enabled: true,
            priority: 0,
            source_ip: None,
            destination_ip: None,
            protocol: None,
            destination_port: None,
            source_port: None,
            target_wan: target_wan.to_string(),
            failover_mode: FailoverMode::Automatic,
            fallback_wan: None,
            status: RuleStatus::Active,
            fwmark: 0,
        }
    }

    #[test]
    fn test_serialization_round_trip() {
        let rule = SteeringRule {
            id: "rule-1".to_string(),
            name: "VoIP Traffic".to_string(),
            enabled: true,
            priority: 900,
            source_ip: Some(vec!["192.168.1.0/24".to_string()]),
            destination_ip: None,
            protocol: Some(Protocol::Udp),
            destination_port: Some(PortMatch::Range(5060, 5061)),
            source_port: Some(PortMatch::Single(443)),
            target_wan: "wwan0".to_string(),
            failover_mode: FailoverMode::PreferredFallback,
            fallback_wan: Some("wwan1".to_string()),
            status: RuleStatus::Dormant,
            fwmark: 101,
        };

        let json = serde_json::to_string_pretty(&rule).unwrap();
        let deserialized: SteeringRule = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.id, "rule-1");
        assert_eq!(deserialized.name, "VoIP Traffic");
        assert!(deserialized.enabled);
        assert_eq!(deserialized.priority, 900);
        assert_eq!(deserialized.source_ip, Some(vec!["192.168.1.0/24".to_string()]));
        assert!(deserialized.destination_ip.is_none());
        assert_eq!(deserialized.protocol, Some(Protocol::Udp));
        assert_eq!(deserialized.destination_port, Some(PortMatch::Range(5060, 5061)));
        assert_eq!(deserialized.source_port, Some(PortMatch::Single(443)));
        assert_eq!(deserialized.target_wan, "wwan0");
        assert_eq!(deserialized.failover_mode, FailoverMode::PreferredFallback);
        assert_eq!(deserialized.fallback_wan.as_deref(), Some("wwan1"));
        assert_eq!(deserialized.status, RuleStatus::Dormant);
        assert_eq!(deserialized.fwmark, 101);
    }

    #[test]
    fn test_protocol_serde_lowercase() {
        let json = serde_json::to_string(&Protocol::Tcp).unwrap();
        assert_eq!(json, "\"tcp\"");

        let json = serde_json::to_string(&Protocol::Udp).unwrap();
        assert_eq!(json, "\"udp\"");

        let json = serde_json::to_string(&Protocol::Icmp).unwrap();
        assert_eq!(json, "\"icmp\"");

        let parsed: Protocol = serde_json::from_str("\"tcp\"").unwrap();
        assert_eq!(parsed, Protocol::Tcp);
    }

    #[test]
    fn test_failover_mode_serde_snake_case() {
        let json = serde_json::to_string(&FailoverMode::PreferredFallback).unwrap();
        assert_eq!(json, "\"preferred_fallback\"");

        let parsed: FailoverMode = serde_json::from_str("\"strict\"").unwrap();
        assert_eq!(parsed, FailoverMode::Strict);
    }

    #[test]
    fn test_port_match_untagged() {
        // Single port serializes as a bare number
        let json = serde_json::to_string(&PortMatch::Single(443)).unwrap();
        assert_eq!(json, "443");

        // Range serializes as a two-element array
        let json = serde_json::to_string(&PortMatch::Range(8000, 9000)).unwrap();
        assert_eq!(json, "[8000,9000]");

        // Deserialize single
        let parsed: PortMatch = serde_json::from_str("80").unwrap();
        assert_eq!(parsed, PortMatch::Single(80));

        // Deserialize range
        let parsed: PortMatch = serde_json::from_str("[5060,5061]").unwrap();
        assert_eq!(parsed, PortMatch::Range(5060, 5061));
    }

    #[test]
    fn test_defaults_when_fields_missing() {
        let json = r#"{
            "id": "rule-1",
            "name": "Test",
            "enabled": true,
            "source_ip": null,
            "destination_ip": null,
            "protocol": null,
            "destination_port": null,
            "source_port": null,
            "target_wan": "wwan0",
            "fallback_wan": null
        }"#;

        let rule: SteeringRule = serde_json::from_str(json).unwrap();
        assert_eq!(rule.priority, 0);
        assert_eq!(rule.failover_mode, FailoverMode::Automatic);
        assert_eq!(rule.status, RuleStatus::Active);
        assert_eq!(rule.fwmark, 0);
    }

    #[test]
    fn test_load_save_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("steering-rules.json");
        let path_str = path.to_str().unwrap();

        // Save two rules
        let rules = vec![
            SteeringRule {
                status: RuleStatus::Dormant,
                fwmark: 999,
                ..make_test_rule("r1", "Gaming", "wwan0")
            },
            SteeringRule {
                protocol: Some(Protocol::Tcp),
                destination_port: Some(PortMatch::Single(443)),
                failover_mode: FailoverMode::Strict,
                ..make_test_rule("r2", "HTTPS Only", "wwan1")
            },
        ];

        save_rules(path_str, &rules).unwrap();

        // Load them back
        let loaded = load_rules(path_str);
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].id, "r1");
        assert_eq!(loaded[0].name, "Gaming");
        // Runtime fields should be reset to defaults after save
        assert_eq!(loaded[0].status, RuleStatus::Active);
        assert_eq!(loaded[0].fwmark, 0);

        assert_eq!(loaded[1].id, "r2");
        assert_eq!(loaded[1].protocol, Some(Protocol::Tcp));
        assert_eq!(loaded[1].failover_mode, FailoverMode::Strict);
    }

    #[test]
    fn test_load_missing_file_returns_empty() {
        let rules = load_rules("/tmp/nonexistent-steering-rules-test.json");
        assert!(rules.is_empty());
    }

    #[test]
    fn test_save_creates_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("deep").join("rules.json");
        let path_str = path.to_str().unwrap();

        save_rules(path_str, &[make_test_rule("r1", "Test", "wwan0")]).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn test_assign_priorities() {
        let mut rules = vec![
            make_test_rule("a", "First", "wwan0"),
            make_test_rule("b", "Second", "wwan1"),
            make_test_rule("c", "Third", "wwan0"),
        ];

        assign_priorities(&mut rules);

        assert_eq!(rules[0].priority, 900);
        assert_eq!(rules[0].fwmark, 100);
        assert_eq!(rules[1].priority, 901);
        assert_eq!(rules[1].fwmark, 101);
        assert_eq!(rules[2].priority, 902);
        assert_eq!(rules[2].fwmark, 102);
    }

    #[test]
    fn test_assign_priorities_empty() {
        let mut rules: Vec<SteeringRule> = vec![];
        assign_priorities(&mut rules);
        assert!(rules.is_empty());
    }

    // ── Firewall rule generation tests ────────────────────────────────

    #[test]
    fn test_nftables_rule_generation_full() {
        let rule = SteeringRule {
            source_ip: Some(vec!["192.168.1.0/24".to_string()]),
            destination_ip: Some(vec!["10.0.0.1".to_string()]),
            protocol: Some(Protocol::Udp),
            source_port: Some(PortMatch::Single(12345)),
            destination_port: Some(PortMatch::Range(5060, 5080)),
            fwmark: 100,
            ..make_test_rule("r1", "VoIP", "wwan0")
        };

        let result = generate_nft_rule(&rule);
        assert_eq!(result.len(), 1);
        assert_eq!(
            result[0],
            "ip saddr 192.168.1.0/24 ip daddr 10.0.0.1 meta l4proto udp udp sport 12345 udp dport 5060-5080 mark set 0x64"
        );
    }

    #[test]
    fn test_nftables_rule_generation_minimal() {
        let rule = SteeringRule {
            fwmark: 100,
            ..make_test_rule("r1", "Catch All", "wwan0")
        };

        let result = generate_nft_rule(&rule);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], "mark set 0x64");
    }

    #[test]
    fn test_iptables_rule_generation_full() {
        let rule = SteeringRule {
            source_ip: Some(vec!["192.168.1.0/24".to_string()]),
            destination_ip: Some(vec!["10.0.0.1".to_string()]),
            protocol: Some(Protocol::Udp),
            source_port: Some(PortMatch::Single(12345)),
            destination_port: Some(PortMatch::Range(5060, 5080)),
            fwmark: 100,
            ..make_test_rule("r1", "VoIP", "wwan0")
        };

        let result = generate_iptables_rules(&rule);
        assert_eq!(result.len(), 1);
        assert_eq!(
            result[0],
            vec![
                "-s", "192.168.1.0/24",
                "-d", "10.0.0.1",
                "-p", "udp",
                "--sport", "12345",
                "--dport", "5060:5080",
                "-j", "MARK",
                "--set-mark", "100",
            ]
        );
    }

    #[test]
    fn test_iptables_rule_generation_single_port() {
        let rule = SteeringRule {
            protocol: Some(Protocol::Tcp),
            destination_port: Some(PortMatch::Single(443)),
            fwmark: 101,
            ..make_test_rule("r2", "HTTPS", "wwan1")
        };

        let result = generate_iptables_rules(&rule);
        assert_eq!(result.len(), 1);
        assert_eq!(
            result[0],
            vec![
                "-p", "tcp",
                "--dport", "443",
                "-j", "MARK",
                "--set-mark", "101",
            ]
        );
    }

    // ── Validation tests ───────────────────────────────────────────────

    fn test_wan_ids() -> Vec<String> {
        vec!["wwan0".to_string(), "wwan1".to_string(), "eth0".to_string()]
    }

    #[test]
    fn test_validate_rule_valid() {
        let rule = SteeringRule {
            source_ip: Some(vec!["192.168.1.0/24".to_string()]),
            destination_ip: Some(vec!["10.0.0.1".to_string()]),
            protocol: Some(Protocol::Tcp),
            destination_port: Some(PortMatch::Range(80, 443)),
            source_port: Some(PortMatch::Single(12345)),
            failover_mode: FailoverMode::PreferredFallback,
            fallback_wan: Some("wwan1".to_string()),
            ..make_test_rule("r1", "Full Rule", "wwan0")
        };
        assert!(validate_rule(&rule, &test_wan_ids()).is_ok());
    }

    #[test]
    fn test_validate_rule_empty_name() {
        let rule = make_test_rule("r1", "  ", "wwan0");
        let result = validate_rule(&rule, &test_wan_ids());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("name"));
    }

    #[test]
    fn test_validate_rule_target_wan_missing() {
        let rule = make_test_rule("r1", "Test", "wwan99");
        let result = validate_rule(&rule, &test_wan_ids());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("wwan99"));
    }

    #[test]
    fn test_validate_rule_port_without_protocol() {
        let rule = SteeringRule {
            destination_port: Some(PortMatch::Single(80)),
            ..make_test_rule("r1", "No Proto", "wwan0")
        };
        let result = validate_rule(&rule, &test_wan_ids());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("protocol"));
    }

    #[test]
    fn test_validate_rule_fallback_required_for_preferred() {
        let rule = SteeringRule {
            failover_mode: FailoverMode::PreferredFallback,
            fallback_wan: None,
            ..make_test_rule("r1", "No Fallback", "wwan0")
        };
        let result = validate_rule(&rule, &test_wan_ids());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("fallback_wan"));
    }

    #[test]
    fn test_validate_rule_fallback_same_as_target() {
        let rule = SteeringRule {
            failover_mode: FailoverMode::PreferredFallback,
            fallback_wan: Some("wwan0".to_string()),
            ..make_test_rule("r1", "Same WAN", "wwan0")
        };
        let result = validate_rule(&rule, &test_wan_ids());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("differ"));
    }

    #[test]
    fn test_validate_rule_invalid_port_range() {
        let rule = SteeringRule {
            protocol: Some(Protocol::Tcp),
            destination_port: Some(PortMatch::Range(443, 80)),
            ..make_test_rule("r1", "Bad Range", "wwan0")
        };
        let result = validate_rule(&rule, &test_wan_ids());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("less than"));
    }

    #[test]
    fn test_validate_rule_icmp_with_ports() {
        let rule = SteeringRule {
            protocol: Some(Protocol::Icmp),
            destination_port: Some(PortMatch::Single(80)),
            ..make_test_rule("r1", "ICMP Port", "wwan0")
        };
        let result = validate_rule(&rule, &test_wan_ids());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("ICMP"));
    }

    #[test]
    fn test_validate_rule_invalid_ip() {
        let rule = SteeringRule {
            source_ip: Some(vec!["not-an-ip".to_string()]),
            ..make_test_rule("r1", "Bad IP", "wwan0")
        };
        let result = validate_rule(&rule, &test_wan_ids());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid IP"));
    }

    #[test]
    fn test_validate_rule_invalid_cidr_prefix() {
        let rule = SteeringRule {
            source_ip: Some(vec!["192.168.1.0/33".to_string()]),
            ..make_test_rule("r1", "Bad CIDR", "wwan0")
        };
        let result = validate_rule(&rule, &test_wan_ids());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("CIDR prefix"));
    }

    #[test]
    fn test_validate_rule_zero_port() {
        let rule = SteeringRule {
            protocol: Some(Protocol::Tcp),
            destination_port: Some(PortMatch::Single(0)),
            ..make_test_rule("r1", "Zero Port", "wwan0")
        };
        let result = validate_rule(&rule, &test_wan_ids());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("1-65535"));
    }

    #[test]
    fn test_fwmark_ip_rule_args() {
        let add_args = fwmark_ip_rule_add_args(100, 101, 900);
        assert_eq!(
            add_args,
            vec!["rule", "add", "fwmark", "100", "lookup", "101", "priority", "900"]
        );

        let del_args = fwmark_ip_rule_del_args(900);
        assert_eq!(
            del_args,
            vec!["rule", "del", "priority", "900"]
        );
    }

    // ── Reconciliation tests ──────────────────────────────────────────

    fn make_routing_state() -> HashMap<String, RoutingTableEntry> {
        let mut rs = HashMap::new();
        rs.insert(
            "wwan0".to_string(),
            RoutingTableEntry {
                table_number: 100,
                rule_priority: 1000,
                gateway: Some("10.0.0.1".to_string()),
                device: "usb0".to_string(),
                source_ip: "10.0.0.2".to_string(),
            },
        );
        rs.insert(
            "wwan1".to_string(),
            RoutingTableEntry {
                table_number: 101,
                rule_priority: 1001,
                gateway: Some("10.0.1.1".to_string()),
                device: "usb1".to_string(),
                source_ip: "10.0.1.2".to_string(),
            },
        );
        rs
    }

    #[test]
    fn test_reconcile_statuses_no_change_when_healthy() {
        let rs = make_routing_state();
        let healthy = vec!["wwan0".to_string(), "wwan1".to_string()];
        let mut rules = vec![SteeringRule {
            priority: 900,
            fwmark: 100,
            status: RuleStatus::Active,
            ..make_test_rule("r1", "Test", "wwan0")
        }];

        let changes = reconcile_statuses(&mut rules, &rs, &healthy, &FirewallBackend::Unknown);
        assert!(changes.is_empty());
        assert_eq!(rules[0].status, RuleStatus::Active);
    }

    #[test]
    fn test_reconcile_statuses_automatic_goes_dormant() {
        let rs = make_routing_state();
        let healthy = vec!["wwan1".to_string()]; // wwan0 is down
        let mut rules = vec![SteeringRule {
            priority: 900,
            fwmark: 100,
            status: RuleStatus::Active,
            failover_mode: FailoverMode::Automatic,
            ..make_test_rule("r1", "Auto Rule", "wwan0")
        }];

        // run_ip_cmd will fail on non-Linux, but the status should still update
        let changes = reconcile_statuses(&mut rules, &rs, &healthy, &FirewallBackend::Unknown);
        assert_eq!(changes.len(), 1);
        assert_eq!(rules[0].status, RuleStatus::Dormant);
        assert!(changes[0].contains("Auto Rule"));
    }

    #[test]
    fn test_reconcile_statuses_strict_goes_blocked() {
        let rs = make_routing_state();
        let healthy = vec!["wwan1".to_string()]; // wwan0 is down
        let mut rules = vec![SteeringRule {
            priority: 900,
            fwmark: 100,
            status: RuleStatus::Active,
            failover_mode: FailoverMode::Strict,
            ..make_test_rule("r1", "Strict Rule", "wwan0")
        }];

        let changes = reconcile_statuses(&mut rules, &rs, &healthy, &FirewallBackend::Unknown);
        assert_eq!(changes.len(), 1);
        assert_eq!(rules[0].status, RuleStatus::Blocked);
    }

    #[test]
    fn test_reconcile_statuses_preferred_fallback_goes_dormant() {
        let rs = make_routing_state();
        let healthy = vec!["wwan1".to_string()]; // wwan0 is down
        let mut rules = vec![SteeringRule {
            priority: 900,
            fwmark: 100,
            status: RuleStatus::Active,
            failover_mode: FailoverMode::PreferredFallback,
            fallback_wan: Some("wwan1".to_string()),
            ..make_test_rule("r1", "FB Rule", "wwan0")
        }];

        let changes = reconcile_statuses(&mut rules, &rs, &healthy, &FirewallBackend::Unknown);
        assert_eq!(changes.len(), 1);
        assert_eq!(rules[0].status, RuleStatus::Dormant);
    }

    #[test]
    fn test_reconcile_statuses_skips_disabled() {
        let rs = make_routing_state();
        let healthy: Vec<String> = vec![]; // all down
        let mut rules = vec![SteeringRule {
            priority: 900,
            fwmark: 100,
            enabled: false,
            status: RuleStatus::Active,
            ..make_test_rule("r1", "Disabled", "wwan0")
        }];

        let changes = reconcile_statuses(&mut rules, &rs, &healthy, &FirewallBackend::Unknown);
        assert!(changes.is_empty());
        // Status unchanged because rule is disabled
        assert_eq!(rules[0].status, RuleStatus::Active);
    }

    // ── Multi-IP tests ─────────────────────────────────────────────────

    #[test]
    fn test_nftables_rule_generation_multi_source_ip() {
        let rule = SteeringRule {
            source_ip: Some(vec![
                "192.168.1.0/24".to_string(),
                "10.0.0.0/8".to_string(),
            ]),
            fwmark: 100,
            ..make_test_rule("r1", "Multi Source", "wwan0")
        };

        let result = generate_nft_rule(&rule);
        assert_eq!(result.len(), 1);
        assert_eq!(
            result[0],
            "ip saddr { 192.168.1.0/24, 10.0.0.0/8 } mark set 0x64"
        );
    }

    #[test]
    fn test_nftables_rule_generation_multi_dest_ip() {
        let rule = SteeringRule {
            destination_ip: Some(vec![
                "1.2.3.0/24".to_string(),
                "5.6.7.0/24".to_string(),
            ]),
            fwmark: 101,
            ..make_test_rule("r1", "Multi Dest", "wwan0")
        };

        let result = generate_nft_rule(&rule);
        assert_eq!(result.len(), 1);
        assert_eq!(
            result[0],
            "ip daddr { 1.2.3.0/24, 5.6.7.0/24 } mark set 0x65"
        );
    }

    #[test]
    fn test_iptables_rule_generation_multi_dest_ip() {
        let rule = SteeringRule {
            destination_ip: Some(vec![
                "1.2.3.0/24".to_string(),
                "5.6.7.0/24".to_string(),
            ]),
            fwmark: 100,
            ..make_test_rule("r1", "Multi Dest", "wwan0")
        };

        let result = generate_iptables_rules(&rule);
        assert_eq!(result.len(), 2);
        assert_eq!(
            result[0],
            vec!["-d", "1.2.3.0/24", "-j", "MARK", "--set-mark", "100"]
        );
        assert_eq!(
            result[1],
            vec!["-d", "5.6.7.0/24", "-j", "MARK", "--set-mark", "100"]
        );
    }

    #[test]
    fn test_iptables_rule_generation_multi_source_and_dest() {
        let rule = SteeringRule {
            source_ip: Some(vec![
                "192.168.1.0/24".to_string(),
                "10.0.0.0/8".to_string(),
            ]),
            destination_ip: Some(vec![
                "1.2.3.0/24".to_string(),
                "5.6.7.0/24".to_string(),
            ]),
            fwmark: 100,
            ..make_test_rule("r1", "Multi Both", "wwan0")
        };

        let result = generate_iptables_rules(&rule);
        // 2 sources x 2 destinations = 4 rules
        assert_eq!(result.len(), 4);
        assert_eq!(
            result[0],
            vec!["-s", "192.168.1.0/24", "-d", "1.2.3.0/24", "-j", "MARK", "--set-mark", "100"]
        );
        assert_eq!(
            result[1],
            vec!["-s", "192.168.1.0/24", "-d", "5.6.7.0/24", "-j", "MARK", "--set-mark", "100"]
        );
        assert_eq!(
            result[2],
            vec!["-s", "10.0.0.0/8", "-d", "1.2.3.0/24", "-j", "MARK", "--set-mark", "100"]
        );
        assert_eq!(
            result[3],
            vec!["-s", "10.0.0.0/8", "-d", "5.6.7.0/24", "-j", "MARK", "--set-mark", "100"]
        );
    }

    #[test]
    fn test_validate_rule_multi_ip_valid() {
        let rule = SteeringRule {
            source_ip: Some(vec![
                "192.168.1.0/24".to_string(),
                "10.0.0.0/8".to_string(),
            ]),
            destination_ip: Some(vec!["1.2.3.4".to_string()]),
            ..make_test_rule("r1", "Multi Valid", "wwan0")
        };
        assert!(validate_rule(&rule, &test_wan_ids()).is_ok());
    }

    #[test]
    fn test_validate_rule_multi_ip_one_invalid() {
        let rule = SteeringRule {
            source_ip: Some(vec![
                "192.168.1.0/24".to_string(),
                "not-valid".to_string(),
            ]),
            ..make_test_rule("r1", "Multi Invalid", "wwan0")
        };
        let result = validate_rule(&rule, &test_wan_ids());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid IP"));
    }

    #[test]
    fn test_reconcile_statuses_target_not_in_routing_state() {
        let rs = HashMap::new(); // empty routing state
        let healthy = vec!["wwan0".to_string()];
        let mut rules = vec![SteeringRule {
            priority: 900,
            fwmark: 100,
            status: RuleStatus::Active,
            ..make_test_rule("r1", "No Route", "wwan0")
        }];

        let changes = reconcile_statuses(&mut rules, &rs, &healthy, &FirewallBackend::Unknown);
        assert_eq!(changes.len(), 1);
        assert_eq!(rules[0].status, RuleStatus::Dormant);
    }
}
