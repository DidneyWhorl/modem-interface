/**
 * SignalTrending Panel
 *
 * Displays historical signal quality charts (RSRP, RSRQ, SINR) over
 * selectable time windows using the backend signal history endpoint.
 */

import { useState } from 'react';
import { TrendingUp, BarChart3 } from 'lucide-react';
import { useSignalHistory } from '@/hooks';
import { SignalTrendChart } from './SignalTrendChart';
import type { SignalHistoryWindow } from '@/types/api';

const WINDOWS: { value: SignalHistoryWindow; label: string }[] = [
  { value: '1h', label: '1h' },
  { value: '6h', label: '6h' },
  { value: '24h', label: '24h' },
];

const CHARTS = [
  { dataKey: 'rsrp' as const, label: 'RSRP (Signal Power)', unit: 'dBm', color: '#3b82f6', domain: [-140, -44] as [number, number] },
  { dataKey: 'rsrq' as const, label: 'RSRQ (Signal Quality)', unit: 'dB', color: '#8b5cf6', domain: [-20, -3] as [number, number] },
  { dataKey: 'sinr' as const, label: 'SINR (Signal/Noise)', unit: 'dB', color: '#22c55e', domain: [-20, 30] as [number, number] },
];

export function SignalTrending() {
  const [window, setWindow] = useState<SignalHistoryWindow>('1h');
  const { data, isLoading } = useSignalHistory({ window, enabled: true });

  const samples = data?.samples ?? [];

  return (
    <div className="p-4">
      {/* Header + window selector */}
      <div className="flex items-center justify-between mb-4">
        <div className="flex items-center gap-2">
          <TrendingUp className="w-5 h-5 text-theme-text-muted" />
          <h2 className="text-lg font-medium text-theme-text-primary">Signal Trending</h2>
        </div>

        <div className="flex rounded-lg overflow-hidden border border-theme-border">
          {WINDOWS.map((w) => (
            <button
              key={w.value}
              onClick={() => setWindow(w.value)}
              className={`px-3 py-1 text-xs font-medium transition-colors ${
                window === w.value
                  ? 'bg-theme-accent-muted text-theme-text-accent'
                  : 'text-theme-text-muted hover:bg-theme-bg-tertiary'
              }`}
            >
              {w.label}
            </button>
          ))}
        </div>
      </div>

      {isLoading && samples.length === 0 ? (
        <div className="loading-state">
          <div className="loading-spinner" />
          <span>Loading signal history...</span>
        </div>
      ) : samples.length === 0 ? (
        <div className="empty-state">
          <BarChart3 className="w-8 h-8 text-theme-text-muted" />
          <p className="text-sm text-theme-text-secondary">No signal history available</p>
          <p className="text-xs text-theme-text-muted">History will appear as signal data is collected</p>
        </div>
      ) : (
        <div className="space-y-3">
          {CHARTS.map((chart) => (
            <SignalTrendChart
              key={chart.dataKey}
              samples={samples}
              dataKey={chart.dataKey}
              label={chart.label}
              unit={chart.unit}
              color={chart.color}
              domain={chart.domain}
            />
          ))}
        </div>
      )}
    </div>
  );
}
