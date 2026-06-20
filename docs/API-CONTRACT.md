# API Contract — modem-interface v1.0.100

**Owner:** PM/Architecture chat (this document is the source of truth)
**Consumers:** Frontend session (calls these), Backend/API session (implements these)
**Last updated:** 2026-06-19

> ⚠️ Sessions do not modify this file. Cross-boundary changes go to `PENDING-CHANGES.md`.

---

## Authentication

All endpoints except `/ctrl-modem/api/auth/*`, `/ctrl-modem/api/setup`, and `/ctrl-modem/api/license/*` require a valid session cookie.

```
Cookie: session=<token>
```

**Public (unauthenticated) routes shown on the login screen** return *reduced* shapes
that omit identifiers a logged-out client should not see:

| Public route | Reduced shape | Omits | Authenticated equivalent (full shape) |
|--------------|---------------|-------|----------------------------------------|
| `GET /api/modem/:modem_id/status` | `PublicModemStatus` | `ip_address` (H1) | `GET /api/modem/status` → `ModemStatus` |
| `GET /api/modem/:modem_id/signal` | `PublicSignalInfo` | `cell_id` (2026-06-19) | `GET /api/modem/signal` → `SignalInfo` |

`cell_id` is a coarsely-geolocatable serving-cell identifier; it is dropped from the
pre-auth `signal` response and remains available on the authenticated routes
(`/api/modem/signal` compat, `/api/modem/:id/signal/extended`, and the WebSocket
`signal` payload).

Unauthenticated requests → `401 Unauthorized`
Rate-limited requests → `429 Too Many Requests`
Modem unavailable (health check failed) → `503 Service Unavailable` + `Retry-After: N`
Lock timeout (modem busy) → `503 Service Unavailable` + `Retry-After: N`

---

## Operation Timeout Classes

| Class | Timeout | Used For |
|-------|---------|---------|
| Lock acquire | 2s | All routes — waiting for modem mutex |
| Quick | 5s | signal, status, info, gps, stats |
| State change | 15s | connect, disconnect, airplane, bands, MBN, power |
| Long | 60s | network_scan |

---

## Route Reference

### License

License endpoints are **public** — they bypass auth middleware, so license activation
can function before login. The license/portal is **optional** (v1.4.0-dev.2 pivot): an
unlicensed device has full local functionality. Activating a license only unlocks
cloud-dependent features (see the per-feature gates below).

#### `GET /api/license/status`
Return current license state and device token.

**Response:** `200 OK`
```typescript
LicenseStatusResponse = {
  state: "unlicensed" | "valid" | "expired" | "invalid_signature" | "device_mismatch";
  device_token: string;       // Base32 SHA-256 hardware fingerprint
  tier?: string;              // e.g. "pro", "enterprise" (present when valid/expired)
  expires_at?: string;        // ISO 8601 datetime (present when valid)
  user_id?: string;           // Portal user ID (present when valid/expired)
}
```

#### `POST /api/license/activate`
Validate a license key against the device token and store it on disk.

**Request:**
```typescript
{ license_key: string }       // Ed25519-signed key from portal
```

**Success Response:** `200 OK` — `LicenseStatusResponse` (state will be `"valid"`)

**Error Responses:**
| Status | Condition | Message |
|--------|-----------|---------|
| `400` | Empty key | `"License key is required"` |
| `400` | Bad signature | `"Invalid license key: signature verification failed"` |
| `400` | Wrong device | `"License key is for a different device"` |
| `400` | Past expiry | `"License key has expired"` |
| `400` | Malformed | `"Invalid license key format"` |
| `500` | Disk write fail | `"Failed to store license key"` |

#### No global license gate (removed v1.4.0-dev.2)

There is **no** blunt license gate on protected routes. The license/portal is optional —
unlicensed devices get full local API access; protected routes are governed by
**authentication** only (a valid session). The former `403 license_required` blanket
response on protected routes no longer exists.

Cloud-dependent features remain gated by their own **per-feature** license checks
(`LicenseState::has_feature(...)`), independent of authentication. Today the only such
feature is the remote-access tunnel (`has_feature("remote_access")` + `tunnel.enabled` —
see the Tunnel section). Future cloud features follow the same per-feature pattern.

---

### Tunnel

Remote access tunnel configuration. Requires authentication.

#### `GET /api/tunnel/config`
Return tunnel configuration and feature availability.

**Response:** `200 OK`
```typescript
{
  enabled: boolean;           // Whether tunnel client is enabled
  ports: number[];            // Allowed local ports for proxying (default [443, 8443])
  url: string;                // Portal tunnel endpoint URL
  feature_available: boolean; // Whether license includes remote_access feature
}
```

#### `PUT /api/tunnel/config`
Update tunnel configuration. Requires admin role.

**Request:**
```typescript
{
  enabled?: boolean;          // Toggle tunnel on/off
  ports?: number[];           // Update allowed port list (non-empty, no port 0)
}
```

**Success Response:** `200 OK` — same shape as GET response with updated values.

**Error Responses:**
| Status | Condition | Message |
|--------|-----------|---------|
| `400` | Empty ports array | `"At least one port must be configured"` |
| `400` | Port 0 in list | `"Port 0 is not valid"` |

---

### Modem Discovery

#### `GET /api/modems`
List all modems on the system.

**Response:** `200 OK`
```typescript
ModemListItem[] where ModemListItem = {
  id: string;              // "{VID}:{PID}:{USB_SERIAL}" e.g. "2c7c:0122:e3183572"
  detected: DetectedModem;
  discovery: DiscoveryInfo;
  health: ModemHealth;
  last_signal: SignalInfo | null;
}
```

> `detected.device_path` reflects the port the handler is actively using; after an automatic serial self-heal (USB re-enumeration) it is refreshed within one cache cycle (≤60s).

#### `GET /api/modem/detect`
Return detection metadata for all discovered modems (alias of above, no modem_id).

**Response:** `200 OK` — `DetectedModem[]`

> Each `device_path` reflects the live port within ≤60s of a self-heal (same reconcile as `GET /api/modems`).

---

### Per-Modem Routes

All routes below have the form `GET /api/modem/:modem_id/...`

The `:modem_id` is the stable USB serial-based ID: `{VID}:{PID}:{USB_SERIAL}`

#### `GET /api/modem/:modem_id/info`
Device identification (served from discovery cache — no AT commands).

