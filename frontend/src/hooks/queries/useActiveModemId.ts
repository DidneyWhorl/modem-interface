import { useActiveProfile } from './useModemProfiles';

export function useActiveModemId(): string | undefined {
  const { data: activeProfile } = useActiveProfile();
  return activeProfile?.modem_id;
}
