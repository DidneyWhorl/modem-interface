import { useMutation, useQueryClient } from '@tanstack/react-query';
import { selectMbnProfile, deactivateMbnProfile, setMbnAutoSelect } from '@/api';
import type { MbnSelectRequest, MbnAutoSelectRequest, MbnActionResult } from '@/types/api';
import { pdpDetailsQueryKey } from '../queries/usePdpDetails';

export function useSelectMbnProfile() {
  const queryClient = useQueryClient();
  return useMutation<MbnActionResult, Error, { modemId: string; req: MbnSelectRequest }>({
    mutationFn: ({ modemId, req }) => selectMbnProfile(modemId, req),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: pdpDetailsQueryKey });
    },
  });
}

export function useDeactivateMbnProfile() {
  const queryClient = useQueryClient();
  return useMutation<MbnActionResult, Error, { modemId: string }>({
    mutationFn: ({ modemId }) => deactivateMbnProfile(modemId),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: pdpDetailsQueryKey });
    },
  });
}

export function useSetMbnAutoSelect() {
  const queryClient = useQueryClient();
  return useMutation<MbnActionResult, Error, { modemId: string; req: MbnAutoSelectRequest }>({
    mutationFn: ({ modemId, req }) => setMbnAutoSelect(modemId, req),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: pdpDetailsQueryKey });
    },
  });
}
