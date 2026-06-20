import { useQuery } from '@tanstack/react-query';
import { getWanStatus, getWatchdogLog } from '@/api/wan';
import { useUIStore } from '@/stores/uiStore';
import type { WanStatusResponse, WanWatchdogLogResponse } from '@/types/api';

export const wanStatusQueryKey = ['wan', 'status'] as const;
export const watchdogLogQueryKey = ['wan', 'watchdog-log'] as const;

export function useWanStatus() {
  const wsConnected = useUIStore((s) => s.wsConnected);

  return useQuery<WanStatusResponse>({
    queryKey: wanStatusQueryKey,
    queryFn: getWanStatus,
    staleTime: 10_000,
    // Poll as fallback only when WebSocket is not connected
    refetchInterval: wsConnected ? false : 30_000,
    refetchOnWindowFocus: false,
    retry: 1,
  });
}

export function useWatchdogLog(enabled = true) {
  return useQuery<WanWatchdogLogResponse>({
    queryKey: watchdogLogQueryKey,
    queryFn: getWatchdogLog,
    staleTime: 30_000,
    refetchOnWindowFocus: false,
    enabled,
    retry: 1,
  });
}
