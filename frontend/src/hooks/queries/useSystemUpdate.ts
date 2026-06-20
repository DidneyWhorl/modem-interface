import { useQuery } from '@tanstack/react-query';
import { checkForUpdate, getUpdateStatus, getVersion } from '@/api/system';

export const versionQueryKey = ['system', 'version'] as const;
export const updateCheckQueryKey = ['system', 'update', 'check'] as const;
export const updateStatusQueryKey = ['system', 'update', 'status'] as const;

export function useVersion() {
  return useQuery({
    queryKey: versionQueryKey,
    queryFn: getVersion,
    staleTime: Infinity,
  });
}

export function useUpdateCheck() {
  return useQuery({
    queryKey: updateCheckQueryKey,
    queryFn: checkForUpdate,
    staleTime: 5 * 60 * 1000,
    enabled: false, // Only fetch on manual trigger via refetch()
  });
}

export function useUpdateStatus() {
  return useQuery({
    queryKey: updateStatusQueryKey,
    queryFn: getUpdateStatus,
    staleTime: 10_000,
  });
}
