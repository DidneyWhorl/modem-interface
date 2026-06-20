import { useQuery } from '@tanstack/react-query';
import { getSteeringRules } from '@/api/steering';
import { useUIStore } from '@/stores/uiStore';
import type { SteeringListResponse } from '@/types/steering';

export const steeringQueryKey = ['wan', 'steering'] as const;

export function useSteeringRules() {
  const wsConnected = useUIStore((s) => s.wsConnected);

  return useQuery<SteeringListResponse>({
    queryKey: steeringQueryKey,
    queryFn: getSteeringRules,
    staleTime: 10_000,
    // Poll as fallback only when WebSocket is not connected
    refetchInterval: wsConnected ? false : 30_000,
    refetchOnWindowFocus: false,
    retry: 1,
  });
}