**Response:** `200 OK`
```typescript
DeviceInfo = {
  imei: string;
  manufacturer: string;
  model: string;
  firmware_version: string;
  supported_protocols: ("qmi" | "mbim" | "mhi" | "at")[];
}
```

#### `GET /api/modem/:modem_id/status`
Current connection state and network info (served from 60-second cache).

**Response:** `200 OK`
```typescript
ModemStatus = {
  connected: boolean;
  technology: "2G" | "3G" | "4G" | "5G" | null;
  operator: string | null;
  signal_strength: number;   // 0-100 normalized
  ip_address: string | null;
}
```
**Error:** `503` if cache not yet populated (returns `Retry-After: 60`).

#### `GET /api/modem/:modem_id/signal`
Detailed signal metrics (served from 60-second cache).

> **PUBLIC route (login screen, unauthenticated).** Returns the **reduced**
> `PublicSignalInfo` shape — identical to `SignalInfo` but **without `cell_id`**
> (a coarsely-geolocatable serving-cell identifier). The full `SignalInfo`
> (including `cell_id`) is served on the authenticated compat route
> `GET /api/modem/signal`, on `GET /api/modem/:id/signal/extended`, and in the
> WebSocket `signal` payload.

**Response:** `200 OK`
```typescript
PublicSignalInfo = {
  rssi: number;       // dBm
  rsrp: number;       // dBm
  rsrq: number;       // dB
  sinr: number;       // dB
  band: string;       // e.g. "B14", "n78"
  technology: "2G" | "3G" | "4G" | "5G" | null;
  // cell_id intentionally omitted pre-auth
}
```
**Error:** `503` if cache not yet populated (returns `Retry-After: 60`).

The authenticated compat route returns the full shape:
```typescript
// GET /api/modem/signal  (authenticated)
SignalInfo = {
  rssi: number;       // dBm
  rsrp: number;       // dBm
  rsrq: number;       // dB
  sinr: number;       // dB
  band: string;       // e.g. "B14", "n78"
  cell_id: string;    // serving cell identifier
  technology: "2G" | "3G" | "4G" | "5G" | null;
}
```

#### `GET /api/modem/:modem_id/signal/history`
Signal quality history from in-memory ring buffer (up to 24h at 60s intervals).
No hardware call — reads from state only. Data resets on service restart.

**Query Parameters:**
| Param | Values | Default | Description |
|-------|--------|---------|-------------|
| `window` | `1h`, `6h`, `24h` | `1h` | Time window to return |

**Response:** `200 OK`
```typescript
SignalHistory = {
  modem_id: string;
  samples: SignalSample[];
}

SignalSample = {
  ts: number;      // Unix epoch seconds
  rsrp: number;    // dBm (f32)
  rsrq: number;    // dB (f32)
  sinr: number;    // dB (f32)
}
```

**Notes:**
- Returns up to 60 samples (1h), 360 (6h), or 1440 (24h)
- Empty `samples` array on fresh boot (no data yet)
- Compat route: `GET /api/modem/signal/history` (uses selected/first modem)

#### `GET /api/modem/:modem_id/signal/extended`
Extended signal: carrier aggregation, neighbour cells.

**Response:** `200 OK` — `ExtendedSignalInfo`
```typescript
ExtendedSignalInfo = {
  primary: SignalInfo;
  secondary_cells: SignalInfo[];
  carrier_aggregation: boolean;
  network_type: string;
}
```

#### `GET /api/modem/:modem_id/signal/antenna`
Per-RX-port antenna metrics (direct AT command — not cached).

**Response:** `200 OK` — `AntennaMetrics`
```typescript
AntennaMetrics = {
  ports: {
    port: number;         // 0-3 per technology (resets for each tech)
    rsrp: number;         // dBm
    rsrq: number;         // dB
    sinr: number;         // dB
    technology?: string;  // "LTE" | "NR5G-NSA" | "NR5G-SA" (omitted for legacy)
  }[];
}
```

**Multi-Technology Support (v1.0.22+):**
- Each port now includes an optional `technology` field
- Port numbering **resets per technology** (e.g., LTE has RX0-RX3, then NR5G-NSA has RX0-RX1)
- Legacy single-technology responses omit the technology field
- Typical scenarios:
  - **LTE-only**: 4 ports without technology field
  - **NR5G-SA**: 2-4 ports with `technology: "NR5G-SA"`
  - **NR5G-NSA (dual-connectivity)**: 4 LTE ports + 1-2 NR5G-NSA ports

#### `GET /api/modem/:modem_id/stats`
Data usage statistics for the current session.

**Response:** `200 OK`
```typescript
DataStats = {
  bytes_tx: number;
  bytes_rx: number;
  session_uptime_secs: number;
}
```

#### `GET /api/modem/:modem_id/health`
Per-modem health/availability state (read from ModemContext.health, no hardware call).

**Response:** `200 OK`
```typescript
ModemHealth = {
  available: boolean;
  state: "ok" | "rebooting" | "unavailable" | "error";
  message: string | null;
}
```

#### `GET /api/modem/:modem_id/gps`
GPS position (only for capable modems, served from 60-second cache when panel active).

**Response:** `200 OK`
```typescript
GpsInfo = {
  latitude: number;
  longitude: number;
  altitude: number | null;
  speed: number | null;
  fix_type: string;
  satellites: number;
  timestamp: string;  // ISO 8601
}
```
**Error:** `400` if GPS not supported by modem profile.

#### `POST /api/modem/:modem_id/gps/stop`
Stop the GPS engine.

**Response:** `200 OK` — `{ success: boolean }`

#### `GET /api/modem/:modem_id/pdp`
PDP context details + MBN carrier profiles + live current APN config + per-context active flags.

Issues `AT+CGDCONT?`, `AT+CGACT?`, and (on Quectel modems with `apn_live_config.query`) `AT+QICSGP=<cid>`.

**Response:** `200 OK`
```typescript
{
  pdp_contexts: {
    cid: string;
    pdp_type: string;
    apn: string;
    active: boolean;    // from AT+CGACT? (true = context is active/connected)
  }[];
  mbn_config: string;
  mbn_profiles: MbnProfile[];
  mbn_auto_select: boolean | null;
  mbn_selected_profile: string | null;
  mbn_supported: boolean;
  current_config: {
    cid: number | null;       // default editing context: lowest CGDCONT CID whose APN ∉ {ims, sos}.
                              // null when all contexts are reserved or modem reports none.
    apn: string;
    ip_type: "ipv4" | "ipv6" | "ipv4v6";  // from CGDCONT pdp_type (works for all modems)
    auth_type: "none" | "pap" | "chap";   // from QICSGP on Quectel; "none" on Telit/generic
    username: string;                      // "" when none configured or QICSGP unsupported
    has_password: boolean;                 // true iff QICSGP reports a non-empty password field.
                                           // Password VALUE is never returned.
  };
}
```

