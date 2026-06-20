import { useMutation, useQueryClient } from '@tanstack/react-query';
import { applyUpdate } from '@/api/system';
import type { UpdateApplyResult } from '@/api/system';

export function useApplyUpdate() {
  const queryClient = useQueryClient();

  return useMutation<UpdateApplyResult, Error, void>({
    mutationFn: applyUpdate,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['system'] });
    },
  });
}
