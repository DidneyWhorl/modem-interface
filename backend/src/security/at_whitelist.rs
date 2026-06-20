//! AT command whitelist for security.
//!
//! Validates AT commands against an allowlist to prevent dangerous operations.
//! Commands are categorized by risk level:
//! - Safe: Read-only commands allowed without confirmation
//! - RequiresConfirmation: State-changing commands that need user acknowledgment
//! - Blocked: Dangerous commands that are never allowed
//!
//! Three layers of whitelist configuration are merged (highest priority first):
//! 1. Runtime overrides (user-configured, persisted to disk)
//! 2. Modem profile additions (per-model, compiled or loaded from filesystem)
//! 3. Base static whitelist (hardcoded defaults)

use std::collections::{HashMap, HashSet};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};

use crate::hardware::profiles::ProfileAtWhitelist;

/// Command safety classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandSafety {
    /// Safe read-only commands.
    Safe,
    /// Commands that change state but are allowed with confirmation.
    RequiresConfirmation,
    /// Commands that are blocked entirely.
    Blocked,
}

/// Serializable tier name for JSON persistence and API responses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandTier {
    Safe,
    Confirmation,
    Blocked,
}

impl From<CommandSafety> for CommandTier {
    fn from(safety: CommandSafety) -> Self {
        match safety {
            CommandSafety::Safe => CommandTier::Safe,
            CommandSafety::RequiresConfirmation => CommandTier::Confirmation,
            CommandSafety::Blocked => CommandTier::Blocked,
        }
    }
}

impl From<CommandTier> for CommandSafety {
    fn from(tier: CommandTier) -> Self {
        match tier {
            CommandTier::Safe => CommandSafety::Safe,
            CommandTier::Confirmation => CommandSafety::RequiresConfirmation,
            CommandTier::Blocked => CommandSafety::Blocked,
        }
    }
}

/// Result of whitelist validation.
#[derive(Debug, Clone)]
pub struct ValidationResult {
    pub safety: CommandSafety,
    #[allow(dead_code)]
    pub command: String,
    pub reason: Option<String>,
}

/// Runtime overrides to the AT command whitelist.
/// Persisted to /etc/modem-interface/at-whitelist-overrides.json.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WhitelistOverrides {
    /// Additional commands to treat as safe.
    #[serde(default)]
    pub safe_commands: Vec<String>,
    /// Additional commands that require confirmation.
    #[serde(default)]
    pub confirmation_commands: Vec<String>,
    /// Additional command prefixes to block.
    #[serde(default)]
    pub blocked_prefixes: Vec<String>,
    /// Tier overrides: move base/profile commands to a different tier.
    /// Key = command (uppercased), Value = new tier.
    #[serde(default)]
    pub tier_overrides: HashMap<String, CommandTier>,
}

/// Full merged whitelist response for the API.
/// Shape matches docs/API-CONTRACT.md: separate tier arrays + profile metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergedWhitelist {
    pub safe: Vec<String>,
    pub confirmation: Vec<String>,
    pub blocked_prefixes: Vec<String>,
    pub profile_name: String,
    pub profile_label: String,
    pub overrides: WhitelistOverrides,
}

// =============================================================================
// Static whitelist sets
// =============================================================================

/// Commands that are always safe (read-only, no side effects).
/// Only includes standard 3GPP commands. Vendor-specific commands belong in modem profiles.
static SAFE_COMMANDS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    [
        // Basic
        "AT",
        "ATI",
        "ATI0",
        "ATI1",
        "ATI2",
        "ATI3",
        // Signal & Network (read)
        "AT+CSQ",
        "AT+CESQ",
        "AT+COPS?",
        "AT+CREG?",
        "AT+CGREG?",
        "AT+CEREG?",
        "AT+C5GREG?",
        // SIM (read)
        "AT+CPIN?",
        "AT+CCID",
        "AT+CIMI",
        // Configuration (read)
        "AT+CGDCONT?",
        "AT+CGATT?",
        "AT+CGACT?",
        // Service provider name (standard)
        "AT+CSPN?",
        // Sierra specific (read)
        "AT!GSTATUS?",
        "AT!BAND?",
        "AT!SELRAT?",
        // Identification (standard 3GPP)
        "AT+CGMI",
        "AT+CGMM",
        "AT+CGMR",
        "AT+CGSN",
        "AT+GMI",
        "AT+GMM",
        "AT+GMR",
        "AT+GSN",
        // Ericsson
        "AT+EGMR=0,5",
    ]
    .into_iter()
    .collect()
});

