/**
 * API Types for OpenWRT Modem Interface
 * 
 * These types define the contract between frontend and backend.
 * Any changes here must be synced with the Rust backend types.
 */

// ============================================================================
// Modem Types
// ============================================================================

export type NetworkTechnology = '2G' | '3G' | '4G' | '5G' | null;

export interface ModemStatus {
  connected: boolean;
  technology: NetworkTechnology;
  operator: string | null;
  signal_strength: number; // 0-100 normalized
  ip_address: string | null;
}

export interface SignalInfo {
  rssi: number;    // Received Signal Strength Indicator (dBm)
  rsrp: number;    // Reference Signal Received Power (dBm)
  rsrq: number;    // Reference Signal Received Quality (dB)
  sinr: number;    // Signal to Interference+Noise Ratio (dB)
  band: string;    // Current band (e.g., "B3", "n78")
  cell_id: string; // Serving cell identifier
}

export interface GpsInfo {
  latitude: number;
  longitude: number;
  altitude: number | null;
  speed: number | null;
  fix_type: string;
  satellites: number;
  timestamp: string;  // ISO 8601
}

export interface ExtendedSignalInfo {
  primary: SignalInfo;
  secondary_cells: SignalInfo[];
  carrier_aggregation: boolean;
  network_type: string;
}

export interface AntennaMetrics {
  ports: { port: number; rsrp: number; rsrq: number; sinr: number; technology?: string }[];
}

export interface ModemInfo {
  manufacturer: string;
  model: string;
  firmware_version: string;
  imei: string;
  supported_protocols: ProtocolType[];
}

export type ProtocolType = 'qmi' | 'mbim' | 'ecm' | 'mhi' | 'at';

export interface DetectedModem {
  device_path: string;
  protocol: ProtocolType;
  description: string;
  vendor_id: string | null;
  product_id: string | null;
  profile_id: string | null;
  has_profile: boolean;
}

// ============================================================================
// Connection Types
// ============================================================================

export type AuthType = 'none' | 'pap' | 'chap';
export type IpType = 'ipv4' | 'ipv6' | 'ipv4v6';

export interface ConnectionConfig {
  cid: number;
  apn: string;
  username?: string;
  password?: string;
  auth_type: AuthType;
  ip_type: IpType;
}

export interface ConnectionResult {
  success: boolean;
  ip_address?: string;
  gateway?: string;
  dns_servers?: string[];
  error?: string;
}

// ============================================================================
// SIM Types
// ============================================================================

export type SimState = 'ready' | 'pin_required' | 'puk_required' | 'error' | 'not_inserted';

export interface SimStatus {
  present: boolean;
  state: SimState;
  iccid: string | null;
  imsi: string | null;        // Only available when unlocked
  operator_name: string | null;
  pin_retries?: number;
  puk_retries?: number;
}

export type PinOperation = 'verify' | 'change' | 'enable' | 'disable';

export interface PinRequest {
  operation: PinOperation;
  pin: string;
  new_pin?: string; // For 'change' operation
}

export interface PinResult {
  success: boolean;
  error?: string;
  retries_remaining?: number;
}

// ============================================================================
// Network Types
// ============================================================================

export type RegistrationStatus = 
  | 'not_registered'
  | 'registered_home'
  | 'searching'
  | 'denied'
  | 'unknown'
  | 'registered_roaming';

export interface NetworkInfo {
  status: RegistrationStatus;
  operator: string | null;
  technology: NetworkTechnology;
  mcc: string | null; // Mobile Country Code
  mnc: string | null; // Mobile Network Code
}

export interface AvailableNetwork {
  operator: string;
  mcc: string;
  mnc: string;
  technology: NetworkTechnology;
  status: 'available' | 'current' | 'forbidden';
}

export interface NetworkScanResult {
  networks: AvailableNetwork[];
  scan_duration_ms: number;
}

// ============================================================================
// AT Command Types
// ============================================================================

export interface ATCommandRequest {
  command: string;
  timeout_ms?: number;
  /** Set to true to confirm execution of commands that require confirmation */
  confirmed?: boolean;
}

export interface ATCommandResponse {
  success: boolean;
  response: string;
  error?: string;
}

// ============================================================================
// AT Whitelist Types
// ============================================================================

export type CommandTier = 'safe' | 'confirmation' | 'blocked';
export type CommandSource = 'base' | 'profile' | 'custom';

