/**
 * AntennaMetrics Panel
 *
 * Displays per-antenna-port signal measurements from AT+QRSRP, AT+QSINR,
 * AT+QRSRQ, and AT+QCSQ. Shows RSRP, RSRQ, SINR for each RX port.
 *
 * Polling is manual — user must start/stop it, and can pick the interval.
 * Pauses automatically when the browser tab is hidden.
 */

import { useState } from 'react';
import { useAntennaMetrics, useExtendedSignal } from '@/hooks';
import { useUIStore } from '@/stores/uiStore';
import {
  rsrpToQuality,
  rsrqToQuality,
  sinrToQuality,
  qualityToColor,
  isSentinel,
} from '@/lib/signal-utils';
import { Antenna, Play, Pause, RefreshCw } from 'lucide-react';
import { CarrierAggregationSection } from './CarrierAggregationSection';

const INTERVAL_OPTIONS = [
  { value: 2000, label: '2s' },
  { value: 5000, label: '5s' },
  { value: 10000, label: '10s' },
  { value: 30000, label: '30s' },
];

export function AntennaMetrics() {
  const [polling, setPolling] = useState(false);
  const interval = useUIStore((s) => s.pollingIntervals['antenna-metrics'] ?? 5000);
  const setPollingInterval = useUIStore((s) => s.setPollingInterval);

  const { data: metrics, isLoading, isFetching, dataUpdatedAt, refetch } = useAntennaMetrics({
    enabled: polling,
    refreshInterval: interval,
  });

  const { data: extSignal } = useExtendedSignal({
    enabled: polling,
    refreshInterval: interval,
  });

  const handleRefresh = () => {
    if (!isFetching) refetch();
  };

  if (isLoading && !metrics) {
    return (
      <div className="p-4">
        <div className="flex items-center gap-2 mb-4">
          <Antenna className="w-5 h-5 text-theme-text-muted" />
          <h2 className="text-lg font-medium text-theme-text-primary">
            Antenna Metrics
          </h2>
        </div>
        <div className="loading-state">
          <div className="loading-spinner" />
          <span>Loading antenna metrics...</span>
        </div>
      </div>
    );
  }

  const hasAnyData = metrics && metrics.ports.length > 0;
  const lastUpdate = dataUpdatedAt ? new Date(dataUpdatedAt).toLocaleTimeString() : '--';

  return (
    <div className="p-4">
      {/* Header */}
      <div className="flex items-center justify-between mb-3">
        <div className="flex items-center gap-2">
          <Antenna className={`w-5 h-5 ${polling ? 'text-theme-text-accent' : 'text-theme-text-muted'}`} />
          <h2 className="text-lg font-medium text-theme-text-primary">
            Antenna Metrics
          </h2>
        </div>

        {/* Controls */}
        <div className="flex items-center gap-1">
          {/* Start / Stop */}
          <button
            onClick={() => setPolling(!polling)}
            className={`btn-icon p-1.5 ${
              polling
                ? 'text-theme-success hover:text-theme-success hover:bg-theme-accent-muted'
                : ''
            }`}
            title={polling ? 'Stop polling' : 'Start polling'}
          >
            {polling ? <Play className="w-4 h-4" /> : <Pause className="w-4 h-4" />}
          </button>

          {/* Interval Selector */}
          <select
            value={interval}
            onChange={(e) => setPollingInterval('antenna-metrics', Number(e.target.value))}
            className="select-compact"
            title="Polling interval"
          >
            {INTERVAL_OPTIONS.map(opt => (
              <option key={opt.value} value={opt.value}>{opt.label}</option>
            ))}
          </select>

          {/* Manual Refresh */}
          <button
            onClick={handleRefresh}
            disabled={isFetching}
            className="btn-icon p-1.5"
            title="Refresh now"
          >
            <RefreshCw className={`w-4 h-4 ${isFetching ? 'animate-spin' : ''}`} />
          </button>
        </div>
      </div>

      {/* Status line */}
      <div className="flex items-center justify-between text-xs text-theme-text-muted mb-4">
        <span className="flex items-center gap-1.5">
          {polling ? (
            <>
              <span className="w-1.5 h-1.5 rounded-full bg-theme-success animate-pulse" />
              <span>Polling · {interval / 1000}s</span>
            </>
          ) : (
            <>
              <span className="w-1.5 h-1.5 rounded-full bg-theme-text-muted" />
              <span>Stopped</span>
            </>
          )}
        </span>
        <span>Updated {lastUpdate}</span>
      </div>

      {/* Carrier Aggregation (shares polling state with antenna metrics) */}
      {extSignal && (
        <div className="mb-4">
          <CarrierAggregationSection extSignal={extSignal} />
        </div>
      )}

      {!hasAnyData ? (
        <div className="empty-state">
          <Antenna className="w-8 h-8 text-theme-text-muted" />
          <p className="text-sm text-theme-text-secondary">
            {polling ? 'No antenna data available' : 'Antenna metrics are paused'}
          </p>
          <p className="text-xs text-theme-text-muted">
            {polling ? 'Waiting for modem to report antenna data' : 'Press the play button above to start polling'}
          </p>
        </div>
      ) : (
        <>
          {/* Column headers */}
          <div className="grid grid-cols-4 gap-2 mb-2 px-2 text-xs text-theme-text-muted uppercase tracking-wide">
            <span>Port</span>
            <span>RSRP</span>
            <span>RSRQ</span>
            <span>SINR</span>
          </div>

          {/* Antenna rows - grouped by technology */}
          <div className="space-y-4">
            {(() => {
              // Group ports by technology
              const grouped = metrics!.ports.reduce((acc, port) => {
                const tech = port.technology || 'Unknown';
                if (!acc[tech]) acc[tech] = [];
                acc[tech].push(port);
                return acc;
              }, {} as Record<string, typeof metrics.ports>);

              return Object.entries(grouped).map(([tech, ports]) => (
                <div key={tech}>
                  {/* Technology label */}
                  {tech !== 'Unknown' && (
                    <div className="mb-2 px-2 flex items-center gap-2">
                      <span className="text-xs font-semibold text-theme-accent uppercase tracking-wider">
                        {tech === 'NR5G-NSA' ? '5G NSA' : tech === 'NR5G-SA' ? '5G SA' : tech}
                      </span>
                      <div className="flex-1 h-px bg-theme-border" />
                    </div>
                  )}

                  {/* Port rows */}
                  <div className="space-y-1.5">
                    {ports.map((p) => (
                      <div
                        key={`${tech}-${p.port}`}
                        className="grid grid-cols-4 gap-2 items-center p-2 rounded-lg text-sm bg-theme-bg-primary"
                      >
                        <span className="font-medium text-theme-text-primary">RX{p.port}</span>
                        <span className={isSentinel(p.rsrp) ? 'text-theme-text-muted' : qualityToColor(rsrpToQuality(p.rsrp))}>
                          {isSentinel(p.rsrp) ? 'N/A' : p.rsrp}
                        </span>
                        <span className={isSentinel(p.rsrq) ? 'text-theme-text-muted' : qualityToColor(rsrqToQuality(p.rsrq))}>
                          {isSentinel(p.rsrq) ? 'N/A' : p.rsrq}
                        </span>
                        <span className={isSentinel(p.sinr) || p.sinr === 0 ? 'text-theme-text-muted' : qualityToColor(sinrToQuality(p.sinr))}>
                          {isSentinel(p.sinr) || p.sinr === 0 ? 'N/A' : p.sinr}
                        </span>
                      </div>
                    ))}
                  </div>
                </div>
              ));
            })()}
          </div>

          {/* Units legend */}
          <div className="mt-3 text-xs text-theme-text-muted text-center">
            RSRP (dBm) · RSRQ (dB) · SINR (dB)
          </div>
        </>
      )}
    </div>
  );
}
