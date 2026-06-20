/**
 * SignalMeter Component
 *
 * Real-time signal strength visualization with:
 * - 5-bar signal indicator (based on RSRP)
 * - Numeric values for RSSI, RSRP, RSRQ, SINR
 * - Band and cell information
 * - Color-coded quality indicators
 * - Cache-driven updates (60s) + manual on-demand refresh
 */

import { useState, useEffect } from 'react';
import { useSignal } from '@/hooks';
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
import { Signal, ChevronDown, ChevronUp, Radio, AlertTriangle } from 'lucide-react';

interface SignalBarProps {
  filled: boolean;
  quality: SignalQuality;
  index: number;
}

function SignalBar({ filled, quality, index }: SignalBarProps) {
  const heights = ['h-3', 'h-5', 'h-7', 'h-9', 'h-11'];

  return (
    <div
      className={`
        w-3 rounded-sm transition-all duration-300
        ${heights[index]}
        ${filled ? qualityToBgColor(quality) : 'bg-theme-bg-tertiary'}
      `}
    />
  );
}

interface MetricCardProps {
  label: string;
  value: string;
  quality: SignalQuality;
  subtitle?: string;
  unavailable?: boolean;
}

function MetricCard({ label, value, quality, subtitle, unavailable }: MetricCardProps) {
  return (
    <div className="bg-theme-bg-primary rounded-lg p-3">
      <div className="text-xs text-theme-text-secondary uppercase tracking-wide">
        {label}
      </div>
      <div className={`text-lg font-semibold ${unavailable ? 'text-theme-text-muted' : qualityToColor(quality)}`}>
        {value}
      </div>
      {subtitle && (
        <div className="text-xs text-theme-text-muted">
          {subtitle}
        </div>
      )}
    </div>
  );
}

