/**
 * Modem API Functions
 * 
 * Maps to:
 *   GET  /api/modem/detect
 *   GET  /api/modem/status
 *   GET  /api/modem/signal
 *   POST /api/modem/connect
 *   POST /api/modem/disconnect
 *   POST /api/modem/command
 */

import { apiGet, apiPost, apiPut, apiDelete } from './client';
import type {
  DetectedModem,
  ModemStatus,
  ModemHealth,
  SignalInfo,
  GpsInfo,
  SimStatus,
  ExtendedSignalInfo,
  AntennaMetrics,
  ConnectionConfig,
  ConnectionResult,
  ATCommandRequest,
  ATCommandResponse,
  MergedWhitelist,
  WhitelistOverrides,
  BandConfigResponse,
  BandConfigRequest,
  BandConfigApplyResult,
  MbnProfile,
  MbnSelectRequest,
  MbnAutoSelectRequest,
  MbnActionResult,
  ApnProfile,
  ApnProfileRequest,
  ApnProfileApplyRequest,
  ApnProfileApplyResult,
  ApnProfileImportResult,
  SignalHistory,
  SignalHistoryWindow,
  CurrentApnConfig,
  ApnApplyRequest,
  ApnApplyResult,
} from '@/types/api';

/**
 * Detect connected modems and their available protocols.
 * Returns list of detected modems with device paths and protocol info.
 */
export async function detectModems(): Promise<DetectedModem[]> {
  return apiGet<DetectedModem[]>('/modem/detect');
}

/**
 * Get current modem status including connection state and network info.
 */
export async function getModemStatus(): Promise<ModemStatus> {
  return apiGet<ModemStatus>('/modem/status');
}

/**
 * Get detailed signal metrics.
 * Returns RSSI, RSRP, RSRQ, SINR, band info, and cell ID.
 */
export async function getSignalInfo(): Promise<SignalInfo> {
  return apiGet<SignalInfo>('/modem/signal');
}

/**
 * Get current GPS position data.
 * Only available on modems with GPS capability (e.g. Quectel RM551E-GL).
 * Also starts the GPS engine if not already running.
 */
export async function getGpsInfo(): Promise<GpsInfo> {
  return apiGet<GpsInfo>('/modem/gps');
}

/**
 * Stop the GPS engine (AT+QGPSEND).
 */
export async function stopGps(modemId: string): Promise<{ success: boolean }> {
  return apiPost<{ success: boolean }>(`/modem/${modemId}/gps/stop`);
}

/**
 * Get PDP context details and MBN config from modem.
 */
export async function getPdpDetails(): Promise<PdpDetails> {
  return apiGet<PdpDetails>('/modem/pdp');
}

export interface PdpDetails {
  pdp_contexts: {
    cid: string;
    pdp_type: string;
    apn: string;
    /** true = context active/connected (from AT+CGACT?). */
    active: boolean;
  }[];
  mbn_config: string;
  // Structured MBN carrier profile data
  mbn_profiles: MbnProfile[];
  mbn_auto_select: boolean | null;
  mbn_selected_profile: string | null;
  mbn_supported: boolean;
  /** Live current APN config read from the modem. */
  current_config: CurrentApnConfig;
}

/**
 * Get extended signal info: carrier aggregation, network detail, neighbour cells.
 */
export async function getExtendedSignalInfo(): Promise<ExtendedSignalInfo> {
  return apiGet<ExtendedSignalInfo>('/modem/signal/extended');
}

/**
 * Get per-antenna-port signal metrics (RSRP, RSRQ, SINR per RX port).
 */
export async function getAntennaMetrics(): Promise<AntennaMetrics> {
  return apiGet<AntennaMetrics>('/modem/signal/antenna');
}

/**
 * Establish a data connection with the given APN configuration.
 */
export async function connect(config: ConnectionConfig): Promise<ConnectionResult> {
  return apiPost<ConnectionResult, ConnectionConfig>('/modem/connect', config);
}

/**
 * Terminate the current data connection.
 */
export async function disconnect(): Promise<{ success: boolean }> {
  return apiPost<{ success: boolean }>('/modem/disconnect');
}

/**
 * Execute a whitelisted AT command.
 * Note: Only safe commands are allowed by the backend.
 * Privileged commands require confirmation.
 */