**Notes:**
- `current_config.ip_type` is derived from `CGDCONT pdp_type` for all modems (not from QICSGP `context_type`). Telit/generic modems have full `ip_type` coverage.
- `current_config.auth_type`, `username`, and `has_password` are blank/false when the modem profile has no `apn_live_config.query` template (Telit/generic).
- `current_config.cid` being `null` is a valid state (e.g., modem in factory state with only `ims`/`sos` contexts).
- Security: the password value from QICSGP is never surfaced; only `has_password` is returned.

---

### On-Demand Refresh Endpoints

These endpoints bypass the 60-second cache and issue direct AT commands.
They are called by panel refresh buttons — never called automatically.
Each returns fresh data and updates the cache as a side effect.

| Endpoint | Updates |
|----------|---------|
| `POST /api/modem/:modem_id/signal/refresh` | SignalInfo cache |
| `POST /api/modem/:modem_id/status/refresh` | ModemStatus cache |
| `POST /api/modem/:modem_id/device/refresh` | DeviceInfo discovery cache |
| `POST /api/modem/:modem_id/sim/refresh` | SIM discovery cache |
| `POST /api/modem/:modem_id/gps/refresh` | GPS cache |
| `POST /api/modem/:modem_id/registration/refresh` | Registration cache |

Each returns the refreshed data in the same shape as the corresponding GET endpoint.

> **Authorization (intentional read-class — decided 2026-06-19).** These six
> `*_refresh` routes — and the `POST /api/gps/panel` gate below — require a valid
> session but **deliberately do not require admin / a write-tier role.** They issue
> **read-class** AT queries (the same data the 60-second cache already reads) and
> write only into the read cache; they perform no modem configuration change. This
> is the manual-troubleshooting "refresh = bypass-cache read" escape hatch, available
> to any authenticated user including ReadOnly. This is a recorded product decision,
> not an oversight: gating them behind admin would break manual refresh for read-tier
> operators with no security gain (a ReadOnly user can already see the cached values).
> Mutating routes (connect/disconnect, APN writes, reboot, raw-AT) remain admin-gated.

---

### GPS Panel Gate

#### `POST /api/gps/panel`
Gate GPS polling in the 60-second cache cycle.
GPS AT commands are only issued during the cache cycle when the panel is active.

**Request:** `{ active: boolean }`
**Response:** `200 OK` — `{ active: boolean }`

---

### Connect / Disconnect

#### `POST /api/modem/:modem_id/connect`
Establish data connection.

**Request:**
```typescript
ConnectionConfig = {
  apn: string;           // required, 1-100 chars
  username?: string;
  password?: string;
  auth_type: "none" | "pap" | "chap";
  ip_type: "ipv4" | "ipv6" | "ipv4v6";
  cid: number;           // 1-8
}
```
**Response:** `200 OK` — `ModemStatus` (post-connect)
**Error:** `400` invalid APN/CID, `503` timeout

> ⚠️ Uses AT+CFUN flight mode sequence (AT+CFUN=4 → AT+CFUN=1), NOT AT+CGACT.
> Required for ECM/QMI connections — standard PDP context commands do not work.

#### `POST /api/modem/:modem_id/disconnect`
Terminate data connection.

**Response:** `200 OK` — `ModemStatus` (post-disconnect)

#### `POST /api/modem/:modem_id/reconnect`
Re-establish the data bearer using the APN **already saved on the modem**.

Performs a pure radio cycle (`AT+CFUN=0` → wait ~1 s → `AT+CFUN=1`) with **no**
`AT+CGDCONT` write. Distinct from `connect`, which writes a new PDP context before
cycling the radio. Use when the saved APN is correct and only the bearer needs
to be re-established (e.g. recovery after a transient drop).

**Request:** none (no body required)

**Response:** `200 OK` — `ModemStatus` (post-reconnect, bearer up)

**Side-effect:** broadcasts a `ConnectionState { state: "connected" }` WebSocket event
immediately on success, providing UI feedback without waiting for the 60 s cache cycle.

**Errors:** `404` modem not found, `503` modem busy or reconnect timed out (15 s)

**Auth:** required (same route group as connect/disconnect)

**Compat route:** `POST /api/modem/reconnect` — operates on the selected or first modem.

> Item #42 Phase 2 — APN/PDP panel redesign. Added v1.3.0-dev.21.

#### `POST /api/modem/:modem_id/apn/apply`
Diff-aware APN apply. Writes the APN/IP/auth/username/password (and optionally
the MBN carrier profile) for a PDP context, choosing the lightest operation that
satisfies the request.

**Request:**
```typescript
{
  cid: number;                          // PDP context ID, 1-8
  apn: string;                          // required, 1-100 chars
  ip_type: "ipv4" | "ipv6" | "ipv4v6";  // required
  auth_type: "none" | "pap" | "chap";   // required ("none" is valid)
  username?: string;                    // optional
  password?: string | null;             // omitted/null = leave stored password unchanged
  mbn_profile?: string | null;          // omitted = unchanged; null or "__auto__" = Auto;
                                         //   string = select that profile
}
```

The `mbn_profile` field is **three-state**:
- **omitted** — leave the MBN selection unchanged.
- **`null`** or **`"__auto__"`** — set MBN to Auto (`AT+QMBNCFG="AutoSel",1`).
- **a profile name** — select that specific profile.

**Response:** `200 OK`
```typescript
{
  success: boolean;
  had_errors: boolean;    // derived: a step_log line records a failure (error/failed/timeout, case-insensitive)
  mbn_changed: boolean;   // true iff the MBN selection differed and was rewritten
  rebooted: boolean;      // true iff the modem was rebooted (MBN-change path only)
  step_log: string[];     // human-readable step labels (never contains the password)
  message: string;        // operator-facing summary
}
```

**Behavior:**
- **MBN unchanged** (or modem reports `mbn_supported=false`): live-write the APN/auth
  via `AT+QICSGP` (Quectel) or `AT+CGDCONT` (Telit/generic fallback — APN + IP only,
  no auth). **No radio cycle, no reboot.** `rebooted=false`,
  `message` = "Saved — click Reconnect to apply to the live link."
