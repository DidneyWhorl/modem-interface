/**
 * TelemetryToggle Component
 *
 * Opt-in toggle for sending telemetry to the CTRL-Cloud portal.
 * Only renders when the device is licensed.
 */

import { useCallback, useEffect, useState } from 'react';
import { Radio } from 'lucide-react';
import clsx from 'clsx';
import { getTelemetryConfig, updateTelemetryConfig, type TelemetryConfig } from '@/api/telemetry';
import type { LicenseStatus } from '@/types/api';

interface TelemetryToggleProps {
  licenseInfo?: LicenseStatus | null;
}

export function TelemetryToggle({ licenseInfo }: TelemetryToggleProps) {
  const [config, setConfig] = useState<TelemetryConfig | null>(null);
  const [loading, setLoading] = useState(false);

  // Only render for licensed devices
  const isLicensed = licenseInfo?.state === 'valid';

  useEffect(() => {
    if (!isLicensed) return;
    let cancelled = false;
    getTelemetryConfig()
      .then((c) => { if (!cancelled) setConfig(c); })
      .catch(() => { /* silent — telemetry is optional */ });
    return () => { cancelled = true; };
  }, [isLicensed]);

  const handleToggle = useCallback(async () => {
    if (!config || loading) return;
    setLoading(true);
    try {
      const updated = await updateTelemetryConfig(!config.local_enabled);
      setConfig(updated);
    } catch {
      /* silent — telemetry is optional */
    } finally {
      setLoading(false);
    }
  }, [config, loading]);

  if (!isLicensed || !config) return null;

  return (
    <div>
      <button
        onClick={handleToggle}
        disabled={loading}
        className={clsx(
          'w-full flex items-center gap-2 px-3 py-2.5 sm:py-2 min-h-[44px] sm:min-h-0',
          'rounded-lg transition-colors text-sm',
          'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-theme-accent focus-visible:ring-offset-1',
          loading && 'opacity-50 cursor-wait',
          config.local_enabled
            ? 'text-theme-success bg-theme-success/20 hover:bg-theme-success/30'
            : 'text-theme-text-secondary hover:text-theme-text-primary hover:bg-theme-bg-tertiary/50'
        )}
        title={config.local_enabled ? 'Disable telemetry' : 'Enable telemetry'}
      >
        <Radio className="w-4 h-4" />
        <span>{config.local_enabled ? 'Telemetry: On' : 'Telemetry: Off'}</span>
      </button>

      {/* Status text */}
      {config.local_enabled && (
        <div className="px-4 pt-1 pb-0.5">
          {config.active ? (
            <span className="text-[10px] text-theme-success">Telemetry active</span>
          ) : (
            <span className="text-[10px] text-theme-warning">Waiting for portal access</span>
          )}
        </div>
      )}
    </div>
  );
}
