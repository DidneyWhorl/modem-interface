/**
 * SimCard Component
 *
 * Displays SIM card information:
 * - Presence and state (ready, PIN required, etc.)
 * - ICCID (masked by default)
 * - IMSI (when unlocked)
 * - Operator name from SIM
 */

import { useState } from 'react';
import { useSimStatus } from '@/hooks';
import { CreditCard, Lock, Unlock, Eye, EyeOff, AlertCircle, AlertTriangle } from 'lucide-react';
import type { SimState } from '@/types/api';

function getStateConfig(state: SimState): {
  label: string;
  color: string;
  icon: typeof Lock;
} {
  switch (state) {
    case 'ready':
      return { label: 'Ready', color: 'text-theme-success', icon: Unlock };
    case 'pin_required':
      return { label: 'PIN Required', color: 'text-theme-warning', icon: Lock };
    case 'puk_required':
      return { label: 'PUK Required', color: 'text-theme-error', icon: Lock };
    case 'not_inserted':
      return { label: 'Not Inserted', color: 'text-theme-text-muted', icon: AlertCircle };
    case 'error':
    default:
      return { label: 'Error', color: 'text-theme-error', icon: AlertCircle };
  }
}

function maskIdentifier(id: string | null | undefined): string {
  if (!id) return 'N/A';
  if (id.length < 8) return id;
  return id.slice(0, 4) + '••••••' + id.slice(-4);
}

export function SimCard() {
  const { data: status, isLoading, error } = useSimStatus();
  const [showIccid, setShowIccid] = useState(false);
  const [showImsi, setShowImsi] = useState(false);

  if (isLoading) {
    return (
      <div className="card p-6">
        <div className="flex items-center gap-2 mb-4">
          <CreditCard className="w-5 h-5 text-theme-text-muted" />
          <h2 className="text-lg font-medium text-theme-text-primary">
            SIM Card
          </h2>
        </div>
        <div className="loading-state">
          <div className="loading-spinner" />
          <span>Loading SIM status...</span>
        </div>
      </div>
    );
  }

  if (error) {
    return (
      <div className="card p-6">
        <div className="flex items-center gap-2 mb-4">
          <CreditCard className="w-5 h-5 text-theme-error" />
          <h2 className="text-lg font-medium text-theme-text-primary">
            SIM Card
          </h2>
        </div>
        <div className="error-state">
          <AlertTriangle className="w-8 h-8 text-theme-error" />
          <p className="text-sm text-theme-text-secondary">Failed to load SIM status</p>
          <p className="text-xs text-theme-text-muted">{error.message}</p>
        </div>
      </div>
    );
  }

  const stateConfig = getStateConfig(status?.state ?? 'error');
  const StateIcon = stateConfig.icon;

  return (
    <div className="card p-6">
      {/* Header */}
      <div className="flex items-center justify-between mb-6">
        <div className="flex items-center gap-2">
          <CreditCard className="w-5 h-5 text-theme-accent" />
          <h2 className="text-lg font-medium text-theme-text-primary">
            SIM Card
          </h2>
        </div>
        <div className={`flex items-center gap-1.5 ${stateConfig.color}`}>
          <StateIcon className="w-4 h-4" />
          <span className="text-sm font-medium">{stateConfig.label}</span>
        </div>
      </div>

      {/* Not Inserted State */}
      {!status?.present && (
        <div className="empty-state">
          <CreditCard className="w-8 h-8 text-theme-text-muted" />
          <p className="text-sm text-theme-text-secondary">No SIM card detected</p>
          <p className="text-xs text-theme-text-muted">Insert a SIM card and check the connection</p>
        </div>
      )}

      {/* SIM Info */}
      {status?.present && (
        <dl className="space-y-3">
          {/* Operator */}
          {status.operator_name && (
            <div className="flex items-center justify-between py-2 border-b border-theme-border-light">
              <dt className="text-sm text-theme-text-secondary">
                Operator
              </dt>
              <dd className="font-medium text-theme-text-primary">
                {status.operator_name}
              </dd>
            </div>
          )}

          {/* ICCID */}
          <div className="flex items-center justify-between py-2 border-b border-theme-border-light">
            <dt className="text-sm text-theme-text-secondary">ICCID</dt>
            <dd className="flex items-center gap-2">
              <span className="font-mono text-xs text-theme-text-primary">
                {showIccid ? status.iccid || 'N/A' : maskIdentifier(status.iccid)}
              </span>
              {status.iccid && (
                <button
                  onClick={() => setShowIccid(!showIccid)}
                  className="btn-icon p-2 sm:p-1 min-w-[44px] min-h-[44px] sm:min-w-0 sm:min-h-0 flex items-center justify-center hover:text-theme-text-secondary"
                >
                  {showIccid ? (
                    <EyeOff className="w-3.5 h-3.5" />
                  ) : (
                    <Eye className="w-3.5 h-3.5" />
                  )}
                </button>
              )}
            </dd>
          </div>

          {/* IMSI (only if available) */}
          {status.imsi && (
            <div className="flex items-center justify-between py-2">
              <dt className="text-sm text-theme-text-secondary">IMSI</dt>
              <dd className="flex items-center gap-2">
                <span className="font-mono text-xs text-theme-text-primary">
                  {showImsi ? status.imsi : maskIdentifier(status.imsi)}
                </span>
                <button
                  onClick={() => setShowImsi(!showImsi)}
                  className="btn-icon p-2 sm:p-1 min-w-[44px] min-h-[44px] sm:min-w-0 sm:min-h-0 flex items-center justify-center hover:text-theme-text-secondary"
                >
                  {showImsi ? (
                    <EyeOff className="w-3.5 h-3.5" />
                  ) : (
                    <Eye className="w-3.5 h-3.5" />
                  )}
                </button>
              </dd>
            </div>
          )}
        </dl>
      )}

      {/* PIN Required State */}
      {status?.state === 'pin_required' && (
        <div className="mt-4 p-3 bg-theme-warning/10 rounded-lg">
          <p className="text-sm text-theme-warning">
            SIM PIN is required. Enter PIN to unlock.
          </p>
          {/* TODO: Add PIN entry form */}
        </div>
      )}

      {/* PUK Required State */}
      {status?.state === 'puk_required' && (
        <div className="mt-4 p-3 bg-theme-error/10 rounded-lg">
          <p className="text-sm text-theme-error">
            SIM is locked. PUK code required to unlock.
          </p>
        </div>
      )}
    </div>
  );
}