/// Commands that require user confirmation (state changes).
/// Only includes standard 3GPP commands. Vendor-specific commands belong in modem profiles.
static CONFIRMATION_COMMANDS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    [
        // Radio/power control
        "AT+CFUN",
        // Network selection
        "AT+COPS",
        // APN configuration
        "AT+CGDCONT",
        // Data connection
        "AT+CGACT",
        "AT+CGDATA",
        // PIN operations
        "AT+CPIN",
        "AT+CLCK",
        // Sierra specific
        "AT!BAND",
        "AT!SELRAT",
    ]
    .into_iter()
    .collect()
});

/// Commands that are never allowed (dangerous).
/// Only includes standard/generic prefixes. Vendor-specific commands belong in modem profiles.
static BLOCKED_PREFIXES: Lazy<Vec<&'static str>> = Lazy::new(|| {
    vec![
        // Sierra specific
        "AT!BOOTHOLD",
        "AT!RESET",
        // Factory reset
        "AT&F",
        "AT!NVRESTORE",
        // NVRAM writes
        "AT!NVWRITE",
        // Unlock codes
        "AT+CLCK",      // Facility lock (blocked, use dedicated PIN endpoints)
        "AT^CARDUNLOCK",
    ]
});

/// Commands that can NEVER be downgraded by a runtime `tier_override`.
///
/// These are the always-block prefixes from `backend/src/security/CLAUDE.md`
/// ("What the AT whitelist must always block (regardless of overrides)") plus
/// the generic destructive prefixes (`AT&F`, `AT!RESET`, `AT+CLCK`). A runtime
/// override that targets any of these is ignored — the command stays Blocked.
///
/// Checked at step 0 of `validate_command_with_context`, BEFORE `tier_overrides`.
/// Matched by `starts_with` against the uppercased command (so `AT+CLCK=...`,
/// `AT+QFASTBOOT`, etc. all match). Comparisons assume entries are uppercase.
static NEVER_OVERRIDABLE: Lazy<Vec<&'static str>> = Lazy::new(|| {
    vec![
        // Qualcomm/Quectel firmware / NVRAM / fastboot set (security/CLAUDE.md)
        "AT+QFASTBOOT",   // Firmware bootloader mode
        "AT+QDOWNLOAD",   // Firmware download mode
        "AT+QPRTPARA",    // Factory reset (parameter restore)
        "AT+QNVW",        // NVRAM write
        "AT+QNVFW",       // NVRAM file write
        "AT+QSIMDET",     // SIM detect hardware config
        "AT$QCPWRDN",     // Qualcomm power down
        "AT$QCDMG",       // Qualcomm DM (diagnostic) mode
        // Generic destructive prefixes
        "AT&F",           // Factory reset
        "AT!RESET",       // Sierra reset
        "AT+CLCK",        // Facility lock (use dedicated PIN endpoints)
    ]
});

/// True iff `cmd` (already uppercased + trimmed) starts with a NEVER_OVERRIDABLE
/// prefix. Such commands are hard-blocked and cannot be moved to another tier
/// via runtime overrides.
fn is_never_overridable(cmd: &str) -> bool {
    NEVER_OVERRIDABLE.iter().any(|prefix| cmd.starts_with(prefix))
}

// =============================================================================
// Validation functions
// =============================================================================

/// True if `command` contains any ASCII control character (CR, LF, NUL, or any
/// other byte < 0x20, plus DEL 0x7f).
///
/// An embedded CR/LF lets an attacker smuggle a second, permanently-blocked AT
/// command past the single-token validator (e.g. `AT+CFUN=1\rAT+QFASTBOOT`).
/// We fail closed on ANY control char here as defense-in-depth; the
/// authoritative split/reject lives in the hardware serial-write path.
fn contains_control_char(command: &str) -> bool {
    command.chars().any(|c| c.is_control())
}

