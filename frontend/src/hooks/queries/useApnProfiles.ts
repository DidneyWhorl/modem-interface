import { useQuery } from '@tanstack/react-query';
import { getApnProfiles } from '@/api';
import type { ApnProfile } from '@/types/api';

export const apnProfilesQueryKey = (modemId: string) => ['modem', modemId, 'apn-profiles'] as const;

interface UseApnProfilesOptions {
  modemId: string | undefined;
}

export function useApnProfiles(options: UseApnProfilesOptions) {
  const { modemId } = options;

  return useQuery<ApnProfile[]>({
    queryKey: apnProfilesQueryKey(modemId!),
    queryFn: () => getApnProfiles(modemId!),
    enabled: !!modemId,
    staleTime: 30_000,
    refetchOnWindowFocus: false,
    retry: 1,
  });
}
