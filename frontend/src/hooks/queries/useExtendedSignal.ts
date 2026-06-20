/**
 * useExtendedSignal Hook
 *
 * Fetches extended signal data (carrier aggregation, network info, neighbour cells)
 * with smart polling:
 * - Only polls when explicitly enabled (advanced section expanded)
 * - Only polls when page is visible
 * - 10s default interval
 */

import { useQuery } from '@tanstack/react-query';
import { getExtendedSignalInfo } from '@/api';
import { usePageVisibility } from '@/hooks/usePageVisibility';
import type { ExtendedSignalInfo } from '@/types/api';

export const extendedSignalQueryKey = ['modem', 'signal', 'extended'] as const;

interface UseExtendedSignalOptions {
  /** Whether to enable polling (e.g., advanced section is expanded) */
  enabled?: boolean;
  /** Polling interval in ms (default 10000) */
  refreshInterval?: number;
}

export function useExtendedSignal(options: UseExtendedSignalOptions = {}) {
  const { enabled = false, refreshInterval = 10000 } = options;
  const isVisible = usePageVisibility();

  // No WS event pushes extended signal data — always poll via REST when enabled
  const shouldPoll = enabled && isVisible && refreshInterval > 0;

  return useQuery<ExtendedSignalInfo>({
    queryKey: extendedSignalQueryKey,
    queryFn: getExtendedSignalInfo,
    enabled,
    refetchInterval: shouldPoll ? refreshInterval : false,
    staleTime: 5000,
    placeholderData: (prev) => prev,
    retry: 1,
  });
}
