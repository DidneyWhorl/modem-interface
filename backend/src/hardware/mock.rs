//! Mock hardware implementation for development and testing.
//!
//! Provides simulated modem responses that allow frontend development
//! and API testing without physical hardware. Enable with MOCK_HARDWARE=1.

use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::time::Duration;

use super::traits::*;
use super::types::*;

/// Mock modem state for simulation.
struct MockState {
    connected: bool,
    sim_present: bool,
    sim_locked: bool,
    signal_strength: i32,
    operator: String,
    ip_address: Option<String>,
    connect_time: Option<std::time::Instant>,
    bytes_tx: u64,
    bytes_rx: u64,
}

impl Default for MockState {
    fn default() -> Self {
        Self {
            connected: false,
            sim_present: true,
            sim_locked: false,
            signal_strength: -75,
            operator: "Mock Carrier".to_string(),
            ip_address: None,
            connect_time: None,
            bytes_tx: 0,
            bytes_rx: 0,
        }
    }
}

/// Mock implementation of ModemHardware for development.
pub struct MockHardware {
    state: Arc<RwLock<MockState>>,
}

impl MockHardware {
    pub fn new() -> Self {
        Self {
            state: Arc::new(RwLock::new(MockState::default())),
        }
    }

    /// Simulate signal fluctuation.
    fn jitter_signal(base: i32) -> i32 {
        use rand::Rng;
        let mut rng = rand::thread_rng();
        base + rng.gen_range(-5..=5)
    }
}

impl Default for MockHardware {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ModemHardware for MockHardware {
    async fn get_device_info(&self) -> HardwareResult<DeviceInfo> {
        // Simulate slight delay
        tokio::time::sleep(Duration::from_millis(50)).await;

        Ok(DeviceInfo {
            imei: "123456789012345".to_string(),
            manufacturer: "Quectel".to_string(),
            model: "RM520N-GL".to_string(),
            firmware_version: "RM520NGLAAR01A07M4G_01.003.01.003".to_string(),
            supported_protocols: vec!["qmi".to_string(), "at".to_string()],
        })
    }

    async fn get_status(&self) -> HardwareResult<ModemStatus> {
        let state = self.state.read().await;
        // Normalize signal to 0-100: RSSI range -113 (worst) to -51 (best)
        let signal_strength = ((state.signal_strength + 113) * 100 / 62).clamp(0, 100);

        Ok(ModemStatus {
            connected: state.connected,
            technology: if state.connected {
                Some(Technology::Gen4)
            } else {
                None
            },
            operator: if state.connected {
                Some(state.operator.clone())
            } else {
                None
            },
            signal_strength,
            ip_address: state.ip_address.clone(),
        })
    }

    async fn get_signal(&self) -> HardwareResult<SignalInfo> {
        let state = self.state.read().await;
        let rssi = Self::jitter_signal(state.signal_strength) as f64;
        Ok(SignalInfo {
            rssi,
            rsrp: rssi - 20.0,
            rsrq: -10.0 + (rssi + 113.0) / 10.0,
            sinr: 15.0 + (rssi + 113.0) / 5.0,
            band: "B3".to_string(),
            cell_id: "0x1A2B3C".to_string(),
            technology: Some(Technology::Gen4),
        })
    }

    async fn get_data_stats(&self) -> HardwareResult<DataStats> {
        let state = self.state.read().await;
        let uptime = state.connect_time.map(|t| t.elapsed().as_secs()).unwrap_or(0);

        Ok(DataStats {
            bytes_tx: state.bytes_tx,
            bytes_rx: state.bytes_rx,
            session_uptime_secs: uptime,
        })
    }

    async fn connect(&self, config: &ConnectionConfig) -> HardwareResult<()> {
        let state = self.state.write().await;

        if !state.sim_present {
            return Err(HardwareError::SimError("No SIM card".to_string()));
        }
        if state.sim_locked {
            return Err(HardwareError::SimError("SIM PIN required".to_string()));
        }
        if config.apn.is_empty() {
            return Err(HardwareError::CommandRejected("APN required".to_string()));
        }

        // Simulate connection delay
        drop(state);
        tokio::time::sleep(Duration::from_millis(500)).await;

        let mut state = self.state.write().await;
        state.connected = true;
        state.ip_address = Some("10.0.0.42".to_string());
        state.connect_time = Some(std::time::Instant::now());
        state.bytes_tx = 0;
        state.bytes_rx = 0;

        tracing::info!(apn = %config.apn, "Mock modem connected");
        Ok(())
    }