/// Validate an AT command against the base whitelist only (no profile/override context).
pub fn validate_command(command: &str) -> ValidationResult {
    // Fail closed on embedded control characters BEFORE any tier/override
    // matching — a smuggled CR/LF could carry a permanently-blocked command.
    if contains_control_char(command) {
        return ValidationResult {
            safety: CommandSafety::Blocked,
            command: command.to_string(),
            reason: Some(
                "Command contains control characters (CR/LF/control) and is blocked".to_string(),
            ),
        };
    }

    let cmd = command.trim().to_uppercase();

    // Extract base command (before '=' or '?')
    let base_cmd = cmd
        .split(['=', '?'])
        .next()
        .unwrap_or(&cmd);

    // Check blocked first
    for prefix in BLOCKED_PREFIXES.iter() {
        if cmd.starts_with(prefix) {
            return ValidationResult {
                safety: CommandSafety::Blocked,
                command: command.to_string(),
                reason: Some(format!("Command '{prefix}' is blocked for security")),
            };
        }
    }

    // Check if it's a safe read command
    if SAFE_COMMANDS.contains(cmd.as_str()) {
        return ValidationResult {
            safety: CommandSafety::Safe,
            command: command.to_string(),
            reason: None,
        };
    }

    // Check if base command requires confirmation
    for conf_cmd in CONFIRMATION_COMMANDS.iter() {
        if base_cmd.starts_with(conf_cmd) {
            return ValidationResult {
                safety: CommandSafety::RequiresConfirmation,
                command: command.to_string(),
                reason: Some(format!("Command '{conf_cmd}' requires confirmation")),
            };
        }
    }

    // Unknown commands are BLOCKED by default (fail-closed).
    ValidationResult {
        safety: CommandSafety::Blocked,
        command: command.to_string(),
        reason: Some("Unknown command - blocked by default".to_string()),
    }
}

