import { useMutation, useQueryClient } from '@tanstack/react-query';
import { setBandConfig, restoreBands } from '@/api';
import type { BandConfigRequest, BandConfigApplyResult } from '@/types/api';
import { bandConfigQueryKey } from '../queries/useBandConfig';
import { signalQueryKey } from '../queries/useSignal';
import { extendedSignalQueryKey } from '../queries/useExtendedSignal';

export function useSetBandConfig() {
  const queryClient = useQueryClient();

  return useMutation<BandConfigApplyResult, Error, { modemId: string; config: BandConfigRequest }>({
    mutationFn: ({ modemId, config }) => setBandConfig(modemId, config),
    onSuccess: (_data, { modemId }) => {
      // Re-fetch band config and signal data after applying
      queryClient.invalidateQueries({ queryKey: bandConfigQueryKey(modemId) });
      queryClient.invalidateQueries({ queryKey: signalQueryKey });
      queryClient.invalidateQueries({ queryKey: extendedSignalQueryKey });
    },
  });
}

export function useRestoreBands() {
  const queryClient = useQueryClient();

  return useMutation<{ success: boolean; message: string }, Error, { modemId: string }>({
    mutationFn: ({ modemId }) => restoreBands(modemId),
    onSuccess: (_data, { modemId }) => {
      queryClient.invalidateQueries({ queryKey: bandConfigQueryKey(modemId) });
    },
  });
}
