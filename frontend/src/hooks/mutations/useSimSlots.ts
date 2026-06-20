import { useMutation, useQueryClient } from '@tanstack/react-query';
import { updateSimSlotConfig, switchSimSlot } from '@/api';
import type { SimSlotConfig, SimSlotSwitchRequest, SimSlotSwitchResult } from '@/types/api';
import { simSlotsQueryKey } from '../queries/useSimSlots';
import { simStatusQueryKey } from '../queries/useSimStatus';
import { configQueryKey } from '../queries/useConfig';
import { modemStatusQueryKey } from '../queries/useModemStatus';

export function useUpdateSimSlotConfig() {
  const queryClient = useQueryClient();
  return useMutation<SimSlotConfig, Error, SimSlotConfig>({
    mutationFn: updateSimSlotConfig,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: simSlotsQueryKey });
    },
  });
}

export function useSwitchSimSlot() {
  const queryClient = useQueryClient();
  return useMutation<SimSlotSwitchResult, Error, SimSlotSwitchRequest>({
    mutationFn: switchSimSlot,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: simSlotsQueryKey });
      queryClient.invalidateQueries({ queryKey: simStatusQueryKey });
      queryClient.invalidateQueries({ queryKey: configQueryKey });
      queryClient.invalidateQueries({ queryKey: modemStatusQueryKey });
    },
  });
}
