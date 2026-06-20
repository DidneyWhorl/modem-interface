//! USB-net mode detection — boot-time only, profile-driven.
//!
//! See `docs/superpowers/specs/2026-04-30-item-37-mode-detection-design.md` for
//! the design contract. The mode-agnostic principle (`feedback_modem_mode_agnostic.md`)
//! governs how this value can be surfaced — diagnostic only, never on operator UI.
//!
//! ## Detection signal sources (Item #37 sub-task 2a)
//!
//! Detection has two independent signal sources, gathered separately and then
//! cross-checked:
//!
//! 1. **AT command** — `profile.usbnet_detect.query_cmd` (e.g. `AT+QCFG="usbnet"`
//!    for Quectel, `AT#USBCFG?` for Telit). Vendor-specific, parsed by a
//!    profile-declared parser key.
//! 2. **Kernel driver binding** — basename of `/sys/class/net/{netif}/device/driver`
//!    (e.g. `qmi_wwan` → Rmnet, `cdc_ether` → Ecm). Vendor-independent, derived
//!    from the running kernel's USB-net binding decision. Requires the modem's
//!    USB bus-port to resolve `netif` via `find_net_device_for_bus_port`.
//!
//! When both signals are present, **kernel wins on disagreement** (logged at
//! `tracing::info!` with both values). When only AT is present (no `bus_port`
//! supplied, or netif/driver not found), AT decides. When only kernel is
//! present (no `query_cmd` declared but bus_port is supplied), kernel decides.
//! When neither produces a known mode, the result is `UsbNetMode::Unknown`.
//!
//! The kernel-driver read is one-shot per call — USB hot-plug ordering can
//! race the netif into existence after detection runs; the next
//! `ModemContext`-creation event (re-detection) will retry naturally.

use super::profiles::ModemProfile;
use super::traits::ModemHardware;
use super::types::UsbNetMode;
use std::path::Path;
use std::time::Duration;

/// Run boot-time USB-net mode detection with optional kernel-driver cross-check.
///
/// When `bus_port` is `Some`, this function additionally inspects the kernel's
/// USB-net driver binding (via `/sys/class/net/<netif>/device/driver`) as a
/// second, vendor-independent signal. The two signals are reconciled by
/// [`decide_mode`]: kernel wins on disagreement, both-agree is silent success,
/// kernel-only-None falls back to AT, AT-only-Unknown falls back to kernel.
///
/// When `bus_port` is `None`, behavior is identical to [`detect_usbnet_mode`]
/// (AT-only).
///
/// Returns `UsbNetMode::Unknown` for any failure. NEVER returns `Err`.
///
/// Logs at `tracing::warn!` for unexpected AT failures, `tracing::info!` for
/// AT/kernel disagreement and unmapped responses, `tracing::debug!` for
/// routine success.
pub async fn detect_usbnet_mode_with_bus_port(
    handler: &dyn ModemHardware,
    profile: &ModemProfile,
    modem_id: &str,
    bus_port: Option<&str>,
) -> UsbNetMode {
    let at_mode = detect_at_mode(handler, profile, modem_id).await;
    let kernel_mode = bus_port.and_then(|bp| detect_kernel_mode(bp, modem_id));
    decide_mode(at_mode, kernel_mode, modem_id)
}

/// AT-driven detection branch — runs the profile's `query_cmd`, parses the
/// response, returns `UsbNetMode::Unknown` on any failure.
async fn detect_at_mode(
    handler: &dyn ModemHardware,
    profile: &ModemProfile,
    modem_id: &str,
) -> UsbNetMode {
    let Some(cmd) = profile.usbnet_detect.query_cmd.as_deref() else {
        tracing::debug!("[usbnet] {modem_id}: profile declares no detection command");
        return UsbNetMode::Unknown;
    };

    let response = match tokio::time::timeout(
        Duration::from_secs(5),
        handler.execute_at(cmd),
    )
    .await
    {
        Err(_) => {
            tracing::warn!("[usbnet] {modem_id}: detection timed out (5s)");
            return UsbNetMode::Unknown;
        }
        Ok(Err(e)) => {
            tracing::warn!("[usbnet] {modem_id}: AT failed: {e}");
            return UsbNetMode::Unknown;
        }
        Ok(Ok(s)) => s,
    };

    let Some(parser_key) = profile.usbnet_detect.parser.as_deref() else {
        tracing::info!("[usbnet] {modem_id}: response logged (no parser): {response}");
        return UsbNetMode::Unknown;
    };

    let mode = parse_usbnet_response(parser_key, &response);
    if matches!(mode, UsbNetMode::Unknown) {
        tracing::info!(
            "[usbnet] {modem_id}: parser={parser_key} returned Unknown for response: {response}"
        );
    } else {
        tracing::debug!("[usbnet] {modem_id}: AT detected mode={mode:?}");
    }
    mode
}