- **MBN changed** (Auto↔profile or profile↔profile): runs the profile's
  `apn_apply_config` steps (`AT+QMBNCFG="AutoSel",0` + `"Select","<profile>"` for a
  specific profile, or `"AutoSel",1` for Auto) + the live write + reboot
  (`AT+CFUN=1,1`). `rebooted=true`, `mbn_changed=true`. Broadcasts a
  `ModemHealth { state: "rebooting" }` WebSocket event.
- **Nothing changed:** the server is **idempotent** — it executes the (harmless)
  same-value live write and returns `success=true`, `mbn_changed=false`,
  `rebooted=false`. The frontend disables Apply when the form is not dirty.

**Password rule (§11):** when `password` is omitted/null, the modem's current
`AT+QICSGP=<cid>` is re-read and the existing password is re-supplied to the write
(so an untouched placeholder does not clear it). When `password` is provided
(including `""`), the provided value is used. **The password is never logged,
audit-logged, returned, or placed in `step_log`** — only a redacted write label is
recorded.

**Validation:** `400` when APN is empty or > 100 chars, or `cid` is outside 1-8.

**Errors:** `400` validation, `404` modem not found, `503` modem busy / unavailable /
timed out.

**Auth:** required (same route group as connect/disconnect/apn-profiles).

> Item #42 Phase 2 — APN/PDP panel redesign. Added v1.3.0-dev.21.

---

### AT Command Execution

#### `POST /api/modem/:modem_id/command`
Execute a whitelisted AT command.

**Request:**
```typescript
AtCommandRequest = {
  command: string;
  confirmed?: boolean;   // required true for Tier 2 (confirmation) commands
}
```
**Response:** `200 OK`
```typescript
AtCommandResponse = {
  command: string;
  response: string;
  success: boolean;
}
```
**Errors:**
- `403` — command is blocked (Tier 3)
- `428` — command requires confirmation (`confirmed: true` not set)

---

### Power Control

#### `POST /api/modem/:modem_id/power-down`
Graceful power down via `AT+QPOWD=1`. Modem reboots automatically.

**Response:** `200 OK` — `{ success: boolean, message: string }`

#### `POST /api/modem/:modem_id/reboot`
Reboot via `AT+CFUN=1,1`. USB interfaces disappear ~15-30s then re-enumerate.

**Response:** `200 OK` — `{ success: boolean, message: string }`

#### `GET /api/modem/:modem_id/airplane`
Query current airplane mode (CFUN) state.

**Response:** `200 OK` — `{ airplane_mode: boolean }`

#### `POST /api/modem/:modem_id/airplane`
Toggle airplane mode.

**Request:** `{ enabled: boolean }`
→ `true` = radio off (`AT+CFUN=0`)
→ `false` = radio on (`AT+CFUN=1`)

**Response:** `200 OK` — `{ success: boolean, airplane_mode: boolean }`

---

### Band & Mode Configuration

#### `GET /api/modem/:modem_id/bands`
Current band lock + mode + profile's supported bands.

**Response:** `200 OK` — `BandConfigResponse`
```typescript
BandConfigResponse = {
  supported_modes: NetworkModeOption[];
  supported_lte_bands: number[];
  supported_nsa_bands: number[];
  supported_sa_bands: number[];
  supported_nrdc_bands: number[];
  has_nrdc: boolean;
  reboot_on_band_change: boolean;
  has_restore: boolean;
  active_mode_id: string | null;
  active_mode_raw: string | null;
  nr5g_disable_mode: number | null;
  active_lte_bands: number[];
  active_nsa_bands: number[];
  active_sa_bands: number[];
  active_nrdc_bands: number[];
  nrdc_enabled: boolean | null;
}
```
**Error:** `400` if band control not supported by modem profile.

#### `POST /api/modem/:modem_id/bands`
Apply band lock and mode.

**Request:** `BandConfigRequest`
```typescript
BandConfigRequest = {
  mode_id: string;
  lte_bands: number[];
  nsa_bands: number[];
  sa_bands: number[];
  nrdc_bands?: number[];
  nrdc_enabled?: boolean;
}
```
**Response:** `200 OK` — `{ success: boolean, reboot_required: boolean, message: string }`

#### `POST /api/modem/:modem_id/bands/restore`
Restore all bands to factory default.

**Response:** `200 OK` — `{ success: boolean, message: string }`
**Error:** `400` if restore command not in profile.

---

### AT Whitelist Management

Requires: **Admin+ role** + `"at-whitelist"` feature permission.

#### `GET /api/modem/:modem_id/whitelist`
Full merged whitelist (base + profile additions + runtime overrides).

**Response:** `200 OK` — `MergedWhitelist`
```typescript
MergedWhitelist = {
  safe: string[];
  confirmation: string[];
  blocked_prefixes: string[];
  profile_name: string;
  profile_label: string;
  overrides: WhitelistOverrides;
}
```

#### `PUT /api/modem/:modem_id/whitelist`
Update runtime whitelist overrides (persisted to disk).

**Request:** `WhitelistOverrides`
**Response:** `200 OK` — `MergedWhitelist` (new merged view)

---

### MBN Carrier Profile Management

#### `POST /api/modem/:modem_id/mbn/select`
**Request:** `MbnSelectRequest = { profile_name: string }`
**Response:** `200 OK` — `MbnActionResult`

#### `POST /api/modem/:modem_id/mbn/deactivate`
**Response:** `200 OK` — `MbnActionResult`

#### `POST /api/modem/:modem_id/mbn/auto-select`
**Request:** `MbnAutoSelectRequest = { enabled: boolean }`
**Response:** `200 OK` — `MbnActionResult`

```typescript
MbnActionResult = {
  success: boolean;
  reboot_recommended: boolean;
  message: string;
}
```

---

### APN Profiles

#### `GET /api/modem/:modem_id/apn-profiles`
**Response:** `200 OK` — `ApnProfile[]`

#### `POST /api/modem/:modem_id/apn-profiles`
**Response:** `201 Created` — `ApnProfile`

#### `PUT /api/modem/:modem_id/apn-profiles/:id`
**Response:** `200 OK` — `ApnProfile`

