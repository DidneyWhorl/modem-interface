/**
 * useSimStatus Hook
 * 
 * Fetches SIM card status. Changes are infrequent,
 * so longer stale time is appropriate.
 */

import { useQuery } from '@tanstack/react-query';
import { getSimStatus } from '@/api';
import { useUIStore } from '@/stores/uiStore';
import type { SimStatus } from '@/types/api';

export const simStatusQueryKey = ['sim', 'status'] as const;

export function useSimStatus() {
  const wsConnected = useUIStore((s) => s.wsConnected);

  return useQuery<SimStatus>({
    queryKey: simStatusQueryKey,
    queryFn: getSimStatus,
    // SIM status changes rarely; WS sends sim_event to invalidate
    staleTime: 30_000,
    refetchInterval: wsConnected ? false : 60_000,
    placeholderData: (prev) => prev,
  });
}