export interface WhitelistEntry {
  command: string;
  tier: CommandTier;
  source: CommandSource;
  profile_name?: string;
  /** Short display label, e.g. "3GPP", "Sierra", "RM551", "Custom". */
  source_label: string;
  overridden: boolean;
}

export interface WhitelistOverrides {
  safe_commands: string[];
  confirmation_commands: string[];
  blocked_prefixes: string[];
  tier_overrides: Record<string, CommandTier>;
}

export interface MergedWhitelist {
  commands: WhitelistEntry[];
  overrides: WhitelistOverrides;
}

// ============================================================================
// Band & Mode Control Types
// ============================================================================

export interface BandSections {
  lte: boolean;
  nsa: boolean;
  sa: boolean;
}

export interface NetworkModeOption {
  id: string;
  label: string;
  mode_value: string;
  nr5g_disable_value: number | null;
  active_sections: BandSections;
}

export interface BandConfigResponse {
  // Profile metadata
  supported_modes: NetworkModeOption[];
  supported_lte_bands: number[];
  supported_nsa_bands: number[];
  supported_sa_bands: number[];
  supported_nrdc_bands: number[];
  has_nrdc: boolean;
  reboot_on_band_change: boolean;
  has_restore: boolean;
  // Current modem state
  active_mode_id: string | null;
  active_mode_raw: string | null;
  nr5g_disable_mode: number | null;
  active_lte_bands: number[];
  active_nsa_bands: number[];
  active_sa_bands: number[];
  active_nrdc_bands: number[];
  nrdc_enabled: boolean | null;
}

export interface BandConfigRequest {
  mode_id: string;
  lte_bands: number[];
  nsa_bands: number[];
  sa_bands: number[];
  nrdc_bands?: number[];
  nrdc_enabled?: boolean;
}

export interface BandConfigApplyResult {
  success: boolean;
  reboot_required: boolean;
  message: string;
}

// ============================================================================
// MBN Carrier Profile Types
// ============================================================================

export interface MbnProfile {
  index: number;
  selected: boolean;
  activated: boolean;
  name: string;
  version: string;
  revision: string;
}

export interface MbnSelectRequest {
  profile_name: string;
}

export interface MbnAutoSelectRequest {
  enabled: boolean;
}

export interface MbnActionResult {
  success: boolean;
  reboot_recommended: boolean;
  message: string;
}

// ============================================================================
// Configuration Types
// ============================================================================

export interface ModemConfig {
  connection: ConnectionConfig;
  auto_connect: boolean;
  preferred_bands: string[];
  signal_poll_interval: number;
}

// ============================================================================
// Modem Health / Power Control Types
// ============================================================================

export type ModemHealthState = 'ok' | 'unavailable' | 'rebooting' | 'error';

export interface ModemHealth {
  available: boolean;
  state: ModemHealthState;
  message: string | null;
}

// ============================================================================
// WebSocket Event Types
// ============================================================================

export type WebSocketEventType =
  | 'signal_update'
  | 'connection_state'
  | 'registration_change'
  | 'sim_event'
  | 'initial_status'
  | 'modem_health'
  | 'debug_trace'
  | 'wan_status_update'
  | 'speedtest_progress'
  | 'speedtest_complete'
  | 'speedtest_error'
  | 'error';

export interface SignalUpdateEvent {
  type: 'signal_update';
  modem_id: string;
  payload: SignalInfo;
}

export interface ConnectionStateEvent {
  type: 'connection_state';
  payload: {
    state: 'connecting' | 'connected' | 'disconnecting' | 'disconnected' | 'error';
    network: string | null;
    ip: string | null;
  };
};

export interface RegistrationChangeEvent {
  type: 'registration_change';
  payload: {
    status: RegistrationStatus;
    operator: string | null;
    tech: NetworkTechnology;
  };
}

export interface SimEventPayload {
  event: 'inserted' | 'removed' | 'locked' | 'unlocked' | 'error';
  state: SimState;
}

export interface SimEventMessage {
  type: 'sim_event';
  payload: SimEventPayload;
}

export interface InitialStatusEvent {
  type: 'initial_status';
  payload: ModemStatus;
}

export interface ErrorEvent {
  type: 'error';
  payload: {
    code: string;
    message: string;
  };
}