/// Kernel-driver detection branch — resolves the bus-port to a netif via
/// `find_net_device_for_bus_port`, reads `/sys/class/net/<netif>/device/driver`,
/// maps the driver basename to a `UsbNetMode`. Returns `None` when any step
/// produces no signal (no netif yet, no driver symlink, unmapped driver).
fn detect_kernel_mode(bus_port: &str, modem_id: &str) -> Option<UsbNetMode> {
    let netif = super::traits::find_net_device_for_bus_port(bus_port)?;
    let driver = read_kernel_driver(&netif)?;
    let mode = kernel_driver_to_mode(&driver);
    if let Some(m) = mode {
        tracing::debug!(
            "[usbnet] {modem_id}: kernel netif={netif} driver={driver} → {m:?}"
        );
    } else {
        tracing::debug!(
            "[usbnet] {modem_id}: kernel netif={netif} driver={driver} (unmapped)"
        );
    }
    mode
}

/// Reconcile AT and kernel mode signals into a single `UsbNetMode`.
///
/// Decision matrix (kernel wins on real disagreement):
/// - both Some(x) and equal      → x (silent success)
/// - both Some, different        → kernel (info-level mismatch log)
/// - AT Unknown, kernel Some(x)  → x (kernel as primary)
/// - AT Some(x), kernel None     → x (AT-only fallback)
/// - both None/Unknown           → Unknown
pub fn decide_mode(
    at_mode: UsbNetMode,
    kernel_mode: Option<UsbNetMode>,
    modem_id: &str,
) -> UsbNetMode {
    match kernel_mode {
        Some(km) => {
            if matches!(at_mode, UsbNetMode::Unknown) {
                // Kernel wins by default when AT is silent.
                km
            } else if at_mode == km {
                km
            } else {
                tracing::info!(
                    "[usbnet] {modem_id}: mode mismatch — AT={at_mode:?}, kernel={km:?}, choosing kernel"
                );
                km
            }
        }
        None => at_mode,
    }
}

/// Read the kernel-bound USB-net driver basename for `netif` from sysfs.
///
/// Reads `/sys/class/net/<netif>/device/driver`, which is a symlink whose
/// target's basename is the driver name (e.g. `qmi_wwan`, `cdc_ether`,
/// `cdc_mbim`, `cdc_ncm`, `rndis_host`).
///
/// Returns `None` on any failure (file missing, not a symlink, permission,
/// non-Linux platform). Pure filesystem read; no privileges beyond standard
/// read access required.
pub fn read_kernel_driver(netif: &str) -> Option<String> {
    read_kernel_driver_in(Path::new("/sys/class/net"), netif)
}

/// Test-injectable variant of [`read_kernel_driver`] — reads the driver symlink
/// under an arbitrary `class_net_root` (production passes `/sys/class/net`).
fn read_kernel_driver_in(class_net_root: &Path, netif: &str) -> Option<String> {
    let driver_link = class_net_root.join(netif).join("device").join("driver");
    let target = std::fs::read_link(&driver_link).ok()?;
    target
        .file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
}

/// Map a kernel USB-net driver basename to a `UsbNetMode`.
///
/// Canonical mapping (USB-net driver → mode):
///   `qmi_wwan`   → Rmnet
///   `cdc_ether`  → Ecm
///   `cdc_mbim`   → Mbim
///   `cdc_ncm`    → Ncm
///   `rndis_host` → Rndis
///   anything else → None (no kernel signal — caller falls back to AT)
pub fn kernel_driver_to_mode(driver: &str) -> Option<UsbNetMode> {
    match driver {
        "qmi_wwan" => Some(UsbNetMode::Rmnet),
        "cdc_ether" => Some(UsbNetMode::Ecm),
        "cdc_mbim" => Some(UsbNetMode::Mbim),
        "cdc_ncm" => Some(UsbNetMode::Ncm),
        "rndis_host" => Some(UsbNetMode::Rndis),
        _ => None,
    }
}

