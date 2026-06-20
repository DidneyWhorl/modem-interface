/**
 * ConnectionStatus Component
 *
 * Displays current connection state:
 * - Connected/Disconnected status
 * - Network operator name
 * - Technology (2G/3G/4G/5G)
 * - IP address when connected
 * - Connection uptime
 */

import { useModemStatus } from '@/hooks';
import { technologyLabel } from '@/lib/signal-utils';
import { Wifi, WifiOff, Globe, Building2, AlertTriangle } from 'lucide-react';

export function ConnectionStatus() {
  const { data: status, isLoading, error } = useModemStatus();

  if (isLoading) {
    return (
      <div className="card p-6">
        <div className="flex items-center gap-2 mb-4">
          <Wifi className="w-5 h-5 text-theme-text-muted" />
          <h2 className="text-lg font-medium text-theme-text-primary">
            Connection Status
          </h2>
        </div>
        <div className="loading-state">
          <div className="loading-spinner" />
          <span>Loading connection status...</span>
        </div>
      </div>
    );
  }

  if (error) {
    return (
      <div className="card p-6">
        <div className="flex items-center gap-2 mb-4">
          <WifiOff className="w-5 h-5 text-theme-error" />
          <h2 className="text-lg font-medium text-theme-text-primary">
            Connection Status
          </h2>
        </div>
        <div className="error-state">
          <AlertTriangle className="w-8 h-8 text-theme-error" />
          <p className="text-sm text-theme-text-secondary">Failed to load connection status</p>
          <p className="text-xs text-theme-text-muted">{error.message}</p>
        </div>
      </div>
    );
  }

  const isConnected = status?.connected ?? false;

  return (
    <div className="card p-6">
      {/* Header with Status Badge */}
      <div className="flex items-center justify-between mb-6">
        <div className="flex items-center gap-2">
          {isConnected ? (
            <Wifi className="w-5 h-5 text-theme-success" />
          ) : (
            <WifiOff className="w-5 h-5 text-theme-text-muted" />
          )}
          <h2 className="text-lg font-medium text-theme-text-primary">
            Connection Status
          </h2>
        </div>
        <span
          className={`
            inline-flex items-center px-2.5 py-1 rounded-full text-xs font-medium
            ${
              isConnected
                ? 'bg-theme-success/15 text-theme-success'
                : 'bg-theme-bg-tertiary text-theme-text-secondary'
            }
          `}
        >
          {isConnected ? 'Connected' : 'Disconnected'}
        </span>
      </div>

      {/* Status Grid */}
      <div className="grid grid-cols-3 gap-4">
        {/* Operator */}
        <div className="flex items-start gap-3">
          <Building2 className="w-5 h-5 text-theme-text-muted mt-0.5" />
          <div>
            <div className="text-xs text-theme-text-secondary uppercase tracking-wide">
              Operator
            </div>
            <div className="font-medium text-theme-text-primary">
              {status?.operator || 'Not registered'}
            </div>
          </div>
        </div>

        {/* Technology */}
        <div className="flex items-start gap-3">
          <svg
            className="w-5 h-5 text-theme-text-muted mt-0.5"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="2"
          >
            <path d="M2 20h.01M7 20v-4M12 20v-8M17 20V8M22 20V4" />
          </svg>
          <div>
            <div className="text-xs text-theme-text-secondary uppercase tracking-wide">
              Technology
            </div>
            <div className="font-medium text-theme-text-primary">
              {technologyLabel(status?.technology ?? null)}
            </div>
          </div>
        </div>

        {/* IP Address */}
        <div className="flex items-start gap-3">
          <Globe className="w-5 h-5 text-theme-text-muted mt-0.5" />
          <div>
            <div className="text-xs text-theme-text-secondary uppercase tracking-wide">
              IP Address
            </div>
            <div className="font-mono text-sm text-theme-text-primary">
              {status?.ip_address || 'N/A'}
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
