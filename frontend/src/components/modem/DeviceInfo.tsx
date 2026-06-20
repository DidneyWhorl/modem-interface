/**
 * DeviceInfo Component
 *
 * Displays modem hardware information:
 * - Manufacturer and model
 * - IMEI (masked for privacy, with reveal option)
 * - Firmware version
 * - Supported protocols
 * - Profile status badge and capabilities
 * - Profile request button (when no specific profile)
 */

import { useState } from 'react';
import { useDeviceInfo, useActiveProfile } from '@/hooks';
import { Cpu, Eye, EyeOff, Copy, Check, Send, AlertTriangle } from 'lucide-react';
import { ProfileRequestDialog } from './ProfileRequestDialog';

function maskImei(imei: string): string {
  if (!imei || imei.length < 10) return imei;
  return imei.slice(0, 4) + '****' + imei.slice(-4);
}

export function DeviceInfo() {
  const { data: info, isLoading, error } = useDeviceInfo();
  const { data: activeProfile } = useActiveProfile();
  const [showImei, setShowImei] = useState(false);
  const [copied, setCopied] = useState(false);
  const [showRequestDialog, setShowRequestDialog] = useState(false);

  const handleCopyImei = async () => {
    if (info?.imei) {
      await navigator.clipboard.writeText(info.imei);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    }
  };

  const profile = activeProfile?.profile;
  const hasSpecificProfile = profile && !profile.is_generic;
  const caps = profile?.capabilities;

  if (isLoading) {
    return (
      <div className="card p-6">
        <div className="flex items-center gap-2 mb-4">
          <Cpu className="w-5 h-5 text-theme-text-muted" />
          <h2 className="text-lg font-medium text-theme-text-primary">
            Device Info
          </h2>
        </div>
        <div className="loading-state">
          <div className="loading-spinner" />
          <span>Loading device information...</span>
        </div>
      </div>
    );
  }

  if (error) {
    return (
      <div className="card p-6">
        <div className="flex items-center gap-2 mb-4">
          <Cpu className="w-5 h-5 text-theme-error" />
          <h2 className="text-lg font-medium text-theme-text-primary">
            Device Info
          </h2>
        </div>
        <div className="error-state">
          <AlertTriangle className="w-8 h-8 text-theme-error" />
          <p className="text-sm text-theme-text-secondary">Failed to load device information</p>
          <p className="text-xs text-theme-text-muted">{error.message}</p>
        </div>
      </div>
    );
  }

  // Build ATI response string for profile request dialog
  const atiResponse = info
    ? `${info.manufacturer}\n${info.model}\nIMEI: ${info.imei}\nFirmware: ${info.firmware_version}\nProtocols: ${info.supported_protocols?.join(', ')}`
    : '';

  return (
    <div className="card p-6">
      {/* Header */}
      <div className="flex items-center gap-2 mb-6">
        <Cpu className="w-5 h-5 text-theme-accent" />
        <h2 className="text-lg font-medium text-theme-text-primary">
          Device Info
        </h2>
      </div>

      {/* Device Name */}
      <div className="mb-3">
        <div className="text-2xl font-semibold text-theme-text-primary">
          {info?.model || 'Unknown Model'}
        </div>
        <div className="text-sm text-theme-text-secondary">
          {info?.manufacturer || 'Unknown Manufacturer'}
        </div>
      </div>

      {/* Profile Status Badge */}
      {profile && (
        <div className="mb-4">
          {hasSpecificProfile ? (
            <span className="inline-flex items-center px-2 py-0.5 text-xs font-medium rounded bg-theme-success/15 text-theme-success border border-theme-success/30">
              {profile.manufacturer} {profile.model} Profile
            </span>
          ) : (
            <div className="flex items-center gap-2 flex-wrap">
              <span className="inline-flex items-center px-2 py-0.5 text-xs font-medium rounded bg-theme-warning/15 text-theme-warning border border-theme-warning/30">
                Generic Profile
              </span>
              <button
                onClick={() => setShowRequestDialog(true)}
                className="inline-flex items-center gap-1 px-2 py-0.5 text-xs font-medium rounded border border-theme-border text-theme-text-muted hover:text-theme-text-secondary hover:bg-theme-bg-secondary transition-colors"
              >
                <Send className="w-3 h-3" />
                Request Profile
              </button>
            </div>
          )}
        </div>
      )}

      {/* Capabilities Badges */}
      {caps && hasSpecificProfile && (
        <div className="flex flex-wrap gap-1.5 mb-4">
          {caps.supports_5g && (
            <span title="5G NR capable (SA and/or NSA)" className="px-1.5 py-0.5 text-[10px] font-bold rounded bg-theme-accent/15 text-theme-accent border border-theme-accent/30 cursor-help">
              5G
            </span>
          )}
          {caps.supports_carrier_aggregation && (
            <span title="Carrier Aggregation — combines multiple bands for higher throughput" className="px-1.5 py-0.5 text-[10px] font-bold rounded bg-theme-text-accent/15 text-theme-text-accent border border-theme-text-accent/30 cursor-help">
              CA
            </span>
          )}
          {caps.has_gps && (
            <span title="Built-in GPS/GNSS receiver — may require activation and clear sky view to function" className="px-1.5 py-0.5 text-[10px] font-bold rounded bg-theme-success/15 text-theme-success border border-theme-success/30 cursor-help">
              GPS
            </span>
          )}
        </div>
      )}

      {/* Info Grid */}
      <dl className="space-y-3">
        {/* IMEI */}
        <div className="flex items-center justify-between py-2 border-b border-theme-border-light">
          <dt className="text-sm text-theme-text-secondary">IMEI</dt>
          <dd className="flex items-center gap-2">
            <span className="font-mono text-sm text-theme-text-primary">
              {showImei ? info?.imei : maskImei(info?.imei || '')}
            </span>
            <button
              onClick={() => setShowImei(!showImei)}
              className="btn-icon p-1 hover:text-theme-text-secondary"
              title={showImei ? 'Hide IMEI' : 'Show IMEI'}
            >
              {showImei ? (
                <EyeOff className="w-4 h-4" />
              ) : (
                <Eye className="w-4 h-4" />
              )}
            </button>
            <button
              onClick={handleCopyImei}
              className="btn-icon p-1 hover:text-theme-text-secondary"
              title="Copy IMEI"
            >
              {copied ? (
                <Check className="w-4 h-4 text-theme-success" />
              ) : (
                <Copy className="w-4 h-4" />
              )}
            </button>
          </dd>
        </div>

        {/* Firmware */}
        <div className="flex items-center justify-between py-2 border-b border-theme-border-light">
          <dt className="text-sm text-theme-text-secondary">Firmware</dt>
          <dd className="font-mono text-xs text-theme-text-primary break-all text-right">
            {info?.firmware_version || 'Unknown'}
          </dd>
        </div>

        {/* Protocols */}
        <div className="flex items-center justify-between py-2">
          <dt className="text-sm text-theme-text-secondary">Protocols</dt>
          <dd className="flex gap-1.5">
            {info?.supported_protocols?.map((proto) => (
              <span
                key={proto}
                className="inline-flex px-2 py-0.5 text-xs font-medium rounded bg-theme-bg-tertiary text-theme-text-secondary uppercase"
              >
                {proto}
              </span>
            )) || (
              <span className="text-theme-text-muted text-sm">N/A</span>
            )}
          </dd>
        </div>
      </dl>

      {/* Profile Request Dialog */}
      <ProfileRequestDialog
        isOpen={showRequestDialog}
        onClose={() => setShowRequestDialog(false)}
        vendorId={activeProfile?.detected?.vendor_id || ''}
        productId={activeProfile?.detected?.product_id || ''}
        deviceInfoResponse={atiResponse}
      />
    </div>
  );
}