export interface ModemHealthEvent {
  type: 'modem_health';
  modem_id: string;
  payload: ModemHealth;
}

export interface DebugTraceEvent {
  type: 'debug_trace';
  payload: {
    message: string;
  };
}

export interface WanStatusUpdateEvent {
  type: 'wan_status_update';
  payload: WanStatusResponse;
}

export interface SpeedtestProgressEvent {
  type: 'speedtest_progress';
  payload: SpeedtestProgress;
}

export interface SpeedtestCompleteEvent {
  type: 'speedtest_complete';
  payload: SpeedtestResult;
}

export interface SpeedtestErrorEvent {
  type: 'speedtest_error';
  payload: {
    test_id: string;
    error: string;
  };
}

export type WebSocketEvent =
  | SignalUpdateEvent
  | ConnectionStateEvent
  | RegistrationChangeEvent
  | SimEventMessage
  | InitialStatusEvent
  | ModemHealthEvent
  | DebugTraceEvent
  | WanStatusUpdateEvent
  | SpeedtestProgressEvent
  | SpeedtestCompleteEvent
  | SpeedtestErrorEvent
  | ErrorEvent;

// ============================================================================
// APN Profiles — Saved Connection Presets
// ============================================================================

export interface ApnProfile {
  id: string;
  name: string;
  modem_profile_id: string;
  connection: ConnectionConfig;
  mbn_profile?: string;
  created_at: string;
  updated_at: string;
}

export interface ApnProfileRequest {
  name: string;
  modem_profile_id: string;
  connection: ConnectionConfig;
  mbn_profile?: string;
}

export interface ApnProfileApplyRequest {
  profile_id: string;
}

export interface ApnProfileApplyResult {
  success: boolean;
  /** Derived: a step_log line contains ERROR/Failed/Timeout. */
  had_errors: boolean;
  step_log: string[];
  /** true iff the saved profile's MBN differed → reboot. */
  reboot_triggered: boolean;
}

export interface ApnProfileImportResult {
  imported: number;
  skipped: number;
  message: string;
}

// ============================================================================
// Dual SIM Slot Types
// ============================================================================

export interface SimSlotStatus {
  slot: number;
  active: boolean;
  sim_status?: SimStatus;
  assigned_profile_id?: string;
  assigned_profile_name?: string;
}

export interface DualSimInfo {
  supported: boolean;
  dual_sim_disabled?: boolean;
  slot_count: number;
  active_slot: number;
  slots: SimSlotStatus[];
}

export interface SimSlotSwitchRequest {
  target_slot: number;
  apply_profile?: boolean;
}

export interface SimSlotSwitchResult {
  success: boolean;
  rebooting: boolean;
  message: string;
  steps: string[];
}

export interface SimSlotConfig {
  slot1_profile_id?: string;
  slot2_profile_id?: string;
  dual_sim_disabled?: boolean;
}

// ============================================================================
// WAN Manager Types
// ============================================================================

export type FirewallBackend = 'fw3' | 'fw4' | 'unknown';

export interface PlatformCapabilities {
  policy_routing_available: boolean;
  policy_routing_enabled: boolean;
  firewall_backend: FirewallBackend;
  mwan3_detected: boolean;
  openwrt_version: string | null;
}

export interface RoutingTableEntry {
  table_number: number;
  rule_priority: number;
  gateway: string | null;
  device: string;
  source_ip: string;
}

export type WanModemStatus = 'online' | 'offline' | 'checking' | 'standby' | 'no_sim';

export type WanModemState = 'active' | 'standby';

export type RoutingMode = 'failover' | 'load_balance';

export type WanEntryType = 'modem' | 'ethernet';

export interface WanModemEntry {
  modem_id: string;
  label: string;
  interface_name: string;
  network_device: string;
  state: WanModemState;
  metric: number;
  entry_type: WanEntryType;
  original_bridge: string | null;
  mtu: number | null;
  ttl: number | null;
  hop_limit: number | null;
  weight: number | null;
  /**
   * Optional UCI proto override (Item #37 sub-task 2). When set, the daemon
   * writes this exact value to `uci set network.<iface>.proto=...` instead
   * of picking based on detected USB-net mode. Free-form (UCI accepts any
   * string); typical values dhcp/qmi/mbim/static/pppoe. Length 1-32, no
   * whitespace. When null/undefined, the daemon picks automatically.
   *
   * Operator-facing label is "Protocol override" (mode-agnostic — never
   * names modem firmware modes per feedback_modem_mode_agnostic.md). This
   * is an Advanced expert-mode escape hatch.
   */
  proto_override?: string | null;
}

