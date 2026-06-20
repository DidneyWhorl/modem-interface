/**
 * SIM API Functions
 *
 * Maps to:
 *   GET  /api/sim/status
 *   POST /api/sim/pin
 *   GET  /api/sim/slots
 *   GET  /api/sim/slots/config
 *   PUT  /api/sim/slots/config
 *   POST /api/sim/slots/switch
 */

import { apiGet, apiPost, apiPut } from './client';
import type {
  SimStatus, PinRequest, PinResult,
  DualSimInfo, SimSlotConfig, SimSlotSwitchRequest, SimSlotSwitchResult,
} from '@/types/api';

/**
 * Get SIM card status including state, ICCID, and operator info.
 * IMSI is only available when SIM is unlocked.
 */
export async function getSimStatus(): Promise<SimStatus> {
  return apiGet<SimStatus>('/sim/status');
}

/**
 * Perform PIN operations: verify, change, enable, or disable.
 */
export async function pinOperation(request: PinRequest): Promise<PinResult> {
  return apiPost<PinResult, PinRequest>('/sim/pin', request);
}

/**
 * Get dual SIM slot information: active slot, per-slot status and assigned profiles.
 */
export async function getSimSlots(): Promise<DualSimInfo> {
  return apiGet<DualSimInfo>('/sim/slots');
}

/**
 * Get per-slot APN profile assignments.
 */
export async function getSimSlotConfig(): Promise<SimSlotConfig> {
  return apiGet<SimSlotConfig>('/sim/slots/config');
}

/**
 * Update per-slot APN profile assignments.
 */
export async function updateSimSlotConfig(config: SimSlotConfig): Promise<SimSlotConfig> {
  return apiPut<SimSlotConfig, SimSlotConfig>('/sim/slots/config', config);
}

/**
 * Switch active SIM slot. Supports simple swap or full swap with profile apply + reboot.
 */
export async function switchSimSlot(request: SimSlotSwitchRequest): Promise<SimSlotSwitchResult> {
  return apiPost<SimSlotSwitchResult, SimSlotSwitchRequest>('/sim/slots/switch', request);
}
