# Hardware Trait Contract — modem-interface v1.0.150

**Owner:** PM/Architecture chat
**Implements:** Hardware session (`backend/src/hardware/`)
**Consumes:** Backend/API session (calls through `ModemContext.handler`)
**Last updated:** 2026-05-26 — added live_device_path_handle (self-heal device_path reconcile)

> ⚠️ Trait signature changes require PM approval and coordinated update of both sessions.
> Log changes needed to `docs/PENDING-CHANGES.md` before implementing.

---

## ModemHardware Trait

This is the **only interface the API layer calls** into the hardware layer.  
All implementations (real AT handler + mock) must satisfy this contract.

```rust
#[async_trait]
pub trait ModemHardware: Send {
    /// Get device identification (IMEI, manufacturer, model, firmware).
    async fn get_device_info(&self) -> Result<DeviceInfo, HardwareError>;

    /// Get current connection state and network info.
    async fn get_status(&self) -> Result<ModemStatus, HardwareError>;

    /// Get detailed signal metrics (RSSI, RSRP, RSRQ, SINR, band, cell ID).
    async fn get_signal(&self) -> Result<SignalInfo, HardwareError>;

    /// Get extended signal info (carrier aggregation, neighbour cells).
    async fn get_extended_signal(&self) -> Result<ExtendedSignalInfo, HardwareError>;

    /// Get per-RX-port antenna metrics.
    async fn get_antenna_metrics(&self) -> Result<AntennaMetrics, HardwareError>;

    /// Get data usage stats for current session.
    async fn get_data_stats(&self) -> Result<DataStats, HardwareError>;

    /// Establish data connection using provided APN config.
    /// Uses AT+CFUN flight mode sequence (not AT+CGACT).
    async fn connect(&self, config: &ConnectionConfig) -> Result<(), HardwareError>;

    /// Terminate data connection.
    async fn disconnect(&self) -> Result<(), HardwareError>;

    /// Re-establish the data bearer using the APN already saved on the modem.
    /// Pure radio cycle (AT+CFUN=0 → ~1s → AT+CFUN=1), NO AT+CGDCONT write.
    /// Distinct from connect(), which writes a new PDP context before cycling.
    async fn reconnect(&self) -> Result<(), HardwareError>;

    /// Execute a raw AT command and return the response string.
    /// Used by the command endpoint (whitelist-validated before calling).
    async fn execute_at(&self, command: &str) -> Result<String, HardwareError>;

    /// Get current GPS position.
    /// Returns error if GPS not supported by this modem.
    async fn get_gps_position(&self) -> Result<GpsInfo, HardwareError>;

    /// Stop the GPS engine (AT+QGPSEND or equivalent).
    async fn stop_gps(&self) -> Result<(), HardwareError>;

    /// Shared handle to the AT port path the handler is *actually* using.
    /// Real AT handler returns Some(cell) updated on every self-heal reopen;
    /// the mock (and the default) return None. The state layer reconciles this
    /// into the reported device_path once per 60s cache cycle.
    fn live_device_path_handle(&self) -> Option<Arc<Mutex<String>>> { None }
}
```

### Rules for Implementors

1. Every method must be `Send` safe — the handler is behind `Arc<Mutex<...>>`
2. Return `HardwareError` not panics — the API layer converts errors to HTTP responses
3. **Never block the async runtime** — use `tokio::task::spawn_blocking` for serial I/O
4. The `execute_at` method must work for any string — whitelist enforcement is API-layer responsibility
5. When a capability is unsupported (e.g. GPS on a non-GPS modem), return `HardwareError::Unsupported`

### Mock Implementation Rules

`mock.rs` must implement every trait method. Return plausible fake data:
- Signal: `rsrp: -85, rsrq: -10, sinr: 15, rssi: -65, band: "B2", cell_id: "MOCK01"`
- Status: `connected: true, technology: "4G", operator: "Mock Carrier"`
- When adding a new trait method, add a mock implementation immediately in the same PR
- `live_device_path_handle`: the mock has no serial fd → returns None (the trait default); do not override.

---

## Types

### Input Types

```rust
struct ConnectionConfig {
    apn: String,         // validated: non-empty, max 100 chars
    username: Option<String>,
    password: Option<String>,
    auth_type: AuthType, // None | PAP | CHAP
    ip_type: IpType,     // Ipv4 | Ipv6 | Ipv4v6
    cid: u8,             // validated: 1-8
}

enum AuthType { None, Pap, Chap }
enum IpType { Ipv4, Ipv6, Ipv4v6 }
```

### Return Types

