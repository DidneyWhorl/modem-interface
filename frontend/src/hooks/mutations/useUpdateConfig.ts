/**
 * useUpdateConfig Hook
 * 
 * Mutation for updating modem configuration.
 */

import { useMutation, useQueryClient } from '@tanstack/react-query';
import { updateConfig } from '@/api';
import { configQueryKey } from '../queries';
import type { ModemConfig } from '@/types/api';

export function useUpdateConfig() {
  const queryClient = useQueryClient();

  return useMutation<ModemConfig, Error, Partial<ModemConfig>>({
    mutationFn: updateConfig,
    onSuccess: (newConfig) => {
      // Update cache with new config
      queryClient.setQueryData(configQueryKey, newConfig);
    },
  });
}