/// Validate a command with full context: base whitelist + profile additions + runtime overrides.
///
/// Priority order (highest first):
/// 0. NEVER_OVERRIDABLE hard-block (cannot be downgraded by any override)
/// 1. Runtime tier_overrides (exact match on full or base command)
/// 2. Blocked: runtime → profile → base
/// 3. Safe: runtime → profile → base
/// 4. Confirmation: runtime → profile → base
/// 5. Unknown → Blocked (fail-closed)
pub fn validate_command_with_context(
    command: &str,
    profile_whitelist: &ProfileAtWhitelist,
    overrides: &WhitelistOverrides,
) -> ValidationResult {
    // -1. Fail closed on embedded control characters BEFORE any tier/override
    //     matching. An attacker can embed CR/LF (`AT+CFUN=1\rAT+QFASTBOOT`) to
    //     smuggle a permanently-blocked command past the single-token validator.
    //     Defense-in-depth: the authoritative fix lives in the hardware
    //     serial-write path; here we reject the whole command outright.
    if contains_control_char(command) {
        return ValidationResult {
            safety: CommandSafety::Blocked,
            command: command.to_string(),
            reason: Some(
                "Command contains control characters (CR/LF/control) and is blocked".to_string(),
            ),
        };
    }

    let cmd = command.trim().to_uppercase();
    let base_cmd = cmd.split(['=', '?']).next().unwrap_or(&cmd);

    // 0. Hard-block the NEVER_OVERRIDABLE set BEFORE consulting tier_overrides.
    //    A runtime override must never be able to downgrade these (security
    //    invariant — backend/src/security/CLAUDE.md).
    if is_never_overridable(&cmd) {
        return ValidationResult {
            safety: CommandSafety::Blocked,
            command: command.to_string(),
            reason: Some("Command is permanently blocked and cannot be overridden".to_string()),
        };
    }

    // 1. Check runtime tier_overrides first (highest priority)
    if let Some(tier) = overrides
        .tier_overrides
        .get(&cmd)
        .or_else(|| overrides.tier_overrides.get(base_cmd))
    {
        return ValidationResult {
            safety: CommandSafety::from(*tier),
            command: command.to_string(),
            reason: match tier {
                CommandTier::Safe => None,
                CommandTier::Confirmation => Some("Custom override: requires confirmation".to_string()),
                CommandTier::Blocked => Some("Custom override: blocked".to_string()),
            },
        };
    }

    // 2. Check blocked prefixes: runtime → profile → base
    for prefix in &overrides.blocked_prefixes {
        if cmd.starts_with(&prefix.to_uppercase()) {
            return ValidationResult {
                safety: CommandSafety::Blocked,
                command: command.to_string(),
                reason: Some(format!("Custom blocked prefix: '{prefix}'")),
            };
        }
    }
    for prefix in &profile_whitelist.blocked_prefixes {
        if cmd.starts_with(&prefix.to_uppercase()) {
            return ValidationResult {
                safety: CommandSafety::Blocked,
                command: command.to_string(),
                reason: Some(format!("Profile blocked: '{prefix}'")),
            };
        }
    }
    for prefix in BLOCKED_PREFIXES.iter() {
        if cmd.starts_with(prefix) {
            return ValidationResult {
                safety: CommandSafety::Blocked,
                command: command.to_string(),
                reason: Some(format!("Command '{prefix}' is blocked for security")),
            };
        }
    }

    // 3. Check safe commands: runtime → profile → base
    if overrides.safe_commands.iter().any(|c| c.to_uppercase() == cmd) {
        return ValidationResult {
            safety: CommandSafety::Safe,
            command: command.to_string(),
            reason: None,
        };
    }
    if profile_whitelist.safe_commands.iter().any(|c| c.to_uppercase() == cmd) {
        return ValidationResult {
            safety: CommandSafety::Safe,
            command: command.to_string(),
            reason: None,
        };
    }
    if SAFE_COMMANDS.contains(cmd.as_str()) {
        return ValidationResult {
            safety: CommandSafety::Safe,
            command: command.to_string(),
            reason: None,
        };
    }

    // 4. Check confirmation commands: runtime → profile → base
    if overrides.confirmation_commands.iter().any(|c| base_cmd.starts_with(&c.to_uppercase())) {
        return ValidationResult {
            safety: CommandSafety::RequiresConfirmation,
            command: command.to_string(),
            reason: Some("Custom confirmation command".to_string()),
        };
    }
    for conf_cmd in &profile_whitelist.confirmation_commands {
        if base_cmd.starts_with(&conf_cmd.to_uppercase()) {
            return ValidationResult {
                safety: CommandSafety::RequiresConfirmation,
                command: command.to_string(),
                reason: Some(format!("Profile command '{conf_cmd}' requires confirmation")),
            };
        }
    }
    for conf_cmd in CONFIRMATION_COMMANDS.iter() {
        if base_cmd.starts_with(conf_cmd) {
            return ValidationResult {
                safety: CommandSafety::RequiresConfirmation,
                command: command.to_string(),
                reason: Some(format!("Command '{conf_cmd}' requires confirmation")),
            };
        }
    }

    // 5. Unknown commands are BLOCKED by default (fail-closed). A command that
    //    matches no known safe / confirmation / blocked entry is denied — it
    //    cannot be forced through with `confirmed: true`. Operators who need a
    //    new command must add it to the whitelist overrides explicitly.
    ValidationResult {
        safety: CommandSafety::Blocked,
        command: command.to_string(),
        reason: Some("Unknown command - blocked by default".to_string()),
    }
}

// =============================================================================
// Merged whitelist view
// =============================================================================