/// Map a vendor-specific AT response to a `UsbNetMode` based on the parser key.
///
/// Parser keys are declared on `ModemProfile.usbnet_detect.parser`. Unknown keys
/// return `UsbNetMode::Unknown` with a warn-level log.
pub fn parse_usbnet_response(parser: &str, response: &str) -> UsbNetMode {
    match parser {
        "quectel_qcfg_usbnet" => parse_quectel_qcfg_usbnet(response),
        "telit_usbcfg" => parse_telit_usbcfg(response),
        other => {
            tracing::warn!("[usbnet] no parser registered for key={other}");
            UsbNetMode::Unknown
        }
    }
}

/// Parse Quectel `AT+QCFG="usbnet"` response.
///
/// Format: `+QCFG: "usbnet",<n>` followed by `OK`.
///
/// Verified Quectel mapping (sources: **RG520N/RM5x0N/RM521F AT Commands Manual
/// V1.0** §2.x `AT+QCFG="usbnet"` table, and the **Quectel UMTS LTE 5G Linux USB
/// Driver User Guide V3.1** §3 USB-net composition table). Falsifies the earlier
/// "AT response unreliable on RM551E-GL" hypothesis from the 2026-05-01 bench
/// session — the modem and the kernel agree (both say RMNET/QMI); the parser
/// was simply mistranslating four of five known codes.
///
///   0 → Rmnet (QMI / raw-IP — factory default on most Quectel 5G modems)
///   1 → Ecm
///   2 → Mbim
///   3 → Rndis
///   5 → Ncm
///   anything else → Unknown
fn parse_quectel_qcfg_usbnet(response: &str) -> UsbNetMode {
    for line in response.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("+QCFG:") {
            // rest is e.g. ` "usbnet",1`
            if let Some(n_str) = rest.split(',').nth(1) {
                if let Ok(n) = n_str.trim().parse::<u32>() {
                    return match n {
                        0 => UsbNetMode::Rmnet,
                        1 => UsbNetMode::Ecm,
                        2 => UsbNetMode::Mbim,
                        3 => UsbNetMode::Rndis,
                        5 => UsbNetMode::Ncm,
                        _ => UsbNetMode::Unknown,
                    };
                }
            }
        }
    }
    UsbNetMode::Unknown
}

