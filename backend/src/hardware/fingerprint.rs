//! Device token generation from hardware fingerprint.
//!
//! Produces a deterministic, stable identifier from the router's hardware
//! characteristics. The token is a Base32-encoded SHA-256 hash of sorted
//! hardware identifiers joined with ":".
//!
//! **Generate-once, persist-forever model:** On first run the token is computed
//! from hardware inputs and written to `/etc/modem-interface/device-token`.
//! Subsequent boots read the persisted file, ensuring the token never changes
//! even if a MAC address is randomly regenerated.

#[cfg(target_os = "linux")]
use sha2::{Digest, Sha256};
#[cfg(target_os = "linux")]
use std::collections::BTreeMap;

/// Path where the device token is persisted across reboots.
#[cfg(target_os = "linux")]
const PERSISTED_TOKEN_PATH: &str = "/etc/modem-interface/device-token";

/// Generate a deterministic device token from hardware identifiers.
///
/// On Linux (router), reads board name, model, CPU serial, MAC address,
/// machine ID, and root partition UUID. At minimum the primary MAC must
/// be present.
///
/// On non-Linux (development), returns a fixed mock token.
pub fn generate_device_token() -> String {
    #[cfg(target_os = "linux")]
    {
        generate_device_token_linux()
    }
    #[cfg(not(target_os = "linux"))]
    {
        "MOCK-DEV-TOKEN-NOT-FOR-PRODUCTION".to_string()
    }
}

/// Returns true if the MAC address is globally unique (factory-assigned).
///
/// Locally administered MACs have bit 1 of the first octet set (the U/L bit).
/// A MAC is globally unique when `(first_byte & 0x02) == 0`.
/// Also rejects the all-zeros address `00:00:00:00:00:00`.
#[cfg(any(target_os = "linux", test))]
fn is_globally_unique_mac(mac: &str) -> bool {
    let mac = mac.trim();
    if mac.is_empty() || mac == "00:00:00:00:00:00" {
        return false;
    }
    // Parse first octet from "xx:yy:zz:..." format
    let first_octet_str = match mac.split(':').next() {
        Some(s) => s,
        None => return false,
    };
    match u8::from_str_radix(first_octet_str, 16) {
        Ok(byte) => (byte & 0x02) == 0,
        Err(_) => false,
    }
}

#[cfg(target_os = "linux")]
fn is_valid_persisted_token(contents: &str) -> bool {
    let trimmed = contents.trim();
    let len = trimmed.len();
    (40..=60).contains(&len) && trimmed.chars().all(|c| c.is_ascii_alphanumeric())
}

#[cfg(target_os = "linux")]
fn read_persisted_token() -> Option<String> {
    match std::fs::read_to_string(PERSISTED_TOKEN_PATH) {
        Ok(contents) => {
            if is_valid_persisted_token(&contents) {
                let token = contents.trim().to_string();
                tracing::info!(
                    "Using persisted device token from {}",
                    PERSISTED_TOKEN_PATH
                );
                Some(token)
            } else {
                None
            }
        }
        Err(_) => None,
    }
}

#[cfg(target_os = "linux")]
fn persist_token(token: &str) {
    // Create parent directory if needed
    if let Some(parent) = std::path::Path::new(PERSISTED_TOKEN_PATH).parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            tracing::warn!(
                "Failed to create directory {}: {}",
                parent.display(),
                e
            );
            return;
        }
    }
    match std::fs::write(PERSISTED_TOKEN_PATH, token) {
        Ok(()) => {
            tracing::info!(
                "Persisted new device token to {}",
                PERSISTED_TOKEN_PATH
            );
            // Best-effort chmod 0600 — token is a portal credential, not world-readable
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(
                    PERSISTED_TOKEN_PATH,
                    std::fs::Permissions::from_mode(0o600),
                );
            }
        }
        Err(e) => {
            tracing::warn!(
                "Failed to persist device token to {}: {} (token is still usable this session)",
                PERSISTED_TOKEN_PATH,
                e
            );
        }
    }
}

/// Read a MAC address from an interface via `ip link show <iface>`.
#[cfg(target_os = "linux")]
fn read_mac_from_interface(iface: &str) -> Option<String> {
    let output = std::process::Command::new("ip")
        .args(["link", "show", iface])
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("link/ether ") {
            if let Some(mac) = rest.split_whitespace().next() {
                let mac = mac.to_lowercase();
                if mac != "00:00:00:00:00:00" && !mac.is_empty() {
                    return Some(mac);
                }
            }
        }
    }
    None
}

