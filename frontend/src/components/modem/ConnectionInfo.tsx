/**
 * ConnectionInfo Component
 *
 * Combined panel merging Connection Status + Signal Info:
 * - Upper section: connection state badge, operator, technology, IP
 * - Lower section: signal bars + RSRP, band/cell, expandable advanced metrics, quality legend
 */

import { useState, useEffect } from 'react';
import { useModemStatus, useSignal } from '@/hooks';
import { technologyLabel } from '@/lib/signal-utils';
import {
  rsrpToBars,
  rsrpToQuality,
  rsrqToQuality,
  sinrToQuality,
  getOverallQuality,
  qualityToColor,
  qualityToBgColor,
  formatSignalValue,
  isSentinel,
  type SignalQuality,
} from '@/lib/signal-utils';
import {
  Wifi, WifiOff, Globe, Building2, AlertTriangle,
  Signal, ChevronDown, ChevronUp, Radio,
} from 'lucide-react';

function SignalBar({ filled, quality, index }: { filled: boolean; quality: SignalQuality; index: number }) {
  const heights = ['h-3', 'h-5', 'h-7', 'h-9', 'h-11'];
  return (
    <div
      className={`w-3 rounded-sm transition-all duration-300 ${heights[index]} ${filled ? qualityToBgColor(quality) : 'bg-theme-bg-tertiary'}`}
    />
  );
}

function MetricCard({ label, value, quality, subtitle, unavailable }: {
  label: string; value: string; quality: SignalQuality; subtitle?: string; unavailable?: boolean;
}) {
  return (
    <div className="bg-theme-bg-primary rounded-lg p-3">
      <div className="text-xs text-theme-text-secondary uppercase tracking-wide">{label}</div>
      <div className={`text-lg font-semibold ${unavailable ? 'text-theme-text-muted' : qualityToColor(quality)}`}>{value}</div>
      {subtitle && <div className="text-xs text-theme-text-muted">{subtitle}</div>}
    </div>
  );
}

export function ConnectionInfo() {
  const { data: status, isLoading: statusLoading, error: statusError } = useModemStatus();
  const { data: signal, isLoading: signalLoading, error: signalError, dataUpdatedAt } = useSignal({ enabled: true });
  const [showAdvanced, setShowAdvanced] = useState(true);

  // Live "Updated Xs ago" counter
  const [, setTick] = useState(0);
  useEffect(() => {
    const timer = setInterval(() => setTick(t => t + 1), 1000);
    return () => clearInterval(timer);
  }, []);

  const secondsAgo = dataUpdatedAt ? Math.floor((Date.now() - dataUpdatedAt) / 1000) : null;
  const updatedText = secondsAgo !== null
    ? secondsAgo < 60 ? `Updated ${secondsAgo}s ago` : `Updated ${Math.floor(secondsAgo / 60)}m ago`
    : 'Waiting for data...';

  const isConnected = status?.connected ?? false;

  return (
    <div className="p-4 space-y-0">
      {/* === Upper Section: Connection Status === */}
      <ConnectionStatusSection
        isLoading={statusLoading}
        error={statusError}
        isConnected={isConnected}
        operator={status?.operator ?? null}
        technology={status?.technology ?? null}
        ipAddress={status?.ip_address ?? null}
      />

      {/* Divider */}
      <div className="border-t border-theme-border my-4" />

      {/* === Lower Section: Signal === */}
      <SignalSection
        signal={signal}
        isLoading={signalLoading}
        error={signalError}
        updatedText={updatedText}
        showAdvanced={showAdvanced}
        onToggleAdvanced={() => setShowAdvanced(v => !v)}
      />
    </div>
  );
}