> **Password field semantics (`connection.password`, v1.3.0-dev.55+).** The field
> is `Option<String>` and three-state, mirroring the §11 apply rule so that an
> untouched (masked) password is never silently dropped:
> - **omitted/`null`** = unedited → **create** captures the modem's live PDP
>   password for `connection.cid` (best-effort re-read of `AT+QICSGP=<cid>` via the
>   profile's `apn_live_config.query`; falls back to empty if the modem is
>   unavailable or lacks a query template); **update** preserves the existing
>   stored profile password.
> - **`""`** = explicit clear (open APN / no password).
> - **a value** = explicit password.
>
> The frontend "Save as Custom" omits the field when the password is unedited.
> The captured/preserved value is never logged, audit-logged, or placed in a
> step log.

#### `DELETE /api/modem/:modem_id/apn-profiles/:id`
**Response:** `200 OK` — `{ success: boolean }`

#### `POST /api/modem/:modem_id/apn-profiles/apply`
**Request:** `{ profile_id: string }`
**Response:** `200 OK` — `ApnProfileApplyResult`
```typescript
ApnProfileApplyResult = {
  success: boolean;
  had_errors: boolean;        // derived: a step_log line records a failure (error/failed/timeout, case-insensitive)
  step_log: string[];
  reboot_triggered: boolean;  // true iff the saved profile's MBN differed → reboot
}
```

Applies a **saved** APN profile. Routes through the same diff-aware apply core as
`POST /apn/apply` (above): the modem reboots **only if the saved profile's MBN
selection differs from the modem's current MBN state**. An APN-only change (or a
saved profile whose MBN already matches the current selection) is a live write
with **no radio cycle and no reboot** (`reboot_triggered=false`).

A saved profile always expresses a *definite* MBN intent (it can never mean
"leave MBN unchanged"): `mbn_profile=null` → Auto (`AT+QMBNCFG="AutoSel",1`),
`mbn_profile="<name>"` → select that profile. The profile's stored password is
re-supplied to the live write directly (never logged, returned, or audit-logged).

The profile's connection is also persisted to the global on-disk config so the
reconnect/watchdog APN enforcement applies it on the next reconnect.

