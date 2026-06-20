/**
 * useDeviceInfo Hook
 * 
 * Fetches modem device information (IMEI, manufacturer, model, firmware).
 * This data rarely changes, so we use long stale times.
 */

import { useQuery } from '@tanstack/react-query';
import { apiGet } from '@/api/client';

interface DeviceInfo {
  imei: string;
  manufacturer: string;
  model: string;
  firmware_version: string;
  supported_protocols: string[];
}

export const deviceInfoQueryKey = ['modem', 'info'] as const;

async function getDeviceInfo(): Promise<DeviceInfo> {
  return apiGet<DeviceInfo>('/modem/info');
}

export function useDeviceInfo() {
  return useQuery<DeviceInfo>({
    queryKey: deviceInfoQueryKey,
    queryFn: getDeviceInfo,
    staleTime: Infinity,
    refetchOnWindowFocus: false,
  });
}