/// Build the full merged whitelist view for API response.
///
/// Collects commands from all three layers (base, profile, overrides),
/// deduplicates by uppercase key, resolves each command's final tier
/// (applying tier_overrides), and splits into separate lists.
///
/// `profile_label` is the short display name for profile commands (e.g. "RM551").
pub fn get_merged_whitelist(
    profile_whitelist: &ProfileAtWhitelist,
    profile_name: &str,
    profile_label: &str,
    overrides: &WhitelistOverrides,
) -> MergedWhitelist {
    let mut safe: Vec<String> = Vec::new();
    let mut confirmation: Vec<String> = Vec::new();
    let mut blocked: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    // Helper: resolve a command's final tier, accounting for tier_overrides.
    let resolve_tier = |cmd: &str, default_tier: CommandTier| -> CommandTier {
        let upper = cmd.to_uppercase();
        overrides
            .tier_overrides
            .get(&upper)
            .copied()
            .unwrap_or(default_tier)
    };

    // Helper: insert a command into the appropriate list if not already seen.
    let mut insert = |cmd: &str, tier: CommandTier| {
        let key = cmd.to_uppercase();
        if seen.insert(key) {
            match tier {
                CommandTier::Safe => safe.push(cmd.to_string()),
                CommandTier::Confirmation => confirmation.push(cmd.to_string()),
                CommandTier::Blocked => blocked.push(cmd.to_string()),
            }
        }
    };

    // Base safe commands
    for cmd in SAFE_COMMANDS.iter() {
        let tier = resolve_tier(cmd, CommandTier::Safe);
        insert(cmd, tier);
    }

    // Base confirmation commands
    for cmd in CONFIRMATION_COMMANDS.iter() {
        let tier = resolve_tier(cmd, CommandTier::Confirmation);
        insert(cmd, tier);
    }

    // Base blocked prefixes
    for cmd in BLOCKED_PREFIXES.iter() {
        let tier = resolve_tier(cmd, CommandTier::Blocked);
        insert(cmd, tier);
    }

    // Profile additions
    for cmd in &profile_whitelist.safe_commands {
        let tier = resolve_tier(cmd, CommandTier::Safe);
        insert(cmd, tier);
    }
    for cmd in &profile_whitelist.confirmation_commands {
        let tier = resolve_tier(cmd, CommandTier::Confirmation);
        insert(cmd, tier);
    }
    for cmd in &profile_whitelist.blocked_prefixes {
        let tier = resolve_tier(cmd, CommandTier::Blocked);
        insert(cmd, tier);
    }

    // Custom commands from runtime overrides
    for cmd in &overrides.safe_commands {
        insert(cmd, CommandTier::Safe);
    }
    for cmd in &overrides.confirmation_commands {
        insert(cmd, CommandTier::Confirmation);
    }
    for cmd in &overrides.blocked_prefixes {
        insert(cmd, CommandTier::Blocked);
    }

    // Sort each list alphabetically
    safe.sort_by_key(|a| a.to_uppercase());
    confirmation.sort_by_key(|a| a.to_uppercase());
    blocked.sort_by_key(|a| a.to_uppercase());

    MergedWhitelist {
        safe,
        confirmation,
        blocked_prefixes: blocked,
        profile_name: profile_name.to_string(),
        profile_label: profile_label.to_string(),
        overrides: overrides.clone(),
    }
}

// =============================================================================
// Persistence
// =============================================================================

const OVERRIDES_FILE: &str = "/etc/modem-interface/at-whitelist-overrides.json";

/// Load whitelist overrides from disk, or return defaults if not found.
pub async fn load_overrides() -> WhitelistOverrides {
    match tokio::fs::read_to_string(OVERRIDES_FILE).await {
        Ok(content) => match serde_json::from_str::<WhitelistOverrides>(&content) {
            Ok(overrides) => {
                tracing::info!("Loaded AT whitelist overrides from {}", OVERRIDES_FILE);
                overrides
            }
            Err(e) => {
                tracing::warn!("Failed to parse whitelist overrides from {}: {e}", OVERRIDES_FILE);
                WhitelistOverrides::default()
            }
        },
        Err(_) => {
            tracing::info!("No whitelist overrides file at {}, using defaults", OVERRIDES_FILE);
            WhitelistOverrides::default()
        }
    }
}

/// Save whitelist overrides to disk.
pub async fn save_overrides(overrides: &WhitelistOverrides) -> Result<(), String> {
    let json = serde_json::to_string_pretty(overrides)
        .map_err(|e| format!("Failed to serialize whitelist overrides: {e}"))?;

    if let Some(parent) = std::path::Path::new(OVERRIDES_FILE).parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("Failed to create directory: {e}"))?;
    }

    crate::config::write_secret_file(OVERRIDES_FILE, json)
        .await
        .map_err(|e| format!("Failed to write whitelist overrides: {e}"))?;

    tracing::info!("Saved AT whitelist overrides to {}", OVERRIDES_FILE);
    Ok(())
}