    async fn disconnect(&self) -> HardwareResult<()> {
        let mut state = self.state.write().await;

        if !state.connected {
            return Ok(());
        }

        state.connected = false;
        state.ip_address = None;
        state.connect_time = None;

        tracing::info!("Mock modem disconnected");
        Ok(())
    }

    async fn reconnect(&self) -> HardwareResult<()> {
        // Simulate the CFUN radio cycle delay.
        tokio::time::sleep(Duration::from_millis(500)).await;

        let mut state = self.state.write().await;
        state.connected = true;
        state.ip_address = Some("10.0.0.42".to_string());
        state.connect_time = Some(std::time::Instant::now());

        tracing::info!("Mock modem reconnected (saved APN, no CGDCONT)");
        Ok(())
    }

    async fn get_sim_status(&self) -> HardwareResult<SimStatus> {
        let state = self.state.read().await;

        Ok(SimStatus {
            present: state.sim_present,
            state: if !state.sim_present {
                SimState::NotInserted
            } else if state.sim_locked {
                SimState::PinRequired
            } else {
                SimState::Ready
            },
            iccid: if state.sim_present {
                Some("89012345678901234567".to_string())
            } else {
                None
            },
            imsi: if state.sim_present && !state.sim_locked {
                Some("310260123456789".to_string())
            } else {
                None
            },
            operator_name: if state.sim_present {
                Some("Mock Carrier".to_string())
            } else {
                None
            },
        })
    }

    async fn verify_pin(&self, pin: &str) -> HardwareResult<()> {
        let mut state = self.state.write().await;

        if !state.sim_present {
            return Err(HardwareError::SimError("No SIM card".to_string()));
        }
        if !state.sim_locked {
            return Ok(());
        }

        // Mock PIN: "1234"
        if pin == "1234" {
            state.sim_locked = false;
            Ok(())
        } else {
            Err(HardwareError::SimError("Incorrect PIN".to_string()))
        }
    }

    async fn change_pin(&self, old_pin: &str, new_pin: &str) -> HardwareResult<()> {
        if old_pin != "1234" {
            return Err(HardwareError::SimError("Incorrect PIN".to_string()));
        }
        if new_pin.len() < 4 || new_pin.len() > 8 {
            return Err(HardwareError::SimError("PIN must be 4-8 digits".to_string()));
        }
        Ok(())
    }

    async fn enable_pin(&self, pin: &str) -> HardwareResult<()> {
        if pin != "1234" {
            return Err(HardwareError::SimError("Incorrect PIN".to_string()));
        }
        Ok(())
    }

    async fn disable_pin(&self, pin: &str) -> HardwareResult<()> {
        if pin != "1234" {
            return Err(HardwareError::SimError("Incorrect PIN".to_string()));
        }
        Ok(())
    }

    async fn get_registration(&self) -> HardwareResult<RegistrationState> {
        let state = self.state.read().await;
        if state.connected {
            Ok(RegistrationState::Registered { home: true })
        } else {
            Ok(RegistrationState::NotRegistered)
        }
    }

    async fn scan_networks(&self) -> HardwareResult<Vec<AvailableNetwork>> {
        // Simulate long scan time
        tokio::time::sleep(Duration::from_secs(2)).await;

        Ok(vec![
            AvailableNetwork {
                operator_name: "Mock Carrier".to_string(),
                operator_code: "310260".to_string(),
                technology: Technology::Gen4,
                status: NetworkStatus::Current,
            },
            AvailableNetwork {
                operator_name: "Test Mobile".to_string(),
                operator_code: "310410".to_string(),
                technology: Technology::Gen4,
                status: NetworkStatus::Available,
            },
            AvailableNetwork {
                operator_name: "Demo Network".to_string(),
                operator_code: "311480".to_string(),
                technology: Technology::Gen5,
                status: NetworkStatus::Available,
            },
        ])
    }

    async fn select_network(&self, operator_code: Option<&str>) -> HardwareResult<()> {
        tokio::time::sleep(Duration::from_millis(500)).await;

        if let Some(code) = operator_code {
            if !["310260", "310410", "311480"].contains(&code) {
                return Err(HardwareError::CommandRejected(format!(
                    "Unknown operator: {code}"
                )));
            }
        }

        Ok(())
    }

