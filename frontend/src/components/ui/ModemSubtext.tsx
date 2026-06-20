/**
 * ModemSubtext Component
 *
 * Tiny inline label showing the active modem's name.
 * Used in panel headers so users always know which modem they're viewing.
 * Renders as a span for inline use in compact header bars.
 */

import { useActiveProfile } from '@/hooks/queries';

export function ModemSubtext() {
  const { data: activeProfile } = useActiveProfile();
  const profile = activeProfile?.profile;

  if (!profile) return null;

  const name = profile.is_generic
    ? activeProfile?.detected?.vendor_id && activeProfile?.detected?.product_id
      ? `Unknown [${activeProfile.detected.vendor_id}:${activeProfile.detected.product_id}]`
      : 'Unknown Modem'
    : `${profile.manufacturer} ${profile.model}`;

  return (
    <span className="text-sm text-theme-text-secondary font-medium truncate">— {name}</span>
  );
}