```rust
struct DeviceInfo {
    imei: String,
    manufacturer: String,
    model: String,
    firmware_version: String,
    supported_protocols: Vec<String>,   // "qmi", "mbim", "at", etc.
}

struct ModemStatus {
    connected: bool,
    technology: Option<String>,         // "2G", "3G", "4G", "5G"
    operator: Option<String>,
    signal_strength: u8,                // 0-100 normalized
    ip_address: Option<String>,
}

struct SignalInfo {
    rssi: f64,      // dBm
    rsrp: f64,      // dBm
    rsrq: f64,      // dB
    sinr: f64,      // dB
    band: String,   // e.g. "B2", "n78"
    cell_id: String,
}

struct ExtendedSignalInfo {
    primary: SignalInfo,
    secondary_cells: Vec<SignalInfo>,
    carrier_aggregation: bool,
    network_type: String,
}

struct AntennaMetrics {
    ports: Vec<AntennaPort>,
}

struct AntennaPort {
    port: u8,
    rsrp: f64,
    rsrq: f64,
    sinr: f64,
}

struct DataStats {
    bytes_tx: u64,
    bytes_rx: u64,
    session_uptime_secs: u64,
}

struct GpsInfo {
    latitude: f64,
    longitude: f64,
    altitude: Option<f64>,
    speed: Option<f64>,
    fix_type: String,
    satellites: u32,
    timestamp: String,  // ISO 8601
}

enum HardwareError {
    Timeout,
    PortNotFound,
    ParseError(String),
    Unsupported,         // use when feature not available on this modem
    AtError(String),     // modem returned ERROR or CME ERROR
    IoError(String),
}
```

---

## ModemEvent Enum

Events emitted to the global broadcast channel (`AppState.event_tx`).  
Hardware session **emits** these. Backend/API session **reads** them via WebSocket handler.

```rust
enum ModemEvent {
    /// Signal metrics update (broadcast every 2s by signal broadcaster task)
    SignalUpdate {
        modem_id: String,
        signal: SignalInfo,
    },

    /// Connection state change (emit on connect/disconnect)
    ConnectionState {
        modem_id: String,
        state: String,          // "connected", "disconnected", "connecting"
        network: Option<String>,
        ip: Option<String>,
    },

    /// Network registration status change
    RegistrationChange {
        modem_id: String,
        status: String,         // "registered", "searching", "denied", "not_registered"
        operator: Option<String>,
        tech: Option<String>,   // "4G", "5G", etc.
    },

    /// SIM card event
    SimEvent {
        modem_id: String,
        event: String,          // "inserted", "removed", "pin_required"
        state: String,
    },

    /// Modem health/availability state change
    ModemHealth(ModemHealth),

    /// WAN manager status update
    WanStatusUpdate {
        statuses: Vec<WanModemStatus>,
        active_primary: Option<String>,
    },

    /// AT command debug trace (visible in debug console UI)
    /// Use the global debug_trace() fn — no AppState needed
    DebugTrace {
        message: String,
    },

    /// Error notification
    Error {
        code: String,
        message: String,
    },
}
```

### Emitting Debug Traces

The hardware layer uses a global function to emit debug traces without needing AppState:

```rust
use crate::state::debug_trace;

debug_trace(format!("→ {}", cmd));       // outgoing AT command
debug_trace(format!("← {}", response)); // incoming response
```

This is safe to call from anywhere including inside the AT handler's send_command loop.

---

## ModemProfile (read-only from hardware layer)

The hardware layer reads profile data but never mutates it.  
Profiles are owned by `ModemContext.profile: Arc<ModemProfile>`.

### Key fields used by hardware layer

```
profile.commands.signal_cmd          → vendor-specific signal command (or None = use generic)
profile.commands.signal_parse_regex  → regex for parsing signal response
profile.commands.generic_signal_cmd  → fallback: "AT+CSQ"
profile.commands.iccid_cmd           → ICCID query command
profile.commands.registration_cmd    → registration check command
profile.port_mapping.at_port_preference     → preferred ttyUSB port names (ordered)
profile.port_mapping.at_interface_preference → preferred USB interface numbers (ordered)
profile.port_mapping.baud_rate       → serial baud rate (default 115200)
profile.capabilities.has_gps        → gate GPS operations
profile.capabilities.supports_5g    → informational
profile.dual_sim_config              → all dual-SIM AT commands and regex patterns
profile.apn_live_config              → live APN read/write (QICSGP) templates; None/None = CGDCONT fallback
```

### Live APN read/write — `apn_live_config: ApnLiveConfig`