> **Behavior change (Item #42 Phase 2, Task 6, v1.3.0-dev.21+):** this endpoint
> previously **always rebooted** and returned `400` for modems whose profile
> reported `apn_apply_config.supported=false`. It is now diff-aware (reboots only
> on an MBN change) and the `400` gate is **relaxed** — a modem lacking
> `apn_apply_config`/MBN support can still apply via the `AT+CGDCONT` live-write
> fallback (live write only, no reboot). The `ApnProfileApplyResult` response
> shape is unchanged.

---

### Backward-Compatibility Routes

These routes operate on `selected_modem_id` (or lexicographically first modem if none selected).
Used by the frontend in Phase 1. Per-modem routes (`/ctrl-modem/api/modem/:id/...`) are Phase 2.

| Compat Route | Delegates To | Notes |
|-------------|-------------|-------|
| `GET /api/modem/status` | `GET /api/modem/:id/status` | |
| `GET /api/modem/signal` | `GET /api/modem/:id/signal` | |
| `GET /api/modem/info` | `GET /api/modem/:id/info` | |
| `GET /api/modem/gps` | `GET /api/modem/:id/gps` | |
| `GET /api/modem/pdp` | `GET /api/modem/:id/pdp` | |
| `POST /api/modem/signal/refresh` | `POST /api/modem/:id/signal/refresh` | |
| `POST /api/modem/status/refresh` | `POST /api/modem/:id/status/refresh` | |
| `POST /api/modem/device/refresh` | `POST /api/modem/:id/device/refresh` | |
| `POST /api/modem/sim/refresh` | `POST /api/modem/:id/sim/refresh` | |
| `POST /api/modem/gps/refresh` | `POST /api/modem/:id/gps/refresh` | |
| `POST /api/modem/registration/refresh` | `POST /api/modem/:id/registration/refresh` | |
| `GET /api/modem/signal/history` | `GET /api/modem/:id/signal/history` | Query param: `?window=1h\|6h\|24h` |
| `GET /api/modem/signal/antenna` | `GET /api/modem/:id/signal/antenna` | ⚠️ PENDING — not yet implemented (item #20) |
| `POST /api/modem/connect` | `POST /api/modem/:id/connect` | |
| `POST /api/modem/disconnect` | `POST /api/modem/:id/disconnect` | |
| `POST /api/modem/reconnect` | `POST /api/modem/:id/reconnect` | Item #42 Phase 2 |
| `GET /api/sim/status` | per-modem sim status | |
| `GET /api/config` | per-modem config | |
| `POST /api/modem/select` | Sets `selected_modem_id` | Request: `{ modem_id: string }` |
| `GET /api/modem/profile/active` | Returns profile for selected modem | |
| `POST /api/modem/command` | `POST /api/modem/:id/command` | Uses selected/first modem |

---

## Speed Test

### POST /api/speedtest/run

Start a speed test on a specific WAN interface.

**Request:**
```json
{ "mode": "quick" | "full", "wan_id": "2c7c:0122:e3183572" }
```

**Response (202):**
```json
{ "test_id": "uuid" }
```

**Errors:** 400 (invalid wan_id, no network device), 409 (test already running)

Test runs asynchronously. Progress streamed via WebSocket `speedtest_progress` events.

**Modes:**
| | Quick | Full |
|---|---|---|
| Streams | 1 | 6 parallel |
| Data ~consumed | 10-20 MB | 75-150 MB |
| Duration | ~15s | ~30-45s |

### POST /api/speedtest/run-sync

Start a speedtest and block until completion. Returns the full result directly.
Designed for portal tunnel proxy — no WebSocket streaming needed.

> **Authorization (gate (a), shipped `v1.4.0-dev.5` 2026-06-19).** This route stays in
> the **public** route group (so the portal-through-tunnel relay, which carries no router
> session, is not rejected by the auth middleware), but the handler now **self-gates**:
> the speedtest proceeds only if **EITHER** the request source IP is loopback (`127.0.0.0/8`
> or `::1`) **OR** the request carries a valid router session. The tunnel relay forwards
> portal requests via `reqwest` to `127.0.0.1`, so a legitimate portal-through-tunnel call
> (and any on-device caller) is loopback; a raw LAN client cannot forge a loopback source,
> so an unauthenticated LAN request is now rejected with **401** — closing the
> cellular-data-burn hole. It remains bounded by a single-run lock (**409** if already
> running) and the `#[cfg(feature="tunnel")]` build gate.
>
> **CSRF note (gate (a)):** the protected-API CSRF Origin/Referer middleware also exempts
> loopback-source requests, so portal-through-tunnel **mutations** to protected routes pass
> regardless of the relayed `Origin` (a browser-driven CSRF attack never originates from
> loopback). See `docs/superpowers/specs/2026-06-19-gate-a-csrf-tunnel-bench-verify-speedtest-gate-design.md`.

**Request:**
```json
{ "mode": "quick", "wan_id": "2c7c:0122:e3183572" }
```

**Response (200):**
```json
{
  "id": "uuid",
  "timestamp": "2026-04-08T12:00:00Z",
  "mode": "quick",
  "wan_id": "2c7c:0122:e3183572",
  "wan_name": "Quectel RM551E",
  "interface": "wwan0",
  "download_mbps": 95.2,
  "upload_mbps": 42.1,
  "latency_ms": 12.3,
  "jitter_ms": 1.8,
  "bytes_consumed": 15000000,
  "server": "speed.cloudflare.com"
}
```

**Errors:** 404 (WAN not found), 409 (test already running), 500 (engine error)

**Timeout:** Quick ~15s, Full ~45s. Server enforces 90s hard timeout.

### GET /api/speedtest/status

**Response:**
```json
{ "running": true | false }
```

### GET /api/speedtest/history

**Query params:** `?wan_id=X&limit=10`

**Response:**
```json
{
  "results": [
    {
      "id": "uuid",
      "timestamp": "2026-04-07T12:00:00Z",
      "mode": "quick",
      "wan_id": "2c7c:0122:e3183572",
      "wan_name": "RM551E-GL",
      "interface": "usb0",
      "download_mbps": 142.3,
      "upload_mbps": 45.7,
      "latency_ms": 12.5,
      "jitter_ms": 2.1,
      "bytes_consumed": 15000000,
      "server": "cloudflare"
    }
  ]
}
```

---

## WebSocket Events

**Endpoint:** `GET /api/events` (upgrades to WebSocket)

### Authentication Handshake
Before any events are received, the client must authenticate:
1. Client connects to `GET /api/events`
2. Client sends: `{ "type": "auth", "token": "<ws_token>" }`
   (Token obtained via `POST /api/auth/ws-token`)
3. Server responds with `initial_status` event on success
4. Connection closes on auth failure

### Event Reference

All events are JSON objects with a `type` discriminator field.
Per-modem events include a top-level `modem_id` field.

```typescript
// Sent once after successful WS auth — NOT a ModemStatus object
// payload contains modem inventory only
{
  type: "initial_status";
  payload: {
    modem_count: number;
    modem_ids: string[];
  }
}

// Signal update — broadcast every 60s per modem from cache refresh task
// modem_id identifies which modem this data belongs to
// Frontend MUST filter by active modem_id before writing to cache
{
  type: "signal_update";
  modem_id: string;
  payload: SignalInfo;   // { rssi, rsrp, rsrq, sinr, band, cell_id, technology }
}

// Connection state change
{
  type: "connection_state";
  modem_id: string;
  state: string;
  network: string | null;
  ip: string | null;
}

// Network registration change
{
  type: "registration_change";
  modem_id: string;
  status: string;
  operator: string | null;
  tech: string | null;
}

// SIM event (insert/remove/pin change)
{
  type: "sim_event";
  modem_id: string;
  event: string;
  state: string;
}

// Modem health change (unavailable, rebooting, ok)
{
  type: "modem_health";
  modem_id: string;
  available: boolean;
  state: string;
  message: string | null;
}

// WAN manager health check result
{
  type: "wan_status_update";
  statuses: WanModemStatus[];
  active_primary: string | null;
}

// Error notification
{
  type: "error";
  code: string;
  message: string;
}

// AT command debug trace (visible in Debug Console panel)
{
  type: "debug_trace";
  payload: { message: string }
}

// Speed test progress (emitted every ~500ms during a test)
{
  type: "speedtest_progress";
  payload: {
    test_id: string;
    phase: "latency" | "download" | "upload";
    progress_pct: number;       // 0-100
    current_speed_mbps: number;
    bytes_transferred: number;
  }
}

// Speed test completed
{
  type: "speedtest_complete";
  payload: SpeedtestResult;  // same shape as GET /api/speedtest/history items
}

// Speed test error
{
  type: "speedtest_error";
  payload: { test_id: string; error: string }
}
```

> The 60-second cache refresh task runs continuously regardless of whether
> any WebSocket clients are connected. The cache is always warm for REST endpoints.

---

## Error Response Format

All errors return JSON:
```typescript
{
  error: string;    // human-readable message
  code?: string;    // machine-readable error code (optional)
}
```

| HTTP Status | Meaning |
|-------------|---------|
| `400` | Bad request (validation failed) |
| `401` | Unauthenticated |
| `403` | Forbidden (insufficient role) |
| `404` | Modem or resource not found |
| `428` | Precondition required (AT command needs `confirmed: true`) |
| `429` | Rate limited |
| `500` | Internal server error |
| `503` | Modem unavailable or lock timeout — check `Retry-After` header |

---

### CTRL-WAN Routing Modes (Level 3, v1.0.112)

The WAN manager supports two routing modes for default (non-steered) traffic:

**Failover (default):** Single default route to the highest-priority active WAN. On failure, switches to next healthy WAN.

**Load Balance:** Multipath default route distributes traffic across all active WANs proportionally by weight. Uses kernel L4 hashing (5-tuple: src IP, dst IP, src port, dst port, protocol) so each connection sticks to one WAN. Level 2 steering rules take priority over the default route.

#### New fields in `GET /api/wan/status` response:

```jsonc
{
  "enabled": true,
  "routing_mode": "failover",  // "failover" | "load_balance" (default: "failover")
  "modems": [
    {
      "modem_id": "2c7c:0122:e3183572",
      "weight": 3,             // Load balance weight 1-10, null = default (1)
      // ... existing fields ...
    }
  ]
  // ... existing fields ...
}
```

#### New fields in `PUT /api/wan/config` request:

```jsonc
{
  "routing_mode": "load_balance",  // optional, default "failover"
  "modem_priority": [
    {
      "modem_id": "2c7c:0122:e3183572",
      "weight": 3,                  // optional, 1-10, default 1
      // ... existing fields ...
    }
  ]
  // ... existing fields ...
}
```

| Field | Type | Validation |
|-------|------|------------|
| `routing_mode` | `"failover" \| "load_balance"` | Optional, defaults to `"failover"` |
| `weight` | `number \| null` | Per-modem, optional, range 1-10, default 1 |

---

### Modem USB-Net Mode Detection (Item #37 sub-task 1, v1.3.0-dev cycle)

The daemon detects each modem's USB-net mode (ECM / QMI / MBIM / RmNet / NCM /
RNDIS) once at boot via a profile-declared AT command (e.g. `AT+QCFG="usbnet"`
on Quectel, `AT#USBCFG?` on Telit). The detected mode is cached on the modem's
context and exposed via `GET /api/wan/status` for engineer-facing diagnostic use.

> **DIAGNOSTIC ONLY — DO NOT SURFACE IN OPERATOR UI.** Per the mode-agnostic
> principle (`feedback_modem_mode_agnostic.md`), USB-net mode is daemon-internal
> implementation detail, never a feature dimension shown to operators. All WAN
> cards, panel headers, status pills, Connect-button labels, and operator-visible
> strings MUST stay mode-independent. Frontend renders MUST NOT reference this
> field. Engineer-facing surfaces (debug-trace WS panel, this API, journald
> structured logs) MAY name modes directly.

#### New field in `GET /api/wan/status` response (per modem entry):

```jsonc
{
  "modems": [
    {
      "modem_id": "2c7c:0122:e3183572",
      // ... existing fields ...
      "usbnet_mode": "rmnet"   // optional — see below
    }
  ]
}
```

| Field | Type | Notes |
|-------|------|-------|
| `usbnet_mode` | `"ecm" \| "qmi" \| "mbim" \| "rmnet" \| "ncm" \| "rndis" \| "unknown"` (optional) | Detected USB-net mode of the cellular modem. Omitted (`null`/missing) for Ethernet entries (no modem to query). `"unknown"` means detection ran but the AT response could not be mapped (timeout, AT ERROR, parser fail, or profile lacked a detection command). DIAGNOSTIC ONLY — do not render in operator UI. |

A new WebSocket event `usb_net_mode_detected` is broadcast once per modem at
boot for the debug-trace panel:

```jsonc
{
  "type": "usb_net_mode_detected",
  "modem_id": "2c7c:0122:e3183572",
  "payload": { "mode": "rmnet" }
}
```

This event is also engineer-facing only.

---

### Modem Mode-to-Proto Mapping (Item #37 sub-task 2, v1.3.0-dev cycle)

Once the daemon knows the modem's USB-net mode (sub-task 1), it derives the
correct UCI `proto` value at `reconcile_uci_section` time and writes it via
`uci set network.<iface>.proto=...`. The operator does nothing — Save & Apply
on the CTRL-WAN page produces the right proto automatically.

**Mode-to-proto mapping:**

| `usbnet_mode` | UCI `proto` written | Rationale |
|---|---|---|
| `ecm` | `"dhcp"` | Modem runs DHCP server on its USB iface; `udhcpc` works directly. |
| `ncm` | `"dhcp"` | NCM is structurally similar to ECM — link-layer DHCP. |
| `rndis` | `"dhcp"` | Microsoft RNDIS — same DHCP-on-link semantic. |
| `qmi` | `"qmi"` | OpenWrt `proto-qmi` calls `uqmi --start-network`; control-plane IP. |
| `rmnet` | `"qmi"` | Quectel raw-IP mode is QMI-managed. |
| `mbim` | `"mbim"` | OpenWrt `proto-mbim` calls `umbim`. |
| `unknown` | `"dhcp"` | Backwards-compat fallback — never break a working ECM modem because detection returned Unknown. |

Ethernet WAN entries always resolve to `"dhcp"` (cellular mode is irrelevant
for a wired port) unless an explicit override is set (see below).

#### New field in `PUT /api/wan/config` request (`modem_priority` items):

```jsonc
{
  "modem_priority": [
    {
      "modem_id": "2c7c:0122:e3183572",
      // ... existing fields ...
      "proto_override": "static"   // optional — Advanced operator escape hatch
    }
  ]
}
```

| Field | Type | Validation |
|---|---|---|
| `proto_override` | `string \| null` (optional) | UCI `proto` value to write for this entry, overriding the daemon-computed default. Free-form (UCI accepts any string); typical values: `"dhcp"`, `"qmi"`, `"mbim"`, `"static"`, `"pppoe"`. Length 1-32, no whitespace. When omitted/null, the daemon picks based on the modem's detected USB-net mode (see sub-task 1). DIAGNOSTIC NOTE: the operator override exists primarily for testing and edge-case modems where auto-detection maps incorrectly. |

#### New field in `GET /api/wan/status` response (per modem entry):

```jsonc
{
  "modems": [
    {
      "modem_id": "2c7c:0122:e3183572",
      // ... existing fields ...
      "proto_override": "qmi"   // optional — operator's typed override echoed back
    }
  ]
}
```

| Field | Type | Notes |
|-------|------|-------|
| `proto_override` | `string \| null` (optional) | Operator-set UCI `proto` override mirrored from the corresponding `WanModemEntry`. Omitted/absent (or `null`) when no override is set — daemon picks based on detected USB-net mode and the auto-resolved value is daemon-internal and is NOT surfaced. Validation rules apply on PUT only (see PUT subsection above). |

The operator's typed `proto_override` IS surfaced in the `GET /api/wan/status`
response (see the GET subsection above), so the WAN UI form input can echo it
back to the operator. The daemon's auto-resolved proto (when no override is
set) remains daemon-internal and is **NOT** surfaced — mode-agnostic boundary.
Operators see their configured override only; no diff information about the
auto-resolved value is exposed.

