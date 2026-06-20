/**
 * useModemStatus Hook
 * 
 * Fetches modem status with automatic refetching.
 * WebSocket events will invalidate this query for real-time updates.
 */

import { useQuery } from '@tanstack/react-query';
import { getModemStatus } from '@/api';
import { useUIStore } from '@/stores/uiStore';
import type { ModemStatus } from '@/types/api';

export const modemStatusQueryKey = ['modem', 'status'] as const;

export function useModemStatus() {
  const wsConnected = useUIStore((s) => s.wsConnected);

  return useQuery<ModemStatus>({
    queryKey: modemStatusQueryKey,
    queryFn: getModemStatus,
    // Poll as fallback only when WebSocket is not connected
    refetchInterval: wsConnected ? false : 10_000,
    // Keep previous data while refetching for smoother UX
    placeholderData: (prev) => prev,
    // Consider data stale after 5 seconds
    staleTime: 5_000,
  });
}