// =============================================================================
// Utility functions
// =============================================================================

/// Check if a command is immediately executable (no confirmation needed).
#[allow(dead_code)]
pub fn is_safe_command(command: &str) -> bool {
    matches!(validate_command(command).safety, CommandSafety::Safe)
}

/// Check if a command is blocked entirely.
#[allow(dead_code)]
pub fn is_blocked_command(command: &str) -> bool {
    matches!(validate_command(command).safety, CommandSafety::Blocked)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_safe_commands() {
        assert!(is_safe_command("AT"));
        assert!(is_safe_command("AT+CSQ"));
        assert!(is_safe_command("AT+COPS?"));
        assert!(is_safe_command("at+csq")); // Case insensitive
    }

    #[test]
    fn test_confirmation_commands() {
        let result = validate_command("AT+CFUN=1");
        assert_eq!(result.safety, CommandSafety::RequiresConfirmation);

        let result = validate_command("AT+CGDCONT=1,\"IP\",\"internet\"");
        assert_eq!(result.safety, CommandSafety::RequiresConfirmation);
    }

    #[test]
    fn test_blocked_commands() {
        assert!(is_blocked_command("AT&F"));
        assert!(is_blocked_command("AT!RESET"));
        assert!(is_blocked_command("AT+CLCK"));
    }

    #[test]
    fn test_unknown_is_blocked() {
        // Fail-closed: an unknown command must be Blocked, not pushed to the
        // confirmation tier where `confirmed: true` would let it execute.
        let result = validate_command("AT+SOMEUNKNOWN");
        assert_eq!(result.safety, CommandSafety::Blocked);
    }

    #[test]
    fn test_unknown_blocked_with_context() {
        let profile_wl = ProfileAtWhitelist::default();
        let overrides = WhitelistOverrides::default();
        let result = validate_command_with_context("AT+SOMEUNKNOWN", &profile_wl, &overrides);
        assert_eq!(result.safety, CommandSafety::Blocked);
    }

    #[test]
    fn test_context_validation_with_overrides() {
        let profile_wl = ProfileAtWhitelist::default();
        let mut overrides = WhitelistOverrides::default();
        overrides.safe_commands.push("AT+MYCUSTOM".to_string());

        let result = validate_command_with_context("AT+MYCUSTOM", &profile_wl, &overrides);
        assert_eq!(result.safety, CommandSafety::Safe);
    }

    #[test]
    fn test_never_overridable_stays_blocked() {
        // AT+CLCK is on the NEVER_OVERRIDABLE list. A tier_override trying to
        // move it to confirmation MUST be ignored — it stays Blocked.
        let profile_wl = ProfileAtWhitelist::default();
        let mut overrides = WhitelistOverrides::default();
        overrides.tier_overrides.insert("AT+CLCK".to_string(), CommandTier::Confirmation);

        let result = validate_command_with_context("AT+CLCK=test", &profile_wl, &overrides);
        assert_eq!(result.safety, CommandSafety::Blocked);
    }

    #[test]
    fn test_never_overridable_firmware_cmd_cannot_be_downgraded() {
        // A Qualcomm/Quectel firmware command (NVRAM write) must stay Blocked
        // even with an explicit Safe tier_override targeting it.
        let profile_wl = ProfileAtWhitelist::default();
        let mut overrides = WhitelistOverrides::default();
        overrides.tier_overrides.insert("AT+QNVW".to_string(), CommandTier::Safe);
        // Also try downgrading fastboot to confirmation.
        overrides.tier_overrides.insert("AT+QFASTBOOT".to_string(), CommandTier::Confirmation);

        let nvw = validate_command_with_context("AT+QNVW=1,2,3", &profile_wl, &overrides);
        assert_eq!(nvw.safety, CommandSafety::Blocked);

        let fastboot = validate_command_with_context("AT+QFASTBOOT", &profile_wl, &overrides);
        assert_eq!(fastboot.safety, CommandSafety::Blocked);
    }

    #[test]
    fn test_tier_override_still_works_for_non_protected() {
        // A non-protected base command CAN still be moved between tiers.
        // AT+CFUN (confirmation tier) → Safe via override.
        let profile_wl = ProfileAtWhitelist::default();
        let mut overrides = WhitelistOverrides::default();
        overrides.tier_overrides.insert("AT+CFUN".to_string(), CommandTier::Safe);

        let result = validate_command_with_context("AT+CFUN=1", &profile_wl, &overrides);
        assert_eq!(result.safety, CommandSafety::Safe);
    }

    #[test]
    fn test_embedded_cr_is_blocked() {
        // AT+CFUN=1 alone requires confirmation, but a smuggled CR carrying a
        // permanently-blocked command must make the whole thing Blocked.
        let result = validate_command("AT+CFUN=1\rAT+QFASTBOOT");
        assert_eq!(result.safety, CommandSafety::Blocked);

        let profile_wl = ProfileAtWhitelist::default();
        let overrides = WhitelistOverrides::default();
        let result =
            validate_command_with_context("AT+CFUN=1\rAT+QFASTBOOT", &profile_wl, &overrides);
        assert_eq!(result.safety, CommandSafety::Blocked);
    }

    #[test]
    fn test_embedded_lf_is_blocked() {
        let result = validate_command("AT+CSQ\nAT+QNVW=1,2,3");
        assert_eq!(result.safety, CommandSafety::Blocked);

        let profile_wl = ProfileAtWhitelist::default();
        let overrides = WhitelistOverrides::default();
        let result =
            validate_command_with_context("AT+CSQ\nAT+QNVW=1,2,3", &profile_wl, &overrides);
        assert_eq!(result.safety, CommandSafety::Blocked);
    }

    #[test]
    fn test_other_control_chars_are_blocked() {
        // NUL and a bare control byte must also fail closed.
        assert_eq!(validate_command("AT+CSQ\0").safety, CommandSafety::Blocked);
        assert_eq!(
            validate_command("AT\x07I").safety,
            CommandSafety::Blocked
        );
    }

    #[test]
    fn test_normal_commands_unaffected_by_control_guard() {
        // No control chars => classification proceeds as before.
        assert_eq!(validate_command("AT+CSQ").safety, CommandSafety::Safe);
        assert_eq!(
            validate_command("AT+CFUN=1").safety,
            CommandSafety::RequiresConfirmation
        );

        let profile_wl = ProfileAtWhitelist::default();
        let overrides = WhitelistOverrides::default();
        assert_eq!(
            validate_command_with_context("AT+CSQ", &profile_wl, &overrides).safety,
            CommandSafety::Safe
        );
        assert_eq!(
            validate_command_with_context("AT+CFUN=1", &profile_wl, &overrides).safety,
            CommandSafety::RequiresConfirmation
        );
    }

    #[test]
    fn test_context_profile_safe() {
        let mut profile_wl = ProfileAtWhitelist::default();
        profile_wl.safe_commands.push("AT+VENDORCMD".to_string());
        let overrides = WhitelistOverrides::default();

        let result = validate_command_with_context("AT+VENDORCMD", &profile_wl, &overrides);
        assert_eq!(result.safety, CommandSafety::Safe);
    }

    #[test]
    fn test_merged_whitelist_includes_all_sources() {
        let mut profile_wl = ProfileAtWhitelist::default();
        profile_wl.safe_commands.push("AT+PROFILECMD".to_string());

        let mut overrides = WhitelistOverrides::default();
        overrides.safe_commands.push("AT+CUSTOMCMD".to_string());

        let merged = get_merged_whitelist(&profile_wl, "TestModem", "Test", &overrides);

        // Base safe commands present
        assert!(merged.safe.contains(&"AT".to_string()));
        // Profile safe commands present
        assert!(merged.safe.contains(&"AT+PROFILECMD".to_string()));
        // Custom safe commands present
        assert!(merged.safe.contains(&"AT+CUSTOMCMD".to_string()));
        // Profile metadata populated
        assert_eq!(merged.profile_name, "TestModem");
        assert_eq!(merged.profile_label, "Test");
        // Base blocked prefixes present
        assert!(!merged.blocked_prefixes.is_empty());
        // Base confirmation commands present
        assert!(merged.confirmation.contains(&"AT+CFUN".to_string()));
    }
}