export interface WatchdogConfig {
  enabled: boolean;
  check_interval_secs: number;
  failure_threshold: number;
  ping_target: string;
  dns_target: string;
  http_target: string;
  log_retention_days: number;
  restart_on_failure: boolean;
  restart_cooldown_mins: number;
  max_restart_attempts: number;
}

export interface WanConfig {
  enabled: boolean;
  modem_priority: WanModemEntry[];
  watchdog: WatchdogConfig;
  failover_locked: boolean;
  failback_timer_mins: number;
  routing_mode: RoutingMode;
}

export interface WanHealthCheckResult {
  timestamp: string;
  ping_ok: boolean;
  dns_ok: boolean;
  dns_v4_ok: boolean;
  dns_v6_ok: boolean;
  http_ok: boolean;
  overall_ok: boolean;
}

export interface FailoverEvent {
  timestamp: string;
  from_modem_id: string;
  from_label: string;
  to_modem_id: string;
  to_label: string;
  reason: string;
}

export interface WanModemStatusEntry {
  modem_id: string;
  label: string;
  interface_name: string;
  network_device: string;
  state: WanModemState;
  metric: number;
  status: WanModemStatus;
  last_check: WanHealthCheckResult | null;
  consecutive_failures: number;
  is_primary: boolean;
  entry_type: WanEntryType;
  original_bridge: string | null;
  mtu: number | null;
  ttl: number | null;
  hop_limit: number | null;
  operator: string | null;
  imei: string | null;
  restart_suspended: boolean;
  restart_count: number;
  weight: number | null;
  /**
   * Detected USB-net mode of the cellular modem. DIAGNOSTIC ONLY — must NOT be
   * rendered on operator-facing UI per the mode-agnostic principle. Engineer-
   * facing surfaces (debug panels, curl/jq diagnostics) only. Type-safety only;
   * this file is the sole permitted frontend reference.
   */
  usbnet_mode?: 'ecm' | 'qmi' | 'mbim' | 'rmnet' | 'ncm' | 'rndis' | 'unknown';
  /**
   * Operator-set UCI `proto` override for this entry. Mirrored from the
   * corresponding `WanModemEntry`. `null` (or absent) means the daemon
   * picks based on detected USB-net mode. The auto-resolved value is
   * NOT surfaced — only the operator's own typed override is. Advanced
   * expert-mode field per `feedback_modem_mode_agnostic.md`.
   */
  proto_override?: string | null;
}

export interface FailoverOverrideInfo {
  active: boolean;
  original_primary_id: string;
  original_primary_label: string;
  current_primary_id: string;
  current_primary_label: string;
  failover_timestamp: string;
  stabilization_remaining_secs: number | null;
}

export interface WanStatusResponse {
  enabled: boolean;
  failover_locked: boolean;
  modems: WanModemStatusEntry[];
  watchdog: WatchdogConfig;
  failover_history: FailoverEvent[];
  failback_timer_mins: number;
  failover_override: FailoverOverrideInfo | null;
  platform?: PlatformCapabilities;
  routing_tables?: Record<string, RoutingTableEntry>;
  routing_mode: RoutingMode;
}

export interface AvailableEthernetPort {
  port_name: string;
  bridge: string;
  link_status: string;
}

export interface WanScanResponse extends WanStatusResponse {
  available_ethernet_ports: AvailableEthernetPort[];
}

export interface AddEthernetRequest {
  port_name: string;
  label?: string;
}

export interface WanWatchdogLogEntry {
  timestamp: string;
  action: string;
  details: string;
}

export interface WanWatchdogLogResponse {
  entries: WanWatchdogLogEntry[];
  last_recovery: WanWatchdogLogEntry | null;
  retention_days: number;
}

// ============================================================================
// Signal History Types
// ============================================================================

export interface SignalSample {
  ts: number;      // Unix epoch seconds
  rsrp: number;    // dBm (f32)
  rsrq: number;    // dB (f32)
  sinr: number;    // dB (f32)
}

export interface SignalHistory {
  modem_id: string;
  samples: SignalSample[];
}

export type SignalHistoryWindow = '1h' | '6h' | '24h';

