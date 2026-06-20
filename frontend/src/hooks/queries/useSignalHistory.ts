/**
 * useSignalHistory Hook
 *
 * Fetches historical signal samples for trending charts.
 * Refetches every 60s to stay in sync with the backend cache refresh.
 */

import { useQuery } from '@tanstack/react-query';
import { getSignalHistory } from '@/api';
import type { SignalHistory, SignalHistoryWindow } from '@/types/api';

export const signalHistoryQueryKey = (window: SignalHistoryWindow) =>
  ['modem', 'signal', 'history', window] as const;

export function useSignalHistory(options: {
  window?: SignalHistoryWindow;
  enabled?: boolean;
} = {}) {
  const { window = '1h', enabled = false } = options;

  return useQuery<SignalHistory>({
    queryKey: signalHistoryQueryKey(window),
    queryFn: () => getSignalHistory(window),
    enabled,
    refetchInterval: 60_000,
    staleTime: 55_000,
    placeholderData: (prev) => prev,
  });
}
