/**
 * useAntennaMetrics Hook
 *
 * Fetches per-antenna-port signal metrics (AT+QRSRP, AT+QSINR, AT+QRSRQ, AT+QCSQ)
 * with smart polling:
 * - Only polls when explicitly enabled (panel is visible)
 * - Only polls when page is visible
 * - 5s default interval
 */

import { useQuery } from '@tanstack/react-query';
import { getAntennaMetrics } from '@/api';
import { usePageVisibility } from '@/hooks/usePageVisibility';
import type { AntennaMetrics } from '@/types/api';

export const antennaMetricsQueryKey = ['modem', 'signal', 'antenna'] as const;

interface UseAntennaMetricsOptions {
  /** Whether to enable polling */
  enabled?: boolean;
  /** Polling interval in ms (default 5000) */
  refreshInterval?: number;
}

export function useAntennaMetrics(options: UseAntennaMetricsOptions = {}) {
  const { enabled = false, refreshInterval = 5000 } = options;
  const isVisible = usePageVisibility();

  // No WS event pushes antenna metrics — always poll via REST when enabled
  const shouldPoll = enabled && isVisible && refreshInterval > 0;

  return useQuery<AntennaMetrics>({
    queryKey: antennaMetricsQueryKey,
    queryFn: getAntennaMetrics,
    enabled,
    refetchInterval: shouldPoll ? refreshInterval : false,
    staleTime: 3000,
    placeholderData: (prev) => prev,
    retry: 1,
  });
}