/// Parse Telit `AT#USBCFG?` response.
///
/// Format: `#USBCFG: <n>` followed by `OK`.
/// Telit mapping (per FN990 firmware reference; conservative — extend as more
/// codes are confirmed):
///   0 → Ecm (Telit's "MBIM-only-disabled" mode = ECM-style)
///   2 → Mbim
///   3 → Rmnet (RNDIS+ECM composite — pick Rmnet for QMI-stack semantics)
///   4 → Ncm
///   anything else → Unknown
fn parse_telit_usbcfg(response: &str) -> UsbNetMode {
    for line in response.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("#USBCFG:") {
            if let Ok(n) = rest.trim().parse::<u32>() {
                return match n {
                    0 => UsbNetMode::Ecm,
                    2 => UsbNetMode::Mbim,
                    3 => UsbNetMode::Rmnet,
                    4 => UsbNetMode::Ncm,
                    _ => UsbNetMode::Unknown,
                };
            }
        }
    }
    UsbNetMode::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quectel_parses_known_codes() {
        // Verified mapping per RG520N/RM5x0N/RM521F AT Commands Manual V1.0 +
        // Quectel UMTS LTE 5G Linux USB Driver User Guide V3.1.
        assert_eq!(
            parse_quectel_qcfg_usbnet("+QCFG: \"usbnet\",0\r\nOK\r\n"),
            UsbNetMode::Rmnet
        );
        assert_eq!(
            parse_quectel_qcfg_usbnet("+QCFG: \"usbnet\",1\r\nOK\r\n"),
            UsbNetMode::Ecm
        );
        assert_eq!(
            parse_quectel_qcfg_usbnet("+QCFG: \"usbnet\",2\r\nOK\r\n"),
            UsbNetMode::Mbim
        );
        assert_eq!(
            parse_quectel_qcfg_usbnet("+QCFG: \"usbnet\",3\r\nOK\r\n"),
            UsbNetMode::Rndis
        );
        assert_eq!(
            parse_quectel_qcfg_usbnet("+QCFG: \"usbnet\",5\r\nOK\r\n"),
            UsbNetMode::Ncm
        );
    }

    #[test]
    fn quectel_returns_unknown_for_unmapped_or_garbage() {
        assert_eq!(
            parse_quectel_qcfg_usbnet("+QCFG: \"usbnet\",99\r\n"),
            UsbNetMode::Unknown
        );
        assert_eq!(parse_quectel_qcfg_usbnet("ERROR\r\n"), UsbNetMode::Unknown);
        assert_eq!(parse_quectel_qcfg_usbnet(""), UsbNetMode::Unknown);
        assert_eq!(parse_quectel_qcfg_usbnet("garbage"), UsbNetMode::Unknown);
    }

    #[test]
    fn telit_parses_known_codes() {
        assert_eq!(
            parse_telit_usbcfg("#USBCFG: 0\r\nOK\r\n"),
            UsbNetMode::Ecm
        );
        assert_eq!(
            parse_telit_usbcfg("#USBCFG: 2\r\nOK\r\n"),
            UsbNetMode::Mbim
        );
        assert_eq!(
            parse_telit_usbcfg("#USBCFG: 3\r\nOK\r\n"),
            UsbNetMode::Rmnet
        );
        assert_eq!(
            parse_telit_usbcfg("#USBCFG: 4\r\nOK\r\n"),
            UsbNetMode::Ncm
        );
    }

    #[test]
    fn telit_returns_unknown_for_unmapped() {
        assert_eq!(
            parse_telit_usbcfg("#USBCFG: 99\r\n"),
            UsbNetMode::Unknown
        );
        assert_eq!(parse_telit_usbcfg("ERROR\r\n"), UsbNetMode::Unknown);
    }

    #[test]
    fn dispatch_unknown_parser_returns_unknown() {
        assert_eq!(
            parse_usbnet_response("nonexistent_parser", "+QCFG: \"usbnet\",1"),
            UsbNetMode::Unknown
        );
    }

    #[test]
    fn dispatch_routes_to_correct_parser() {
        // Quectel code 1 maps to Ecm under the verified mapping (sub-task 2a).
        assert_eq!(
            parse_usbnet_response("quectel_qcfg_usbnet", "+QCFG: \"usbnet\",1\r\n"),
            UsbNetMode::Ecm
        );
        assert_eq!(
            parse_usbnet_response("telit_usbcfg", "#USBCFG: 2\r\n"),
            UsbNetMode::Mbim
        );
    }

    // ========================================================================
    // Item #37 sub-task 2a — kernel-driver cross-check tests
    // ========================================================================

    #[test]
    fn kernel_driver_to_mode_maps_known_drivers() {
        assert_eq!(kernel_driver_to_mode("qmi_wwan"), Some(UsbNetMode::Rmnet));
        assert_eq!(kernel_driver_to_mode("cdc_ether"), Some(UsbNetMode::Ecm));
        assert_eq!(kernel_driver_to_mode("cdc_mbim"), Some(UsbNetMode::Mbim));
        assert_eq!(kernel_driver_to_mode("cdc_ncm"), Some(UsbNetMode::Ncm));
        assert_eq!(kernel_driver_to_mode("rndis_host"), Some(UsbNetMode::Rndis));
    }

    #[test]
    fn kernel_driver_to_mode_returns_none_for_unknown() {
        assert_eq!(kernel_driver_to_mode("usbnet"), None);
        assert_eq!(kernel_driver_to_mode(""), None);
        assert_eq!(kernel_driver_to_mode("qmi-wwan"), None); // dash, not underscore
    }

    #[test]
    fn decide_mode_kernel_wins_on_disagreement() {
        // Bench scenario: AT says Ecm (sub-task 2a fix would say Rmnet for code
        // 0; here we simulate a hypothetical cross-firmware mismatch). Kernel
        // wins regardless of which side is "right".
        assert_eq!(
            decide_mode(UsbNetMode::Ecm, Some(UsbNetMode::Rmnet), "test:1"),
            UsbNetMode::Rmnet
        );
        assert_eq!(
            decide_mode(UsbNetMode::Mbim, Some(UsbNetMode::Ecm), "test:2"),
            UsbNetMode::Ecm
        );
    }

    #[test]
    fn decide_mode_agrees_silently() {
        assert_eq!(
            decide_mode(UsbNetMode::Rmnet, Some(UsbNetMode::Rmnet), "test:3"),
            UsbNetMode::Rmnet
        );
        assert_eq!(
            decide_mode(UsbNetMode::Ecm, Some(UsbNetMode::Ecm), "test:4"),
            UsbNetMode::Ecm
        );
    }

    #[test]
    fn decide_mode_falls_back_to_at_when_kernel_absent() {
        assert_eq!(
            decide_mode(UsbNetMode::Rmnet, None, "test:5"),
            UsbNetMode::Rmnet
        );
        assert_eq!(decide_mode(UsbNetMode::Unknown, None, "test:6"), UsbNetMode::Unknown);
    }

    #[test]
    fn decide_mode_falls_back_to_kernel_when_at_unknown() {
        // AT couldn't parse / no profile cmd / timeout → kernel signal stands.
        assert_eq!(
            decide_mode(UsbNetMode::Unknown, Some(UsbNetMode::Mbim), "test:7"),
            UsbNetMode::Mbim
        );
    }

    #[cfg(unix)]
    #[test]
    fn read_kernel_driver_in_resolves_symlink_basename() {
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().expect("tempdir");
        let class_net = tmp.path();
        // Build the structure:
        //   <tmp>/wwan0/device → <tmp>/.usbdev (target dir)
        //   <tmp>/wwan0/device/driver → <tmp>/.drivers/qmi_wwan
        // Simpler: create the symlink at the exact path the function reads.
        let netif_dir = class_net.join("wwan0");
        std::fs::create_dir_all(netif_dir.join("device")).unwrap();
        let driver_target = class_net.join(".drivers").join("qmi_wwan");
        std::fs::create_dir_all(driver_target.parent().unwrap()).unwrap();
        std::fs::create_dir_all(&driver_target).unwrap();
        symlink(&driver_target, netif_dir.join("device").join("driver")).unwrap();

        let driver = read_kernel_driver_in(class_net, "wwan0");
        assert_eq!(driver.as_deref(), Some("qmi_wwan"));
    }

    #[test]
    fn read_kernel_driver_in_returns_none_for_missing_path() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // No symlink created — read should fail gracefully.
        assert_eq!(read_kernel_driver_in(tmp.path(), "wwan0"), None);
    }

    #[test]
    fn read_kernel_driver_returns_none_on_non_linux_or_missing_sysfs() {
        // Production wrapper hits /sys/class/net/<netif>/device/driver. On
        // Windows the path doesn't exist; on Linux, "no-such-iface-12345"
        // doesn't exist. Either way, None is the contract.
        assert_eq!(read_kernel_driver("no-such-iface-12345"), None);
    }
}