export async function executeATCommand(
  request: ATCommandRequest
): Promise<ATCommandResponse> {
  return apiPost<ATCommandResponse, ATCommandRequest>('/modem/command', request);
}

// ============================================================================
// Modem Power Control
// ============================================================================

/** Get current modem health/availability state. */
export async function getModemHealth(modemId: string): Promise<ModemHealth> {
  return apiGet<ModemHealth>(`/modem/${modemId}/health`);
}

/** Gentle reboot the modem (AT+QPOWD=1). Modem shuts down gracefully and boots back up. */
export async function powerDownModem(modemId: string): Promise<{ success: boolean; message: string }> {
  return apiPost<{ success: boolean; message: string }>(`/modem/${modemId}/power-down`);
}

/** Reboot the modem (AT+CFUN=1,1). Will auto-reconnect after ~30s. */
export async function rebootModem(modemId: string): Promise<{ success: boolean; message: string }> {
  return apiPost<{ success: boolean; message: string }>(`/modem/${modemId}/reboot`);
}

/** Get current airplane mode state (queries AT+CFUN? without changing it). */
export async function getAirplaneMode(modemId: string): Promise<{ airplane_mode: boolean }> {
  return apiGet<{ airplane_mode: boolean }>(`/modem/${modemId}/airplane`);
}

/** Toggle airplane mode. enabled=true: radio off, enabled=false: radio on. */
export async function setAirplaneMode(modemId: string, enabled: boolean): Promise<{ success: boolean; airplane_mode: boolean }> {
  return apiPost<{ success: boolean; airplane_mode: boolean }>(`/modem/${modemId}/airplane`, { enabled });
}

// ============================================================================
// Band & Mode Control
// ============================================================================

/** Get current band lock and mode configuration from the modem. */
export async function getBandConfig(modemId: string): Promise<BandConfigResponse> {
  return apiGet<BandConfigResponse>(`/modem/${modemId}/bands`);
}

/** Apply band lock and mode changes. */
export async function setBandConfig(modemId: string, config: BandConfigRequest): Promise<BandConfigApplyResult> {
  return apiPost<BandConfigApplyResult, BandConfigRequest>(`/modem/${modemId}/bands`, config);
}

/** Restore all bands to factory default. */
export async function restoreBands(modemId: string): Promise<{ success: boolean; message: string }> {
  return apiPost<{ success: boolean; message: string }>(`/modem/${modemId}/bands/restore`);
}

// ============================================================================
// AT Whitelist Management
// ============================================================================

/** Get the full merged AT command whitelist (base + profile + custom). */
export async function getWhitelist(modemId: string): Promise<MergedWhitelist> {
  return apiGet<MergedWhitelist>(`/modem/${modemId}/whitelist`);
}

/** Update AT whitelist overrides. Returns the new merged view. */
export async function updateWhitelist(modemId: string, overrides: WhitelistOverrides): Promise<MergedWhitelist> {
  return apiPut<MergedWhitelist, WhitelistOverrides>(`/modem/${modemId}/whitelist`, overrides);
}

// ============================================================================
// MBN Carrier Profile Management
// ============================================================================

/** Select an MBN carrier profile by name. */
export async function selectMbnProfile(modemId: string, req: MbnSelectRequest): Promise<MbnActionResult> {
  return apiPost<MbnActionResult, MbnSelectRequest>(`/modem/${modemId}/mbn/select`, req);
}

/** Deactivate the currently active MBN carrier profile. */
export async function deactivateMbnProfile(modemId: string): Promise<MbnActionResult> {
  return apiPost<MbnActionResult>(`/modem/${modemId}/mbn/deactivate`);
}

/** Toggle MBN auto-select (modem picks carrier profile based on SIM). */
export async function setMbnAutoSelect(modemId: string, req: MbnAutoSelectRequest): Promise<MbnActionResult> {
  return apiPost<MbnActionResult, MbnAutoSelectRequest>(`/modem/${modemId}/mbn/auto-select`, req);
}

// ============================================================================
// APN Profile Management
// ============================================================================

/** List APN profiles for the active modem. */
export async function getApnProfiles(modemId: string): Promise<ApnProfile[]> {
  return apiGet<ApnProfile[]>(`/modem/${modemId}/apn-profiles`);
}