    async fn get_gps_position(&self) -> HardwareResult<GpsInfo> {
        Ok(GpsInfo {
            latitude: 37.38746,
            longitude: -121.97236,
            altitude: Some(9.0),
            speed: Some(0.0),
            fix_type: "3D".to_string(),
            satellites: 12,
            timestamp: "2026-02-10T12:34:56Z".to_string(),
        })
    }

    async fn stop_gps(&self) -> HardwareResult<()> {
        Ok(())
    }

    async fn get_extended_signal(&self) -> HardwareResult<ExtendedSignalInfo> {
        let state = self.state.read().await;
        let rssi = Self::jitter_signal(state.signal_strength) as f64;

        // Simulate QCAINFO PCC: LTE Band 2 is the primary component carrier
        // (the LTE anchor in NSA mode). This matches real behavior where
        // AT+QCAINFO returns:
        //   +QCAINFO: "PCC",5330,50,"LTE BAND 2",2,325,-85,-10,-54,15
        //   +QCAINFO: "SCC",3450,50,"LTE BAND 7",2,100,...
        //   +QCAINFO: "SCC",627264,100,"NR5G BAND 78",2,500,...
        //   +QCAINFO: "SCC",66486,50,"LTE BAND 66",2,200,...
        let primary = SignalInfo {
            rssi,
            rsrp: rssi - 20.0,
            rsrq: -10.0 + (rssi + 113.0) / 10.0,
            sinr: 15.0 + (rssi + 113.0) / 5.0,
            band: "B2".to_string(),
            cell_id: "0x1A2B3C".to_string(),
            technology: Some(Technology::Gen4),
        };

        if !state.connected {
            return Ok(ExtendedSignalInfo {
                primary,
                secondary_cells: vec![],
                carrier_aggregation: false,
                network_type: String::new(),
            });
        }

        Ok(ExtendedSignalInfo {
            primary,
            secondary_cells: vec![
                SignalInfo {
                    rssi: -72.0,
                    rsrp: -100.0,
                    rsrq: -12.0,
                    sinr: 12.0,
                    band: "B7".to_string(),
                    cell_id: String::new(),
                    technology: None,
                },
                SignalInfo {
                    rssi: -999.0,
                    rsrp: -92.0,
                    rsrq: -9.0,
                    sinr: 18.0,
                    band: "n78".to_string(),
                    cell_id: String::new(),
                    technology: None,
                },
                SignalInfo {
                    rssi: -68.0,
                    rsrp: -95.0,
                    rsrq: -11.0,
                    sinr: 14.0,
                    band: "B66".to_string(),
                    cell_id: String::new(),
                    technology: None,
                },
            ],
            carrier_aggregation: true,
            network_type: "FDD LTE".to_string(),
        })
    }

    async fn get_antenna_metrics(&self) -> HardwareResult<AntennaMetrics> {
        let state = self.state.read().await;
        if !state.connected {
            return Ok(AntennaMetrics::default());
        }
        Ok(AntennaMetrics {
            ports: vec![
                // LTE anchor layer (4 ports)
                AntennaPort { port: 0, rsrp: -97.0, rsrq: -11.0, sinr: 10.0, technology: Some("LTE".into()) },
                AntennaPort { port: 1, rsrp: -99.0, rsrq: -12.0, sinr: 8.0, technology: Some("LTE".into()) },
                AntennaPort { port: 2, rsrp: -88.0, rsrq: -9.0, sinr: 14.0, technology: Some("LTE".into()) },
                AntennaPort { port: 3, rsrp: -91.0, rsrq: -10.0, sinr: 12.0, technology: Some("LTE".into()) },
                // NR5G-NSA overlay (2 ports - typical for NSA mode)
                AntennaPort { port: 0, rsrp: -85.0, rsrq: -8.0, sinr: 18.0, technology: Some("NR5G-NSA".into()) },
                AntennaPort { port: 1, rsrp: -87.0, rsrq: -9.0, sinr: 16.0, technology: Some("NR5G-NSA".into()) },
            ],
        })
    }