/// Read MAC addresses from WiFi PHY devices in sysfs.
#[cfg(target_os = "linux")]
fn read_wifi_phy_macs() -> Vec<(String, String)> {
    let mut results = Vec::new();
    let phy_dir = std::path::Path::new("/sys/class/ieee80211");
    if let Ok(entries) = std::fs::read_dir(phy_dir) {
        let mut phys: Vec<_> = entries.filter_map(|e| e.ok()).collect();
        phys.sort_by_key(|e| e.file_name());
        for entry in phys {
            let mac_path = entry.path().join("macaddress");
            if let Ok(mac) = std::fs::read_to_string(&mac_path) {
                let mac = mac.trim().to_lowercase();
                if !mac.is_empty() && mac != "00:00:00:00:00:00" {
                    let name = entry.file_name().to_string_lossy().to_string();
                    results.push((name, mac));
                }
            }
        }
    }
    results
}

/// Read addr_assign_type for a network interface (0 = permanent/factory).
#[cfg(target_os = "linux")]
fn read_addr_assign_type(iface: &str) -> Option<u8> {
    let path = format!("/sys/class/net/{iface}/addr_assign_type");
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| s.trim().parse::<u8>().ok())
}

/// Find interfaces with addr_assign_type == 0 (permanent MAC).
#[cfg(target_os = "linux")]
fn find_permanent_mac_interfaces() -> Vec<(String, String)> {
    let mut results = Vec::new();
    let net_dir = std::path::Path::new("/sys/class/net");
    if let Ok(entries) = std::fs::read_dir(net_dir) {
        let mut ifaces: Vec<_> = entries.filter_map(|e| e.ok()).collect();
        ifaces.sort_by_key(|e| e.file_name());
        for entry in ifaces {
            let iface = entry.file_name().to_string_lossy().to_string();
            if iface == "lo" {
                continue;
            }
            if read_addr_assign_type(&iface) == Some(0) {
                let addr_path = entry.path().join("address");
                if let Ok(mac) = std::fs::read_to_string(addr_path) {
                    let mac = mac.trim().to_lowercase();
                    if !mac.is_empty() && mac != "00:00:00:00:00:00" {
                        results.push((iface, mac));
                    }
                }
            }
        }
    }
    results
}

/// Select the best primary MAC address using an ordered preference list.
///
/// Priority:
/// 1. `br-lan` (preserves existing BPi-R4-Pro tokens)
/// 2. `eth0`
/// 3. WiFi PHY MACs from `/sys/class/ieee80211/phy*/macaddress`
/// 4. Any interface with `addr_assign_type == 0`
/// 5. Fallback to `br-lan` even if locally administered
///
/// At each step, only globally-unique (factory-burned) MACs are accepted.
/// Returns `(source_label, mac)` or None if nothing found at all.
#[cfg(target_os = "linux")]
fn select_primary_mac() -> Option<(String, String)> {
    // 1. br-lan (current behavior — keeps existing BPi tokens stable)
    if let Some(mac) = read_mac_from_interface("br-lan") {
        if is_globally_unique_mac(&mac) {
            return Some(("br-lan".to_string(), mac));
        }
    }

    // 2. eth0
    if let Some(mac) = read_mac_from_interface("eth0") {
        if is_globally_unique_mac(&mac) {
            return Some(("eth0".to_string(), mac));
        }
    }

    // 3. WiFi PHY MACs
    for (phy_name, mac) in read_wifi_phy_macs() {
        if is_globally_unique_mac(&mac) {
            return Some((format!("wifi/{phy_name}"), mac));
        }
    }

    // 4. Any interface with addr_assign_type == 0
    for (iface, mac) in find_permanent_mac_interfaces() {
        if is_globally_unique_mac(&mac) {
            return Some((iface, mac));
        }
    }

    // 5. Fallback to br-lan even if locally administered
    if let Some(mac) = read_mac_from_interface("br-lan") {
        tracing::warn!(
            "No globally-unique MAC found; falling back to br-lan (locally administered)"
        );
        return Some(("br-lan".to_string(), mac));
    }

    None
}