function ConnectionStatusSection({ isLoading, error, isConnected, operator, technology, ipAddress }: {
  isLoading: boolean;
  error: Error | null;
  isConnected: boolean;
  operator: string | null;
  technology: string | null;
  ipAddress: string | null;
}) {
  if (isLoading) {
    return (
      <div>
        <div className="flex items-center gap-2 mb-4">
          <Wifi className="w-5 h-5 text-theme-text-muted" />
          <h3 className="text-sm font-medium text-theme-text-secondary uppercase tracking-wide">Connection</h3>
        </div>
        <div className="loading-state">
          <div className="loading-spinner" />
          <span>Loading...</span>
        </div>
      </div>
    );
  }

  if (error) {
    return (
      <div>
        <div className="flex items-center gap-2 mb-4">
          <WifiOff className="w-5 h-5 text-theme-error" />
          <h3 className="text-sm font-medium text-theme-text-secondary uppercase tracking-wide">Connection</h3>
        </div>
        <div className="error-state">
          <AlertTriangle className="w-6 h-6 text-theme-error" />
          <p className="text-sm text-theme-text-secondary">Failed to load connection status</p>
        </div>
      </div>
    );
  }

  return (
    <div>
      {/* Section header + status badge */}
      <div className="flex items-center justify-between mb-4">
        <div className="flex items-center gap-2">
          {isConnected ? (
            <Wifi className="w-5 h-5 text-theme-success" />
          ) : (
            <WifiOff className="w-5 h-5 text-theme-text-muted" />
          )}
          <h3 className="text-sm font-medium text-theme-text-secondary uppercase tracking-wide">Connection</h3>
        </div>
        <span
          className={`inline-flex items-center px-2.5 py-1 rounded-full text-xs font-medium ${
            isConnected
              ? 'bg-theme-success/15 text-theme-success'
              : 'bg-theme-bg-tertiary text-theme-text-secondary'
          }`}
        >
          {isConnected ? 'Connected' : 'Disconnected'}
        </span>
      </div>

      {/* Status grid */}
      <div className="grid grid-cols-3 gap-4">
        <div className="flex items-start gap-3">
          <Building2 className="w-5 h-5 text-theme-text-muted mt-0.5" />
          <div>
            <div className="text-xs text-theme-text-secondary uppercase tracking-wide">Operator</div>
            <div className="font-medium text-theme-text-primary">{operator || 'Not registered'}</div>
          </div>
        </div>
        <div className="flex items-start gap-3">
          <svg className="w-5 h-5 text-theme-text-muted mt-0.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
            <path d="M2 20h.01M7 20v-4M12 20v-8M17 20V8M22 20V4" />
          </svg>
          <div>
            <div className="text-xs text-theme-text-secondary uppercase tracking-wide">Technology</div>
            <div className="font-medium text-theme-text-primary">{technologyLabel(technology)}</div>
          </div>
        </div>
        <div className="flex items-start gap-3">
          <Globe className="w-5 h-5 text-theme-text-muted mt-0.5" />
          <div>
            <div className="text-xs text-theme-text-secondary uppercase tracking-wide">IP Address</div>
            <div className="font-mono text-sm text-theme-text-primary">{ipAddress || 'N/A'}</div>
          </div>
        </div>
      </div>
    </div>
  );
}

interface SignalData {
  rssi: number;
  rsrp: number;
  rsrq: number;
  sinr: number;
  band: string;
  cell_id: string;
}

