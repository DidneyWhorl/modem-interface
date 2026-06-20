/**
 * Modem Profile Hooks
 *
 * React Query hooks for modem profiles, active profile, and detected modems.
 */

import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import {
  getModemProfiles,
  getActiveProfile,
  getDetectedModems,
  selectModem,
  overrideProfile,
  requestProfile,
  rescanModems,
  discoverModem,
} from '@/api/profiles';
import type {
  ModemProfileSummary,
  ActiveModemInfo,
  DetectedModemEnhanced,
  RescanResponse,
  ProfileRequestPayload,
  ProfileRequestResponse,
  DiscoveryResponse,
} from '@/types/profiles';
import { deviceInfoQueryKey } from './useDeviceInfo';
import { modemStatusQueryKey } from './useModemStatus';
import { signalQueryKey } from './useSignal';
import { simStatusQueryKey } from './useSimStatus';
import { gpsQueryKey } from './useGps';
import { extendedSignalQueryKey } from './useExtendedSignal';
import { antennaMetricsQueryKey } from './useAntennaMetrics';
import { configQueryKey } from './useConfig';

// ============================================================================
// Query Keys
// ============================================================================

export const modemProfilesQueryKey = ['modem', 'profiles'] as const;
export const activeProfileQueryKey = ['modem', 'profile', 'active'] as const;
export const detectedModemsQueryKey = ['modem', 'detected'] as const;

// ============================================================================
// Query Hooks
// ============================================================================

/** List all known modem profiles. Rarely changes — long stale time. */
export function useModemProfiles() {
  return useQuery<ModemProfileSummary[]>({
    queryKey: modemProfilesQueryKey,
    queryFn: getModemProfiles,
    staleTime: Infinity,
    refetchOnWindowFocus: false,
  });
}

/** Get the active modem's profile and detection info. */
export function useActiveProfile() {
  return useQuery<ActiveModemInfo>({
    queryKey: activeProfileQueryKey,
    queryFn: getActiveProfile,
    staleTime: 60_000, // 1 minute
    refetchOnWindowFocus: false,
  });
}

/** List all detected modems with profile match status. */
export function useDetectedModems() {
  return useQuery<DetectedModemEnhanced[]>({
    queryKey: detectedModemsQueryKey,
    queryFn: getDetectedModems,
    staleTime: 60_000,
    refetchOnWindowFocus: false,
  });
}

// ============================================================================
// Mutation Hooks
// ============================================================================

/** Switch the active modem. Invalidates all modem queries on success. */
export function useSelectModem() {
  const queryClient = useQueryClient();

  return useMutation<{ success: boolean; modem_id: string }, Error, string>({
    mutationFn: selectModem,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: activeProfileQueryKey });
      queryClient.invalidateQueries({ queryKey: deviceInfoQueryKey });
      queryClient.invalidateQueries({ queryKey: modemStatusQueryKey });
      queryClient.invalidateQueries({ queryKey: signalQueryKey });
      queryClient.invalidateQueries({ queryKey: simStatusQueryKey });
      queryClient.invalidateQueries({ queryKey: gpsQueryKey });
      queryClient.invalidateQueries({ queryKey: extendedSignalQueryKey });
      queryClient.invalidateQueries({ queryKey: antennaMetricsQueryKey });
      queryClient.invalidateQueries({ queryKey: configQueryKey });
    },
  });
}

/** Apply a different profile to the current modem. */
export function useOverrideProfile() {
  const queryClient = useQueryClient();

  return useMutation<ActiveModemInfo, Error, string>({
    mutationFn: overrideProfile,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: activeProfileQueryKey });
      queryClient.invalidateQueries({ queryKey: deviceInfoQueryKey });
    },
  });
}

/** Generate a profile request (mailto + clipboard text). */
export function useRequestProfile() {
  return useMutation<ProfileRequestResponse, Error, ProfileRequestPayload>({
    mutationFn: requestProfile,
  });
}

/** Re-scan USB for newly plugged/unplugged modems. */
export function useRescanModems() {
  const queryClient = useQueryClient();

  return useMutation<RescanResponse, Error>({
    mutationFn: rescanModems,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: detectedModemsQueryKey });
      queryClient.invalidateQueries({ queryKey: activeProfileQueryKey });
      queryClient.invalidateQueries({ queryKey: deviceInfoQueryKey });
      queryClient.invalidateQueries({ queryKey: modemStatusQueryKey });
      queryClient.invalidateQueries({ queryKey: signalQueryKey });
      queryClient.invalidateQueries({ queryKey: simStatusQueryKey });
      queryClient.invalidateQueries({ queryKey: gpsQueryKey });
      queryClient.invalidateQueries({ queryKey: extendedSignalQueryKey });
      queryClient.invalidateQueries({ queryKey: antennaMetricsQueryKey });
      queryClient.invalidateQueries({ queryKey: configQueryKey });
    },
  });
}

/** Run discovery probe on the active modem. Saves results to /tmp on router. */
export function useDiscoverModem() {
  return useMutation<DiscoveryResponse, Error>({
    mutationFn: discoverModem,
  });
}