export function SignalMeter() {
  const [showAdvanced, setShowAdvanced] = useState(true);

  // Signal data from cache + WebSocket push (always enabled — signal is independent of data bearer)
  const { data: signal, isLoading, error, dataUpdatedAt } = useSignal({
    enabled: true,
  });

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

  // Loading state
  if (isLoading && !signal) {
    return (
      <div className="p-4">
        <div className="flex items-center gap-2 mb-4">
          <Signal className="w-5 h-5 text-theme-text-muted" />
          <h2 className="text-lg font-medium text-theme-text-primary">
            Signal Strength
          </h2>
        </div>
        <div className="loading-state">
          <div className="loading-spinner" />
          <span>Loading signal data...</span>
        </div>
      </div>
    );
  }

  // Error state
  if (error && !signal) {
    return (
      <div className="p-4">
        <div className="flex items-center gap-2 mb-4">
          <Signal className="w-5 h-5 text-theme-error" />
          <h2 className="text-lg font-medium text-theme-text-primary">
            Signal Strength
          </h2>
        </div>
        <div className="error-state">
          <AlertTriangle className="w-8 h-8 text-theme-error" />
          <p className="text-sm text-theme-text-secondary">Failed to load signal data</p>
          <p className="text-xs text-theme-text-muted">Use the refresh button to try again</p>
        </div>
      </div>
    );
  }

  // No data state
  if (!signal) {
    return (
      <div className="p-4">
        <div className="flex items-center gap-2 mb-4">
          <Signal className="w-5 h-5 text-theme-text-muted" />
          <h2 className="text-lg font-medium text-theme-text-primary">
            Signal Strength
          </h2>
        </div>
        <div className="empty-state">
          <Signal className="w-8 h-8 text-theme-text-muted" />
          <p className="text-sm text-theme-text-secondary">No signal data available</p>
          <p className="text-xs text-theme-text-muted">Waiting for modem to report signal strength</p>
        </div>
      </div>
    );
  }

  // Calculate signal quality
  const bars = rsrpToBars(signal.rsrp);
  const overallQuality = getOverallQuality(signal.rsrp, signal.rsrq, signal.sinr);
  const rsrpQuality = rsrpToQuality(signal.rsrp);
  const rsrqQuality = rsrqToQuality(signal.rsrq);
  const sinrQuality = sinrToQuality(signal.sinr);

  return (
    <div className="p-4">
      {/* Header */}
      <div className="flex items-center gap-2 mb-3">
        <Signal className={`w-5 h-5 ${qualityToColor(overallQuality)}`} />
        <h2 className="text-lg font-medium text-theme-text-primary">
          Signal
        </h2>
      </div>

      {/* Cache timestamp */}
      <div className="text-xs text-theme-text-muted mb-4 text-right">
        {updatedText}
      </div>

      {/* Main Signal Display */}
      <div className="flex items-end justify-between mb-6">
        {/* Signal Bars */}
        <div className="flex items-end gap-1">
          {[0, 1, 2, 3, 4].map((i) => (
            <SignalBar
              key={i}
              index={i}
              filled={i < bars}
              quality={overallQuality}
            />
          ))}
        </div>

        {/* Primary Metric */}
        <div className="text-right">
          <div className={`text-3xl font-bold ${isSentinel(signal.rsrp) ? 'text-theme-text-muted' : qualityToColor(rsrpQuality)}`}>
            {isSentinel(signal.rsrp) ? 'N/A' : signal.rsrp}
          </div>
          <div className="text-sm text-theme-text-secondary">
            dBm RSRP
          </div>
        </div>
      </div>

      {/* Band & Cell Info */}
      <div className="flex items-center gap-4 mb-4 p-3 bg-theme-bg-primary rounded-lg">
        <Radio className="w-4 h-4 text-theme-text-muted shrink-0" />
        <div className="flex-1 grid grid-cols-2 gap-2 text-sm">
          <div>
            <span className="text-theme-text-secondary">Band </span>
            <span className="font-medium text-theme-text-primary">
              {signal.band || 'N/A'}
            </span>
          </div>
          <div className="truncate">
            <span className="text-theme-text-secondary">Cell </span>
            <span className="font-mono text-xs text-theme-text-primary">
              {signal.cell_id || 'N/A'}
            </span>
          </div>
        </div>
      </div>

      {/* Toggle Advanced Metrics */}
      <button
        onClick={() => setShowAdvanced(!showAdvanced)}
        className="flex items-center gap-1 text-sm text-theme-text-accent hover:opacity-80"
      >
        {showAdvanced ? (
          <>
            <ChevronUp className="w-4 h-4" />
            Hide Details
          </>
        ) : (
          <>
            <ChevronDown className="w-4 h-4" />
            Show Details
          </>
        )}
      </button>

      {/* Advanced Metrics */}
      {showAdvanced && (
        <div className="grid grid-cols-2 sm:grid-cols-4 gap-3 mt-4">
          <MetricCard
            label="RSSI"
            value={formatSignalValue(signal.rssi, 'dbm')}
            quality={rsrpQuality}
            subtitle="Strength Indication"
            unavailable={isSentinel(signal.rssi)}
          />
          <MetricCard
            label="RSRP"
            value={formatSignalValue(signal.rsrp, 'dbm')}
            quality={rsrpQuality}
            subtitle="Signal Power"
            unavailable={isSentinel(signal.rsrp)}
          />
          <MetricCard
            label="RSRQ"
            value={formatSignalValue(signal.rsrq, 'db')}
            quality={rsrqQuality}
            subtitle="Signal Quality"
            unavailable={isSentinel(signal.rsrq)}
          />
          <MetricCard
            label="SINR"
            value={formatSignalValue(signal.sinr, 'db')}
            quality={sinrQuality}
            subtitle="Signal vs Noise"
            unavailable={isSentinel(signal.sinr)}
          />
        </div>
      )}

      {/* Quality Legend (when advanced is shown) */}
      {showAdvanced && (
        <div className="mt-4 pt-4 border-t border-theme-border">
          <div className="flex flex-wrap gap-4 text-xs">
            <div className="flex items-center gap-1">
              <div className="w-3 h-3 rounded-full bg-signal-excellent" />
              <span className="text-theme-text-secondary">Excellent</span>
            </div>
            <div className="flex items-center gap-1">
              <div className="w-3 h-3 rounded-full bg-signal-good" />
              <span className="text-theme-text-secondary">Good</span>
            </div>
            <div className="flex items-center gap-1">
              <div className="w-3 h-3 rounded-full bg-signal-fair" />
              <span className="text-theme-text-secondary">Fair</span>
            </div>
            <div className="flex items-center gap-1">
              <div className="w-3 h-3 rounded-full bg-signal-poor" />
              <span className="text-theme-text-secondary">Poor</span>
            </div>
            <div className="flex items-center gap-1">
              <div className="w-3 h-3 rounded-full bg-signal-none" />
              <span className="text-theme-text-secondary">No Signal</span>
            </div>
          </div>
        </div>
      )}

    </div>
  );
}