> **Runtime mode-change limitation:** USB-net mode is read once at
> `ModemContext` creation and never re-polled. If an operator switches mode
> at runtime via direct AT command (`AT+QCFG="usbnet",2,1` plus reboot), the
> daemon won't detect the new mode until the next `ModemContext` recreation
> (USB re-plug, daemon restart, profile rescan). Manual workaround:
> `/etc/init.d/modem-interface restart` after a mode change.

> **Save & Apply trigger:** all 5 `reconcile_uci_section` callsites are
> inside auth-protected handlers (`update_wan_config` PUT, `scan_wan` POST).
> A daemon restart alone does NOT fire the reconcile loop. Bench
> verification of the mode-derived proto write must trigger a Save & Apply
> via the CTRL-WAN UI.

---

### UCI Device Path Semantics (Item #37 sub-task 2b, v1.3.0-dev cycle)

The UCI `option device` value the daemon writes during reconcile depends on
the resolved `proto` (sub-task 2):

| `proto`                                         | UCI `option device` value             | Source                                                         |
|-------------------------------------------------|---------------------------------------|----------------------------------------------------------------|
| `dhcp` / `static` / `pppoe` / `ppp` / `none`    | network interface name (e.g. `wwan0`) | `entry.network_device`                                         |
| `qmi` / `mbim`                                  | control device path (e.g. `/dev/cdc-wdm0`) | sysfs lookup of `/sys/class/usbmisc/cdc-wdm*` by USB bus-port |