#[cfg(all(test, feature = "mock-hardware"))]
mod integration_tests {
    use super::*;
    use crate::hardware::mock::MockHardware;
    use crate::hardware::profiles::{builtin_profiles, UsbNetDetectConfig};

    fn quectel_mock_profile() -> ModemProfile {
        let mut p = builtin_profiles()
            .into_iter()
            .find(|p| p.identity.model == "RM551E-GL")
            .expect("RM551E-GL profile must exist");
        // Force detection on for the test (already on for builtins, but make explicit).
        p.usbnet_detect = UsbNetDetectConfig {
            query_cmd: Some("AT+QCFG=\"usbnet\"".to_string()),
            parser: Some("quectel_qcfg_usbnet".to_string()),
        };
        p
    }

    #[tokio::test]
    async fn detect_returns_rmnet_for_mock_quectel() {
        let mock = MockHardware::new();
        let profile = quectel_mock_profile();
        let mode = detect_usbnet_mode_with_bus_port(&mock, &profile, "test:modem:1", None).await;
        assert_eq!(mode, UsbNetMode::Rmnet);
    }

    #[tokio::test]
    async fn detect_returns_unknown_when_profile_lacks_query_cmd() {
        let mock = MockHardware::new();
        let mut profile = quectel_mock_profile();
        profile.usbnet_detect.query_cmd = None;
        let mode = detect_usbnet_mode_with_bus_port(&mock, &profile, "test:modem:2", None).await;
        assert_eq!(mode, UsbNetMode::Unknown);
    }
}
