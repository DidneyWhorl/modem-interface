/**
 * useGps Hook
 *
 * Fetches GPS position data with smart polling:
 * - Only polls when explicitly enabled (GPS section expanded)
 * - Only polls when page is visible
 * - 10s default interval (GPS doesn't need 2s updates)
 */

import { useQuery } from '@tanstack/react-query';
import { getGpsInfo } from '@/api';
import { usePageVisibility } from '@/hooks/usePageVisibility';
import type { GpsInfo } from '@/types/api';

export const gpsQueryKey = ['modem', 'gps'] as const;

interface UseGpsOptions {
  /** Whether to enable GPS polling (e.g., GPS section is expanded) */
  enabled?: boolean;
  /** Polling interval in ms (default 10000) */
  refreshInterval?: number;
}

export function useGps(options: UseGpsOptions = {}) {
  const { enabled = false, refreshInterval = 10000 } = options;
  const isVisible = usePageVisibility();

  // No WS event pushes GPS data — always poll via REST when enabled
  const shouldPoll = enabled && isVisible && refreshInterval > 0;

  return useQuery<GpsInfo>({
    queryKey: gpsQueryKey,
    queryFn: getGpsInfo,
    enabled,
    refetchInterval: shouldPoll ? refreshInterval : false,
    staleTime: 5000,
    placeholderData: (prev) => prev,
    retry: 1,
  });
}
