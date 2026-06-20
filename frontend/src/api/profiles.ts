/**
 * Modem Profile API Functions
 *
 * Maps to:
 *   GET  /api/modem/profiles         - List all known profiles
 *   GET  /api/modem/profile/active   - Active modem's profile
 *   GET  /api/modem/detected         - All detected modems
 *   POST /api/modem/select           - Switch active modem
 *   POST /api/modem/profile/override - Apply different profile
 *   POST /api/modem/profile/request  - Generate profile request
 */

import { apiGet, apiPost } from './client';
import type {
  ModemProfileSummary,
  ActiveModemInfo,
  DetectedModemEnhanced,
  RescanResponse,
  ProfileRequestPayload,
  ProfileRequestResponse,
  DiscoveryResponse,
} from '@/types/profiles';

/** List all known modem profiles (built-in + filesystem). */
export async function getModemProfiles(): Promise<ModemProfileSummary[]> {
  return apiGet<ModemProfileSummary[]>('/modem/profiles');
}

/** Get the active modem's profile and detection info. */
export async function getActiveProfile(): Promise<ActiveModemInfo> {
  return apiGet<ActiveModemInfo>('/modem/profile/active');
}

/** List all detected modems with profile match status. */
export async function getDetectedModems(): Promise<DetectedModemEnhanced[]> {
  return apiGet<DetectedModemEnhanced[]>('/modem/detected');
}

/** Switch the active modem to a different detected modem. */
export async function selectModem(modemId: string): Promise<{ success: boolean; modem_id: string }> {
  return apiPost<{ success: boolean; modem_id: string }>('/modem/select', { modem_id: modemId });
}

/** Apply a different profile to the current modem. */
export async function overrideProfile(profileId: string): Promise<ActiveModemInfo> {
  return apiPost<ActiveModemInfo>('/modem/profile/override', { profile_id: profileId });
}

/** Generate a profile request (mailto link + clipboard text). */
export async function requestProfile(
  payload: ProfileRequestPayload
): Promise<ProfileRequestResponse> {
  return apiPost<ProfileRequestResponse>('/modem/profile/request', payload);
}

/** Re-scan USB for modems (hot-plug detection). */
export async function rescanModems(): Promise<RescanResponse> {
  return apiPost<RescanResponse>('/modem/rescan', {});
}

/** Run discovery probe on the active modem. */
export async function discoverModem(): Promise<DiscoveryResponse> {
  return apiPost<DiscoveryResponse>('/modem/discover', {});
}