#[cfg(target_os = "linux")]
fn generate_device_token_linux() -> String {
    // Step 1: Check for persisted token
    if let Some(token) = read_persisted_token() {
        return token;
    }

    // Step 2: Generate from hardware inputs
    let mut sources: BTreeMap<&str, String> = BTreeMap::new();
    let mut input_labels: Vec<String> = Vec::new();

    // Board name
    if let Ok(val) = std::fs::read_to_string("/tmp/sysinfo/board_name") {
        let val = val.trim().to_string();
        if !val.is_empty() {
            sources.insert("board_name", val);
            input_labels.push("board_name".to_string());
        }
    }

    // Board model
    if let Ok(val) = std::fs::read_to_string("/tmp/sysinfo/model") {
        let val = val.trim().to_string();
        if !val.is_empty() {
            sources.insert("board_model", val);
            input_labels.push("board_model".to_string());
        }
    }

    // CPU serial from /proc/cpuinfo
    if let Ok(cpuinfo) = std::fs::read_to_string("/proc/cpuinfo") {
        for line in cpuinfo.lines() {
            if let Some(serial) = line.strip_prefix("Serial") {
                let serial = serial.trim_start_matches(|c: char| c == ':' || c.is_whitespace());
                if !serial.is_empty() {
                    sources.insert("cpu_serial", serial.to_string());
                    input_labels.push("cpu_serial".to_string());
                    break;
                }
            }
        }
    }

    // Primary MAC address — improved selection
    if let Some((source, mac)) = select_primary_mac() {
        sources.insert("primary_mac", mac);
        input_labels.push(format!("primary_mac ({source})"));
    }

    // Machine ID
    if let Ok(val) = std::fs::read_to_string("/etc/machine-id") {
        let val = val.trim().to_string();
        if !val.is_empty() {
            sources.insert("machine_id", val);
            input_labels.push("machine_id".to_string());
        }
    }

    // Root partition UUID
    if let Ok(output) = std::process::Command::new("blkid")
        .arg("/dev/root")
        .output()
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        // Parse UUID="..." from blkid output
        if let Some(start) = stdout.find("UUID=\"") {
            let rest = &stdout[start + 6..];
            if let Some(end) = rest.find('"') {
                let uuid = &rest[..end];
                if !uuid.is_empty() {
                    sources.insert("root_uuid", uuid.to_string());
                    input_labels.push("root_uuid".to_string());
                }
            }
        }
    }

    // Require at least the primary MAC
    if !sources.contains_key("primary_mac") {
        tracing::warn!("Device fingerprint: primary MAC not found, using fallback mock token");
        return "MOCK-DEV-TOKEN-NOT-FOR-PRODUCTION".to_string();
    }

    tracing::debug!("Device fingerprint inputs: {}", input_labels.join(", "));

    // Sort by key (BTreeMap is already sorted), join with ":"
    let joined: String = sources
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(":");

    // SHA-256 hash
    let hash = Sha256::digest(joined.as_bytes());

    // Base32 encode (no padding)
    let token = data_encoding::BASE32_NOPAD.encode(&hash);

    // Step 3: Persist the token
    persist_token(&token);

    token
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mock_token_on_non_linux() {
        // On non-Linux (CI/dev), should return the mock token
        let token = generate_device_token();
        #[cfg(not(target_os = "linux"))]
        assert_eq!(token, "MOCK-DEV-TOKEN-NOT-FOR-PRODUCTION");
        // On Linux without br-lan, should also return mock
        let _ = token;
    }

    #[test]
    fn test_deterministic() {
        let t1 = generate_device_token();
        let t2 = generate_device_token();
        assert_eq!(t1, t2, "Device token must be deterministic");
    }

    #[test]
    fn test_is_globally_unique_mac() {
        // Factory-burned MACs (bit 1 of first octet = 0)
        assert!(is_globally_unique_mac("00:11:22:33:44:55"));
        assert!(is_globally_unique_mac("ac:de:48:00:11:22"));
        assert!(is_globally_unique_mac("10:20:30:40:50:60"));

        // Locally administered MACs (bit 1 of first octet = 1)
        assert!(!is_globally_unique_mac("02:00:00:00:00:01")); // bit 1 set
        assert!(!is_globally_unique_mac("06:11:22:33:44:55")); // bit 1 set
        assert!(!is_globally_unique_mac("fe:ff:ff:ff:ff:ff")); // bit 1 set
        assert!(!is_globally_unique_mac("da:d1:d2:d3:d4:d5")); // 0xda = 1101_1010, bit 1 set

        // Edge cases
        assert!(!is_globally_unique_mac("00:00:00:00:00:00")); // all zeros
        assert!(!is_globally_unique_mac(""));                   // empty
        assert!(!is_globally_unique_mac("not-a-mac"));          // garbage
        assert!(!is_globally_unique_mac("zz:00:00:00:00:00")); // invalid hex
    }

    #[test]
    fn test_is_globally_unique_mac_whitespace() {
        // Should handle leading/trailing whitespace
        assert!(is_globally_unique_mac("  00:11:22:33:44:55  "));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_is_valid_persisted_token() {
        // Valid tokens (40-60 alphanumeric chars)
        assert!(is_valid_persisted_token(
            "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567ABCDEFGH" // 40 chars
        ));
        assert!(is_valid_persisted_token(
            "  ABCDEFGHIJKLMNOPQRSTUVWXYZ234567ABCDEFGH\n" // with whitespace
        ));

        // Invalid tokens
        assert!(!is_valid_persisted_token("SHORT")); // too short
        assert!(!is_valid_persisted_token("")); // empty
        assert!(!is_valid_persisted_token("ABCDEF!@#$%^&*()GHIJKLMNOPQRSTUVWXYZ234567")); // non-alnum
        assert!(!is_valid_persisted_token(
            &"A".repeat(61) // too long
        ));
    }
}
