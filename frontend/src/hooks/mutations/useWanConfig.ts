import { useMutation, useQueryClient } from '@tanstack/react-query';
import {
  updateWanConfig, scanWanModems,
  clearWatchdogLog, failbackNow, acceptFailover,
  addEthernetPort,
} from '@/api/wan';
import type {
  WanStatusResponse, WanConfig,
  WanScanResponse, AddEthernetRequest,
} from '@/types/api';
import { wanStatusQueryKey, watchdogLogQueryKey } from '../queries/useWanStatus';

export function useApplyWanConfig() {
  const queryClient = useQueryClient();
  return useMutation<WanStatusResponse, Error, WanConfig>({
    mutationFn: updateWanConfig,
    onSuccess: (data) => {
      queryClient.setQueryData(wanStatusQueryKey, data);
    },
  });
}

export function useScanWanModems() {
  const queryClient = useQueryClient();
  return useMutation<WanScanResponse, Error, void>({
    mutationFn: scanWanModems,
    onSuccess: (data) => {
      // WanScanResponse flattens WanStatusResponse fields via serde flatten,
      // so we can set the status query data directly (extra fields are harmless).
      queryClient.setQueryData(wanStatusQueryKey, data);
    },
  });
}

export function useAddEthernetPort() {
  const queryClient = useQueryClient();
  return useMutation<WanConfig, Error, AddEthernetRequest>({
    mutationFn: addEthernetPort,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: wanStatusQueryKey });
    },
  });
}

export function useClearWatchdogLog() {
  const queryClient = useQueryClient();
  return useMutation<void, Error, void>({
    mutationFn: clearWatchdogLog,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: watchdogLogQueryKey });
    },
  });
}

export function useFailbackNow() {
  const queryClient = useQueryClient();
  return useMutation<WanStatusResponse, Error, void>({
    mutationFn: failbackNow,
    onSuccess: (data) => {
      queryClient.setQueryData(wanStatusQueryKey, data);
    },
  });
}

export function useAcceptFailover() {
  const queryClient = useQueryClient();
  return useMutation<WanStatusResponse, Error, void>({
    mutationFn: acceptFailover,
    onSuccess: (data) => {
      queryClient.setQueryData(wanStatusQueryKey, data);
    },
  });
}