function SignalSection({ signal, isLoading, error, updatedText, showAdvanced, onToggleAdvanced }: {
  signal: SignalData | undefined | null;
  isLoading: boolean;
  error: Error | null;
  updatedText: string;
  showAdvanced: boolean;
  onToggleAdvanced: () => void;
}) {
  if (isLoading && !signal) {
    return (
      <div>
        <div className="flex items-center gap-2 mb-4">
          <Signal className="w-5 h-5 text-theme-text-muted" />
          <h3 className="text-sm font-medium text-theme-text-secondary uppercase tracking-wide">Signal</h3>
        </div>
        <div className="loading-state">
          <div className="loading-spinner" />
          <span>Loading signal data...</span>
        </div>
      </div>
    );
  }

  if (error && !signal) {
    return (
      <div>
        <div className="flex items-center gap-2 mb-4">
          <Signal className="w-5 h-5 text-theme-error" />
          <h3 className="text-sm font-medium text-theme-text-secondary uppercase tracking-wide">Signal</h3>
        </div>
        <div className="error-state">
          <AlertTriangle className="w-6 h-6 text-theme-error" />
          <p className="text-sm text-theme-text-secondary">Failed to load signal data</p>
        </div>
      </div>
    );
  }

  if (!signal) {
    return (
      <div>
        <div className="flex items-center gap-2 mb-4">
          <Signal className="w-5 h-5 text-theme-text-muted" />
          <h3 className="text-sm font-medium text-theme-text-secondary uppercase tracking-wide">Signal</h3>
        </div>
        <div className="empty-state">
          <Signal className="w-6 h-6 text-theme-text-muted" />
          <p className="text-sm text-theme-text-secondary">No signal data available</p>
        </div>
      </div>
    );
  }

  const bars = rsrpToBars(signal.rsrp);
  const overallQuality = getOverallQuality(signal.rsrp, signal.rsrq, signal.sinr);
  const rsrpQuality = rsrpToQuality(signal.rsrp);
  const rsrqQuality = rsrqToQuality(signal.rsrq);
  const sinrQuality = sinrToQuality(signal.sinr);

  return (
    <div>
      {/* Section header + timestamp */}
      <div className="flex items-center justify-between mb-3">
        <div className="flex items-center gap-2">
          <Signal className={`w-5 h-5 ${qualityToColor(overallQuality)}`} />
          <h3 className="text-sm font-medium text-theme-text-secondary uppercase tracking-wide">Signal</h3>
        </div>
        <div className="text-xs text-theme-text-muted">{updatedText}</div>
      </div>

      {/* Main signal display */}
      <div className="flex items-end justify-between mb-6">
        <div className="flex items-end gap-1">
          {[0, 1, 2, 3, 4].map((i) => (
            <SignalBar key={i} index={i} filled={i < bars} quality={overallQuality} />
          ))}
        </div>
        <div className="text-right">
          <div className={`text-3xl font-bold ${isSentinel(signal.rsrp) ? 'text-theme-text-muted' : qualityToColor(rsrpQuality)}`}>
            {isSentinel(signal.rsrp) ? 'N/A' : signal.rsrp}
          </div>
          <div className="text-sm text-theme-text-secondary">dBm RSRP</div>
        </div>
      </div>

      {/* Band & Cell Info */}
      <div className="flex items-center gap-4 mb-4 p-3 bg-theme-bg-primary rounded-lg">
        <Radio className="w-4 h-4 text-theme-text-muted shrink-0" />
        <div className="flex-1 grid grid-cols-2 gap-2 text-sm">
          <div>
            <span className="text-theme-text-secondary">Band </span>
            <span className="font-medium text-theme-text-primary">{signal.band || 'N/A'}</span>
          </div>
          <div className="truncate">
            <span className="text-theme-text-secondary">Cell </span>
            <span className="font-mono text-xs text-theme-text-primary">{signal.cell_id || 'N/A'}</span>
          </div>
        </div>
      </div>

      {/* Toggle Advanced Metrics */}
      <button
        onClick={onToggleAdvanced}
        className="flex items-center gap-1 text-sm text-theme-text-accent hover:opacity-80"
      >
        {showAdvanced ? (
          <><ChevronUp className="w-4 h-4" />Hide Details</>
        ) : (
          <><ChevronDown className="w-4 h-4" />Show Details</>
        )}
      </button>

      {/* Advanced Metrics */}
      {showAdvanced && (
        <div className="grid grid-cols-2 sm:grid-cols-4 gap-3 mt-4">
          <MetricCard label="RSSI" value={formatSignalValue(signal.rssi, 'dbm')} quality={rsrpQuality} subtitle="Strength Indication" unavailable={isSentinel(signal.rssi)} />
          <MetricCard label="RSRP" value={formatSignalValue(signal.rsrp, 'dbm')} quality={rsrpQuality} subtitle="Signal Power" unavailable={isSentinel(signal.rsrp)} />
          <MetricCard label="RSRQ" value={formatSignalValue(signal.rsrq, 'db')} quality={rsrqQuality} subtitle="Signal Quality" unavailable={isSentinel(signal.rsrq)} />
          <MetricCard label="SINR" value={formatSignalValue(signal.sinr, 'db')} quality={sinrQuality} subtitle="Signal vs Noise" unavailable={isSentinel(signal.sinr)} />
        </div>
      )}

      {/* Quality Legend */}
      {showAdvanced && (
        <div className="mt-4 pt-4 border-t border-theme-border">
          <div className="flex flex-wrap gap-4 text-xs">
            <div className="flex items-center gap-1"><div className="w-3 h-3 rounded-full bg-signal-excellent" /><span className="text-theme-text-secondary">Excellent</span></div>
            <div className="flex items-center gap-1"><div className="w-3 h-3 rounded-full bg-signal-good" /><span className="text-theme-text-secondary">Good</span></div>
            <div className="flex items-center gap-1"><div className="w-3 h-3 rounded-full bg-signal-fair" /><span className="text-theme-text-secondary">Fair</span></div>
            <div className="flex items-center gap-1"><div className="w-3 h-3 rounded-full bg-signal-poor" /><span className="text-theme-text-secondary">Poor</span></div>
            <div className="flex items-center gap-1"><div className="w-3 h-3 rounded-full bg-signal-none" /><span className="text-theme-text-secondary">No Signal</span></div>
          </div>
        </div>
      )}
    </div>
  );
}
