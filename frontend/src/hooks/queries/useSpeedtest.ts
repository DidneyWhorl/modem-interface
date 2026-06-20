/**
 * useSpeedtest Hooks
 *
 * TanStack Query hooks for speedtest history and mutation to run a test.
 */

import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import { getSpeedtestHistory, runSpeedtest } from '@/api/speedtest';
import type { SpeedtestHistoryResponse, SpeedtestMode, RunSpeedtestResponse } from '@/types/api';

export const speedtestHistoryQueryKey = ['speedtest', 'history'] as const;

export function useSpeedtestHistory(limit = 10) {
  return useQuery<SpeedtestHistoryResponse>({
    queryKey: [...speedtestHistoryQueryKey, limit],
    queryFn: () => getSpeedtestHistory(limit),
    staleTime: 30_000,
    retry: 1,
  });
}

export function useRunSpeedtest() {
  const queryClient = useQueryClient();

  return useMutation<RunSpeedtestResponse, Error, { mode: SpeedtestMode; wanId: string }>({
    mutationFn: ({ mode, wanId }) => runSpeedtest(mode, wanId),
    onSuccess: () => {
      // History will be invalidated by WebSocket speedtest_complete event
      queryClient.invalidateQueries({ queryKey: speedtestHistoryQueryKey });
    },
  });
}