    async fn execute_at(&self, command: &str) -> HardwareResult<String> {
        // Simulate command delay
        tokio::time::sleep(Duration::from_millis(50)).await;

        let response = match command.to_uppercase().as_str() {
            "AT" => "OK".to_string(),
            "ATI" => "Quectel\nRM520N-GL\nRevision: RM520NGLAAR01A07M4G\n\nOK".to_string(),
            "AT+CSQ" => {
                let state = self.state.read().await;
                let csq = ((state.signal_strength + 113) / 2).clamp(0, 31);
                format!("+CSQ: {csq},99\n\nOK")
            }
            "AT+COPS?" => {
                let state = self.state.read().await;
                if state.connected {
                    format!("+COPS: 0,0,\"{}\",7\n\nOK", state.operator)
                } else {
                    "+COPS: 0\n\nOK".to_string()
                }
            }
            "AT+CREG?" => "+CREG: 0,1\n\nOK".to_string(),
            "AT+CPIN?" => {
                let state = self.state.read().await;
                if !state.sim_present {
                    "+CME ERROR: 10".to_string()
                } else if state.sim_locked {
                    "+CPIN: SIM PIN\n\nOK".to_string()
                } else {
                    "+CPIN: READY\n\nOK".to_string()
                }
            }
            "AT+QUIMSLOT?" => "+QUIMSLOT: 1\n\nOK".to_string(),
            "AT+QUIMSLOT=1" | "AT+QUIMSLOT=2" => "OK".to_string(),
            "AT+QINISTAT" => "+QINISTAT: 7\n\nOK".to_string(),
            "AT+QSIMSTAT?" => "+QSIMSTAT: 1,1\n\nOK".to_string(),
            "AT+QPINC" => "+QPINC: \"SC\",3,10\n\nOK".to_string(),
            // USB-net mode query — return code 0 (Rmnet, the QMI/raw-IP factory
            // default on Quectel 5G modems) per the verified mapping in sub-task
            // 2a. Keeps the usbnet integration test asserting the most common
            // real-world mode rather than a synthetic ECM stand-in.
            "AT+QCFG=\"USBNET\"" => "+QCFG: \"usbnet\",0\r\nOK\r\n".to_string(),
            "AT#USBCFG?" => "#USBCFG: 3\r\nOK\r\n".to_string(),
            _ => {
                let cmd_upper = command.to_uppercase();
                // Mock dual SIM commands with dynamic slot
                if cmd_upper.starts_with("AT+QUIMSLOT=") {
                    return Ok("OK".to_string());
                }
                // Mock MBN carrier profile responses for AT+QMBNCFG
                if cmd_upper.contains("QMBNCFG") {
                    if cmd_upper.contains("\"LIST\"") {
                        "+QMBNCFG: \"List\",0,1,1,\"ROW_Commercial\",0x0A010809,202408051\n\
                         +QMBNCFG: \"List\",1,0,0,\"VoLTE-ATT\",0x0A010335,202302241\n\
                         +QMBNCFG: \"List\",2,0,0,\"Commercial-TMO\",0x0A01050F,202408301\n\
                         +QMBNCFG: \"List\",3,0,0,\"CDMAless-Verizon\",0x0A010126,202506041\n\
                         +QMBNCFG: \"List\",4,0,0,\"Commercial-Sprint\",0x0A010204,202302241\n\n\
                         OK".to_string()
                    } else if cmd_upper.contains("\"AUTOSEL\"") && !cmd_upper.contains(',') {
                        "+QMBNCFG: \"AutoSel\",0\n\nOK".to_string()
                    } else if cmd_upper.contains("\"SELECT\"") && !cmd_upper.contains(',') {
                        "+QMBNCFG: \"Select\",ROW_Commercial\n\nOK".to_string()
                    } else {
                        // Set commands (AutoSel with value, Select with value, Deactivate)
                        "OK".to_string()
                    }
                } else if cmd_upper.contains("QNWPREFCFG") {
                // Mock band/mode configuration responses for AT+QNWPREFCFG
                    if cmd_upper.contains("MODE_PREF") && !cmd_upper.contains(',') {
                        "+QNWPREFCFG: \"mode_pref\",AUTO\n\nOK".to_string()
                    } else if cmd_upper.contains("NR5G_DISABLE_MODE") && !cmd_upper.contains(',') {
                        "+QNWPREFCFG: \"nr5g_disable_mode\",0\n\nOK".to_string()
                    } else if cmd_upper.contains("LTE_BAND") && !cmd_upper.contains(',') {
                        "+QNWPREFCFG: \"lte_band\",1:2:3:4:5:7:8:12:13:14:17:18:19:20:25:26:28:29:30:32:34:38:39:40:41:42:43:46:48:53:66:70:71\n\nOK".to_string()
                    } else if cmd_upper.contains("NSA_NR5G_BAND") && !cmd_upper.contains(',') {
                        "+QNWPREFCFG: \"nsa_nr5g_band\",1:2:3:5:7:8:12:13:14:18:20:25:26:28:29:30:38:40:41:48:53:66:70:71:75:76:77:78:79:92:94:257:258:260:261\n\nOK".to_string()
                    } else if cmd_upper.contains("NR5G_BAND") && !cmd_upper.contains("NSA") && !cmd_upper.contains("NRDC") && !cmd_upper.contains(',') {
                        "+QNWPREFCFG: \"nr5g_band\",1:2:3:5:7:8:12:13:14:18:20:25:26:28:29:30:38:40:41:48:53:66:70:71:75:76:77:78:79:91:92:93:94:257:258:260:261\n\nOK".to_string()
                    } else if cmd_upper.contains("NRDC_NR5G_BAND") && !cmd_upper.contains(',') {
                        "+QNWPREFCFG: \"nrdc_nr5g_band\",1:2:3:5:7:8:12:13:14:18:20:25:26:28:29:30:38:40:41:48:53:66:70:71:75:76:77:78:79:91:92:93:94:257:258:260:261\n\nOK".to_string()
                    } else if cmd_upper.contains("NRDC_MODE") && !cmd_upper.contains(',') {
                        "+QNWPREFCFG: \"nrdc_mode\",0\n\nOK".to_string()
                    } else {
                        // Set commands (contain comma) → just return OK
                        "OK".to_string()
                    }
                } else {
                    "OK".to_string()
                }
            }
        };

        Ok(response)
    }
}

/// Detect mock modems (returns one mock device).
#[allow(dead_code)]
pub fn detect_mock_modems() -> Vec<DetectedModem> {
    vec![DetectedModem {
        device_path: "/dev/mock0".to_string(),
        protocol: ModemProtocol::Qmi,
        description: "Mock Quectel RM520N-GL".to_string(),
        vendor_id: Some("2c7c".to_string()),
        product_id: Some("0801".to_string()),
        profile_id: Some("generic".to_string()),
        has_profile: false,
        bus_port: None,
        all_ports: Vec::new(),
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mock_device_info() {
        let modem = MockHardware::new();
        let info = modem.get_device_info().await.unwrap();
        assert_eq!(info.manufacturer, "Quectel");
        assert_eq!(info.model, "RM520N-GL");
    }

    #[tokio::test]
    async fn test_mock_connect_disconnect() {
        let modem = MockHardware::new();

        let config = ConnectionConfig {
            cid: 1,
            apn: "internet".to_string(),
            username: None,
            password: None,
            auth_type: AuthType::None,
            ip_type: IpType::Ipv4,
        };

        modem.connect(&config).await.unwrap();
        let status = modem.get_status().await.unwrap();
        assert!(status.connected);
        assert!(status.ip_address.is_some());

        modem.disconnect().await.unwrap();
        let status = modem.get_status().await.unwrap();
        assert!(!status.connected);
    }

    #[tokio::test]
    async fn test_mock_reconnect_after_disconnect() {
        let modem = MockHardware::new();

        let config = ConnectionConfig {
            cid: 1,
            apn: "internet".to_string(),
            username: None,
            password: None,
            auth_type: AuthType::None,
            ip_type: IpType::Ipv4,
        };

        modem.connect(&config).await.unwrap();
        modem.disconnect().await.unwrap();
        let status = modem.get_status().await.unwrap();
        assert!(!status.connected);

        // reconnect() brings the bearer back up using the saved APN — no config.
        modem.reconnect().await.unwrap();
        let status = modem.get_status().await.unwrap();
        assert!(status.connected);

        // connect_time is set, so data stats report a live session.
        let stats = modem.get_data_stats().await.unwrap();
        let _ = stats; // session_uptime_secs derives from connect_time being Some
    }

    #[tokio::test]
    async fn test_mock_at_commands() {
        let modem = MockHardware::new();

        let resp = modem.execute_at("AT").await.unwrap();
        assert_eq!(resp, "OK");

        let resp = modem.execute_at("AT+CSQ").await.unwrap();
        assert!(resp.contains("+CSQ:"));
    }
}