/** Create a new APN profile. */
export async function createApnProfile(modemId: string, req: ApnProfileRequest): Promise<ApnProfile> {
  return apiPost<ApnProfile, ApnProfileRequest>(`/modem/${modemId}/apn-profiles`, req);
}

/** Update an existing APN profile. */
export async function updateApnProfile(modemId: string, id: string, req: ApnProfileRequest): Promise<ApnProfile> {
  return apiPut<ApnProfile, ApnProfileRequest>(`/modem/${modemId}/apn-profiles/${id}`, req);
}

/** Delete an APN profile. */
export async function deleteApnProfile(modemId: string, id: string): Promise<{ success: boolean }> {
  return apiDelete<{ success: boolean }>(`/modem/${modemId}/apn-profiles/${id}`);
}

/** Apply an APN profile (MBN selection + APN config + reboot). */
export async function applyApnProfile(modemId: string, req: ApnProfileApplyRequest): Promise<ApnProfileApplyResult> {
  return apiPost<ApnProfileApplyResult, ApnProfileApplyRequest>(`/modem/${modemId}/apn-profiles/apply`, req);
}

/** Export all APN profiles (for building pre-loaded profiles). */
export async function exportApnProfiles(modemId: string): Promise<ApnProfile[]> {
  return apiGet<ApnProfile[]>(`/modem/${modemId}/apn-profiles/export`);
}

/** Import APN profiles from a JSON array. Skips duplicates by name. */
export async function importApnProfiles(modemId: string, profiles: ApnProfileRequest[]): Promise<ApnProfileImportResult> {
  return apiPost<ApnProfileImportResult, ApnProfileRequest[]>(`/modem/${modemId}/apn-profiles/import`, profiles);
}

// ============================================================================
// APN Apply + Reconnect (Item #42 Phase 3)
// ============================================================================

/**
 * Cycle the modem radio (AT+CFUN=4 → AT+CFUN=1) to force re-registration.
 * No APN change — use when the connection needs a hard reset without reconfiguring.
 */
export async function reconnect(modemId: string): Promise<ModemStatus> {
  return apiPost<ModemStatus>(`/modem/${modemId}/reconnect`);
}

/**
 * Apply APN settings diff-aware: writes CGDCONT/QICSGP only for changed fields,
 * optionally selects an MBN carrier profile, and reboots only when required.
 */
export async function applyApn(modemId: string, request: ApnApplyRequest): Promise<ApnApplyResult> {
  return apiPost<ApnApplyResult, ApnApplyRequest>(`/modem/${modemId}/apn/apply`, request);
}

// ============================================================================
// On-Demand Refresh (bypass 60s cache, hit hardware directly)
// ============================================================================

/** Force-refresh signal metrics from hardware. */
export async function refreshSignal(modemId: string): Promise<SignalInfo> {
  return apiPost<SignalInfo>(`/modem/${modemId}/signal/refresh`);
}

/** Force-refresh modem status from hardware. */
export async function refreshStatus(modemId: string): Promise<ModemStatus> {
  return apiPost<ModemStatus>(`/modem/${modemId}/status/refresh`);
}

/** Force-refresh device info from hardware. */
export async function refreshDevice(modemId: string): Promise<unknown> {
  // TODO: export DeviceInfo from useDeviceInfo.ts and use as type param here
  return apiPost<unknown>(`/modem/${modemId}/device/refresh`);
}

/** Force-refresh SIM status from hardware. */
export async function refreshSim(modemId: string): Promise<SimStatus> {
  return apiPost<SimStatus>(`/modem/${modemId}/sim/refresh`);
}

/** Force-refresh GPS position from hardware. */
export async function refreshGps(modemId: string): Promise<GpsInfo> {
  return apiPost<GpsInfo>(`/modem/${modemId}/gps/refresh`);
}

/**
 * Get signal history samples for trending charts.
 * Window: '1h' | '6h' | '24h' (default: '1h')
 */
export async function getSignalHistory(window: SignalHistoryWindow = '1h'): Promise<SignalHistory> {
  return apiGet<SignalHistory>(`/modem/signal/history?window=${window}`);
}