Templates for reading/writing the *live* APN directly on a PDP context, used by
the APN/PDP panel (Item #42). Two `Option<String>` fields:

```
profile.apn_live_config.query   → read template, e.g. "AT+QICSGP={cid}"
profile.apn_live_config.write   → write template, e.g.
                                   AT+QICSGP={cid},{context_type},"{apn}","{username}","{password}",{auth}
```

`None`/`None` (the `Default`, used by Telit + generic) means the modem has no
QICSGP support and the backend must fall back to `AT+CGDCONT`. The 3 Quectel
profiles (RM551E-GL, RM520N-GL, RM500Q-GL) carry the templates above.

Placeholder semantics (filled by the backend at runtime):

| Placeholder      | Meaning                                                         |
|------------------|-----------------------------------------------------------------|
| `{cid}`          | literal PDP context id (e.g. `1`)                               |
| `{context_type}` | numeric Quectel context type from `IpType`: 1=IPv4, 2=IPv6, 3=IPv4v6 |
| `{apn}`          | literal APN string (inside the template quotes)                |
| `{username}`     | literal auth username (inside quotes)                          |
| `{password}`     | literal auth password (inside quotes)                          |
| `{auth}`         | numeric auth method from `AuthType`: 0=none, 1=PAP, 2=CHAP     |

Bench-confirmed field order (Phase 0, RM551E): `AT+QICSGP=1` returns
`+QICSGP: <context_type>,"<apn>","<user>","<pass>",<auth>`.

**Security:** the backend MUST NOT log the *filled* write command — it contains
the PDP password ("never log passwords"). Only the query (no secrets) may be
traced. Whitelist-wise, QICSGP queries on CIDs 1–8 validate as **Safe** (exact
match) and any QICSGP write validates as **RequiresConfirmation** (bare
`AT+QICSGP` prefix in `confirmation_commands`).

### Profile matching (in ProfileRegistry)

```
1. Check vendor_id + product_id (hex, leading-zero normalized)
2. If match found → use that profile
3. No match → use generic_modem() fallback (3GPP standard commands only)
```

Filesystem overrides: `/etc/modem-interface/profiles/*.toml` can add or replace profiles at runtime.

---

## Port Detection Algorithm

```
For each USB modem:
  1. Extract bus-port from sysfs (e.g. "4-1.1")
  2. Find all ttyUSB* devices with matching bus-port prefix
  3. Sort by interface number from sysfs INTERFACE=
  4. Match against profile.port_mapping.at_interface_preference
  5. Try each port in preference order:
     a. Open serial at profile baud_rate
     b. Send "AT\r\n"
     c. Wait up to 2s for "OK" response
     d. On success → this is the AT port
  6. If all preference ports fail → try ALL ttyUSB* for this modem (30s total)
  7. If still no response → mark modem as unavailable
```

Multi-modem note: Use `at_interface_preference` (interface number matching) not `at_port_preference` (absolute device name) — ttyUSB numbering is unpredictable with multiple modems.

---

## Connection Management

**AT+CGACT does NOT work reliably for ECM/QMI connections.**

Use flight mode sequence:

```
Disconnect:
  AT+CFUN=4   → flight mode (radio off, modem still responsive)
  wait ~500ms
  
Connect:
  AT+CFUN=1   → full functionality (radio on, modem re-registers)
  wait for +CEREG response or poll
```

APN is set via `AT+CGDCONT=<cid>,"<pdp_type>","<apn>"` while radio is off (CFUN=0/4).

### connect() vs reconnect()

Both cycle the radio via CFUN, but they differ in whether a new PDP context is written:

```
connect(config):    AT+CFUN=0 → AT+CGDCONT=<cid>,"<pdp_type>","<apn>" → AT+CFUN=1
                    (writes a NEW APN, then brings the bearer up)

reconnect():        AT+CFUN=0 → (no CGDCONT) → AT+CFUN=1
                    (re-uses whatever APN is already saved on the modem)
```

Use `reconnect()` when the saved APN is correct and only the bearer needs to be
re-established (e.g. recovery after a transient drop). Use `connect(config)` when
the APN itself is changing. Both set the session `connect_time` on success.

---

## Adding a New Modem Profile

1. Add a new `fn myvendor_model() -> ModemProfile { ... }` in `profiles.rs`
2. Add to `builtin_profiles()` vec
3. Set `identity.vendor_id` and `identity.product_id` (lowercase hex)
4. Set `port_mapping.at_interface_preference` (find AT port via `dmesg | grep ttyUSB`)
5. Test with `has_gps: false` and `band_mode_config.supported: false` until verified
6. Document in hardware session CLAUDE.md

New profiles can also be deployed as TOML files to `/etc/modem-interface/profiles/` without recompiling.
