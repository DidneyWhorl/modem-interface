/**
 * useSignal Hook
 *
 * Signal data from 60s master cache + WebSocket push.
 * No client-side polling — data arrives via:
 *   1. Initial GET on mount (from backend cache)
 *   2. WebSocket signal_update events (every 60s)
 *   3. Manual POST /signal/refresh (on-demand)
 */

import { useQuery } from '@tanstack/react-query';
import { getSignalInfo } from '@/api';
import type { SignalInfo } from '@/types/api';

export const signalQueryKey = ['modem', 'signal'] as const;

export function useSignal(options: { enabled?: boolean } = {}) {
  const { enabled = false } = options;

  return useQuery<SignalInfo>({
    queryKey: signalQueryKey,
    queryFn: getSignalInfo,
    enabled,
    refetchInterval: false,
    staleTime: 60_000,
    placeholderData: (prev) => prev,
    retry: (failureCount, error) => {
      if (error && 'status' in error && (error as Record<string, unknown>).status === 409) {
        return false;
      }
      return failureCount < 3;
    },
  });
}