When `proto` requires a control device path but no `cdc-wdm*` device is found
for the modem's USB bus-port (kernel binding race, ECM/NCM/RNDIS modem with a
`proto_override="qmi"` operator assertion against detection, etc.), the daemon
falls back to writing the network interface name and logs an info-level
diagnostic. Proto-qmi/mbim netifd handlers will then fail bring-up with `"The
specified control device does not exist"` — same failure mode as pre-2b
builds.

This resolution is daemon-internal — the resolved `option device` value is
**not** surfaced in any API response. Operators see their configured override
via `proto_override` (sub-task 2) and the daemon's auto-default; they do not
configure or see the device-path resolution directly.

**Operator override interaction:** if `proto_override="qmi"` is set on a
modem entry whose kernel did NOT bind `qmi_wwan` or `cdc_mbim`, no cdc-wdm
device is found and the daemon falls back to the netif. Bring-up will fail.
This is the operator-asserts-mode-against-detection case — failure surfaces
in OpenWrt netifd logs.

**Collision detection** keys on the netif name (`target_device`), NOT on the
control-device path. Two UCI sections both binding `wwan0` are treated as a
collision regardless of how either of them spells `option device` — the
authoritative L2-binding identity is the netif.

> **Bench impact:** sub-task 2b retires the manual `uci set network.<iface>.device='/dev/cdc-wdm0'` patch for QMI/MBIM modems on the M01K43-PMOD bench. Sub-task 2c (v1.3.0-dev.9) retired the requirement to click Scan to apply `proto_override` or `network_device` changes on existing modem entries — Save & Apply alone is now sufficient for all proto-affecting field changes.

---

### Save & Apply Reconcile-on-Existing (Item #37 sub-task 2c, v1.3.0-dev cycle)

`POST /api/wan/config` (Save & Apply) writes UCI for all proto-affecting
field changes on existing modem entries — `proto_override` flips and
`network_device` changes both reach UCI immediately, without requiring
a separate `POST /api/wan/scan` (Scan) click.

**Pre-2c behavior:** the existing-entry diff branch only emitted
`uci set ... metric` and `uci set ... mtu` for fast-path field changes
(state, metric, MTU). Proto-affecting changes (`proto_override`,
`network_device`) persisted to `wan-config.json` but UCI was untouched
until the next Scan reconciled all entries unconditionally. This was
the operator workaround documented in the sub-task 2 release notes.

**Post-2c behavior:** when the diff predicate detects a proto-affecting
field change on a modem entry, `reconcile_uci_section` fires
immediately. The full network reload (`/etc/init.d/network reload`)
also fires so netifd picks up the new proto handler.

**Edge cases preserved:**
- State change (active↔standby) without other diffs → fast-path
  `uci_set_metric` only; no full reload (existing behavior).
- Metric-only or MTU-only changes → fast-path `uci_set_metric` /
  `uci_set_mtu` only; no full reload (existing behavior).
- Ethernet bridge-conversion entries → existing bridge-conversion
  branch fires; sub-task 2c's branch does NOT (Ethernet entries are
  excluded from the predicate).
- Modem disconnected since last save → reconcile fires anyway with
  fallback values (`UsbNetMode::Unknown` → `proto="dhcp"` when no
  override is set; if `proto_override` IS set, the override wins),
  `control_device_path=None` → netif. The disconnected-modem case
  auto-heals on next Scan or modem reconnect.

**USB hot-plug between saves (cached `usbnet_mode` shifts):** the diff
predicate compares operator-changed fields only, NOT the snapshot's
resolved values. If a modem hot-replugs between saves and its cached
`usbnet_mode` changes, the operator must click Scan to trigger an
unconditional reconcile of all existing entries. This is the
documented escape hatch.

---

### Traffic Steering (Level 2, v1.0.110)

#### `GET /api/wan/steering`

List all steering rules ordered by priority.

```typescript
// Response
{
  rules: SteeringRule[];
  firewall_backend: string;  // "fw3" | "fw4" | "unknown"
}
```

#### `POST /api/wan/steering`

Create a new steering rule (appended to end of priority list).

```typescript
// Request
{
  name: string;              // required
  enabled?: boolean;         // default true
  source_ip?: string[] | null;
  destination_ip?: string[] | null;
  protocol?: "tcp" | "udp" | "icmp" | null;
  destination_port?: number | [number, number] | null;
  source_port?: number | [number, number] | null;
  target_wan: string;        // required — modem_id
  failover_mode?: "automatic" | "preferred_fallback" | "strict";
  fallback_wan?: string | null;
}
// Response: SteeringRule (with id, priority, status, fwmark assigned)
```

#### `PUT /api/wan/steering/:id`

Update an existing steering rule. All fields optional (partial update).

```typescript
// Request — same shape as create, all fields optional
// Response: SteeringRule
```

#### `DELETE /api/wan/steering/:id`

Delete a steering rule. Returns `204 No Content`.

#### `PUT /api/wan/steering/reorder`

Reorder steering rules. First-match-wins priority.

```typescript
// Request
{ order: string[] }  // ordered array of rule IDs (must include all)
// Response: SteeringListResponse
```

#### SteeringRule Type

```typescript
{
  id: string;
  name: string;
  enabled: boolean;
  priority: number;          // 900-949
  source_ip: string[] | null;
  destination_ip: string[] | null;
  protocol: "tcp" | "udp" | "icmp" | null;
  destination_port: number | [number, number] | null;
  source_port: number | [number, number] | null;
  target_wan: string;
  target_wan_label: string | null;
  failover_mode: "automatic" | "preferred_fallback" | "strict";
  fallback_wan: string | null;
  status: "active" | "dormant" | "blocked";  // runtime
  fwmark: number;                             // runtime
}