// ============================================================================
// Speedtest Types
// ============================================================================

export type SpeedtestMode = 'quick' | 'medium' | 'full';

export type SpeedtestPhase = 'latency' | 'download' | 'upload';

export interface SpeedtestProgress {
  test_id: string;
  phase: SpeedtestPhase;
  progress_pct: number;
  current_speed_mbps: number;
  bytes_transferred: number;
  running_p90_mbps?: number;
  size_label?: string;
}

export interface ConnectionMetadata {
  ip?: string;
  colo?: string;
  city?: string;
  country?: string;
  asn?: number;
  asn_name?: string;
  latitude?: number;
  longitude?: number;
}

export interface AimScores {
  streaming: string;
  gaming: string;
  video_calls: string;
}

export interface MeasurementBreakdown {
  size_label: string;
  count: number;
  points_bps: number[];
}

export interface SpeedDataPoint {
  timestamp: number;
  speed: number;
  p90: number;
}

export interface SpeedtestResult {
  id: string;
  timestamp: string;
  mode: SpeedtestMode;
  wan_id: string;
  wan_name: string;
  interface: string;
  download_mbps: number;
  upload_mbps: number;
  latency_ms: number;
  jitter_ms: number;
  bytes_consumed: number;
  server: string;
  download_loaded_latency_ms?: number;
  download_loaded_jitter_ms?: number;
  upload_loaded_latency_ms?: number;
  upload_loaded_jitter_ms?: number;
  bufferbloat_ms?: number;
  connection?: ConnectionMetadata;
  scores?: AimScores;
  tcp_loss_ratio?: number;
  download_measurements?: MeasurementBreakdown[];
  upload_measurements?: MeasurementBreakdown[];
}

export interface SpeedtestHistoryResponse {
  results: SpeedtestResult[];
}

export interface SpeedtestStatusResponse {
  running: boolean;
}

export interface RunSpeedtestResponse {
  test_id: string;
}

// ============================================================================
// API Response Wrappers
// ============================================================================

export interface ApiResponse<T> {
  data: T;
  timestamp: number;
}

export interface ApiError {
  code: string;
  message: string;
  details?: Record<string, unknown>;
}

// ============================================================================
// APN Apply Types (Item #42 Phase 3 — POST /apn/apply)
// ============================================================================

/** Live current APN config read from the modem (QICSGP / CGDCONT). */
export interface CurrentApnConfig {
  /** Default editing context. null when all contexts are reserved (ims/sos) or none reported. */
  cid: number | null;
  apn: string;
  ip_type: IpType;
  auth_type: AuthType;
  /** Empty string when none configured or QICSGP unsupported. */
  username: string;
  /** true iff a non-empty password is stored; password value is never returned. */
  has_password: boolean;
}

export interface ApnApplyRequest {
  /** PDP context ID (1-8). */
  cid: number;
  /** APN string (1-100 chars). */
  apn: string;
  ip_type: IpType;
  auth_type: AuthType;
  username?: string;
  /** Omitted or null = leave stored password unchanged. */
  password?: string | null;
  /** Omitted = unchanged; null or "__auto__" = Auto; string = select that named profile. */
  mbn_profile?: string | null;
}

export interface ApnApplyResult {
  success: boolean;
  /** Derived: a step_log line records a failure (error/failed/timeout, case-insensitive). */
  had_errors: boolean;
  mbn_changed: boolean;
  rebooted: boolean;
  step_log: string[];
  message: string;
}

// ============================================================================
// License Types
// ============================================================================

export type LicenseState =
  | 'unlicensed'
  | 'valid'
  | 'expired'
  | 'invalid_signature'
  | 'device_mismatch';

/**
 * Reduced, PUBLIC license shape returned by the unauthenticated
 * `GET /license/status` route (L-01). Carries only `state` + `device_token` —
 * the activation screen's needs — without disclosing tier/expiry/user_id to
 * unauthenticated callers.
 */
export interface PublicLicenseStatus {
  state: LicenseState;
  device_token: string;
}

/**
 * Full license shape returned by the AUTHENTICATED `GET /license/detail` route
 * (and echoed by `POST /license/activate`). Includes the sensitive
 * tier/expiry/user_id fields used by the dashboard's profile display.
 */
export interface LicenseStatus extends PublicLicenseStatus {
  tier?: string;
  expires_at?: string;
  user_id?: string;
}
