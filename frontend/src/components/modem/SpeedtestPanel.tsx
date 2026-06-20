/**
 * SpeedtestPanel
 *
 * Displays speed test controls with a streaming line chart during tests,
 * detailed results after completion, phase indicators, and a history table.
 */

import { useState, useEffect, useCallback, useMemo, useRef } from 'react';
import { useSpeedtestHistory, useRunSpeedtest } from '@/hooks/queries/useSpeedtest';
import { useWanStatus } from '@/hooks/queries/useWanStatus';
import { Gauge, Play, X } from 'lucide-react';
import { SpeedChart } from './SpeedChart';
import { SpeedtestResults } from './SpeedtestResults';
import type { SpeedDataPoint, SpeedtestMode, SpeedtestPhase, SpeedtestProgress, SpeedtestResult } from '@/types/api';

// ---------------------------------------------------------------------------
// Phase Progress Bar
// ---------------------------------------------------------------------------

const PHASE_LABELS: Record<SpeedtestPhase, string> = {
  latency: 'Measuring Latency...',
  download: 'Download...',
  upload: 'Upload...',
};

function PhaseBar({
  activePhase,
  progress,
  completedValues,
}: {
  activePhase: SpeedtestPhase | null;
  progress: number;
  completedValues: Partial<Record<SpeedtestPhase, string>>;
}) {
  const phases: SpeedtestPhase[] = ['latency', 'download', 'upload'];

  return (
    <div className="flex flex-col gap-1 w-full px-2">
      {phases.map((p) => {
        const done = completedValues[p] != null;
        const active = p === activePhase;
        return (
          <div key={p} className="flex items-center gap-2 text-xs">
            <span className={`w-20 text-right ${active ? 'text-theme-accent font-semibold' : done ? 'text-theme-text-secondary' : 'text-theme-text-muted'}`}>
              {done ? completedValues[p] : active ? PHASE_LABELS[p] : p.charAt(0).toUpperCase() + p.slice(1)}
            </span>
            <div className="flex-1 h-1.5 bg-theme-bg-tertiary rounded-full overflow-hidden">
              <div
                className="h-full rounded-full transition-all duration-300"
                style={{
                  width: done ? '100%' : active ? `${progress}%` : '0%',
                  background: done
                    ? 'oklch(0.75 0.2 145)'
                    : active
                    ? 'oklch(0.8 0.18 85)'
                    : 'transparent',
                }}
              />
            </div>
          </div>
        );
      })}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Main Panel
// ---------------------------------------------------------------------------

export function SpeedtestPanel() {
  const { data: history } = useSpeedtestHistory(10);
  const { data: wanStatus } = useWanStatus();
  const runMutation = useRunSpeedtest();

  const [selectedWan, setSelectedWan] = useState<string>('');
  const [running, setRunning] = useState(false);
  const [activePhase, setActivePhase] = useState<SpeedtestPhase | null>(null);
  const [progressPct, setProgressPct] = useState(0);
  const [, setCurrentSpeed] = useState(0);
  const [completedValues, setCompletedValues] = useState<Partial<Record<SpeedtestPhase, string>>>({});
  const [finalResult, setFinalResult] = useState<SpeedtestResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const startTimeRef = useRef<number>(0);
  const runningRef = useRef(false);
  const [elapsedSeconds, setElapsedSeconds] = useState(0);
  const [chartPoints, setChartPoints] = useState<SpeedDataPoint[]>([]);
  const [chartDirection, setChartDirection] = useState<'download' | 'upload' | 'idle'>('idle');
  const chartDirectionRef = useRef<'download' | 'upload' | 'idle'>('idle');
  const [modalResult, setModalResult] = useState<SpeedtestResult | null>(null);

  // Show most recent history result on load (persist across reloads)
  useEffect(() => {
    if (!finalResult && !running && history?.results?.length) {
      setFinalResult(history.results[0]!);
    }
  }, [history, finalResult, running]);

  // Auto-select first WAN if none selected
  const wanModems = useMemo(() => wanStatus?.modems ?? [], [wanStatus?.modems]);
  useEffect(() => {
    if (!selectedWan && wanModems.length > 0) {
      setSelectedWan(wanModems[0]!.modem_id);
    }
  }, [selectedWan, wanModems]);

  // Close modal on Escape key
  useEffect(() => {
    if (!modalResult) return;
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape') setModalResult(null); };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [modalResult]);

  // Listen for WS events via DOM custom events
  const handleProgress = useCallback((e: Event) => {
    if (!runningRef.current) return; // Ignore late events after completion
    const p = (e as CustomEvent<SpeedtestProgress>).detail;
    setActivePhase(p.phase);
    setProgressPct(p.progress_pct);
    setCurrentSpeed(p.current_speed_mbps);

    // Build chart data from progress events
    if (p.phase === 'download' || p.phase === 'upload') {
      const dir = p.phase as 'download' | 'upload';
      setChartDirection(dir);

      if (p.running_p90_mbps != null) {
        const elapsed = Date.now() - startTimeRef.current;
        setChartPoints(prev => {
          // Reset when direction changes (download → upload)
          if (chartDirectionRef.current !== dir) {
            chartDirectionRef.current = dir;
            return [{ timestamp: elapsed, speed: p.current_speed_mbps, p90: p.running_p90_mbps! }];
          }
          return [...prev, { timestamp: elapsed, speed: p.current_speed_mbps, p90: p.running_p90_mbps! }];
        });
      }
    }

    // Lock in previous phase value when a new phase starts
    if (p.phase === 'download') {
      setCompletedValues((prev) => prev.latency != null ? prev : { ...prev });
    }
    if (p.phase === 'upload') {
      setCompletedValues((prev) => prev.download != null ? prev : { ...prev });
    }
  }, []);

  const handleComplete = useCallback((e: Event) => {
    const result = (e as CustomEvent<SpeedtestResult>).detail;
    runningRef.current = false;
    setFinalResult(result);
    setRunning(false);
    setActivePhase(null);
    setCurrentSpeed(result.download_mbps);
    setCompletedValues({
      latency: `${result.latency_ms.toFixed(0)} ms`,
      download: `${result.download_mbps.toFixed(1)} Mbps`,
      upload: `${result.upload_mbps.toFixed(1)} Mbps`,
    });
  }, []);

  const handleError = useCallback((e: Event) => {
    const detail = (e as CustomEvent<{ test_id: string; error: string }>).detail;
    runningRef.current = false;
    setError(detail.error);
    setRunning(false);
    setActivePhase(null);
  }, []);

  // Elapsed timer while running
  useEffect(() => {
    if (!running) return;
    const interval = setInterval(() => {
      setElapsedSeconds(Math.floor((Date.now() - startTimeRef.current) / 1000));
    }, 1000);
    return () => clearInterval(interval);
  }, [running]);

  useEffect(() => {
    window.addEventListener('speedtest-progress', handleProgress);
    window.addEventListener('speedtest-complete', handleComplete);
    window.addEventListener('speedtest-error', handleError);
    return () => {
      window.removeEventListener('speedtest-progress', handleProgress);
      window.removeEventListener('speedtest-complete', handleComplete);
      window.removeEventListener('speedtest-error', handleError);
    };
  }, [handleProgress, handleComplete, handleError]);

  const startTest = (mode: SpeedtestMode) => {
    if (!selectedWan || running) return;
    setError(null);
    setFinalResult(null);
    setCompletedValues({});
    setCurrentSpeed(0);
    setProgressPct(0);
    setActivePhase(null);
    setChartPoints([]);
    setChartDirection('idle');
    chartDirectionRef.current = 'idle';
    runningRef.current = true;
    setRunning(true);
    startTimeRef.current = Date.now();
    setElapsedSeconds(0);
    runMutation.mutate({ mode, wanId: selectedWan });
  };

  const results = history?.results ?? [];

  return (
    <div className="px-3 pb-3 space-y-3">
      {/* Controls row */}
      <div className="flex items-center gap-2 flex-wrap">
        <select
          value={selectedWan}
          onChange={(e) => setSelectedWan(e.target.value)}
          disabled={running}
          className="input-field text-sm flex-1 min-w-[140px]"
        >
          {wanModems.map((w) => (
            <option key={w.modem_id} value={w.modem_id}>
              {w.label || w.modem_id}
            </option>
          ))}
          {wanModems.length === 0 && <option value="">No WANs available</option>}
        </select>
        <div className="flex flex-col items-center gap-0.5">
          <button
            onClick={() => startTest('quick')}
            disabled={running || !selectedWan}
            className="btn-primary text-sm flex items-center gap-1"
          >
            <Play className="w-3.5 h-3.5" /> Quick
          </button>
          <span className="text-[10px] text-theme-text-muted">~15 MB</span>
        </div>
        <div className="flex flex-col items-center gap-0.5">
          <button
            onClick={() => startTest('medium')}
            disabled={running || !selectedWan}
            className="btn-secondary text-sm flex items-center gap-1"
          >
            <Gauge className="w-3.5 h-3.5" /> Medium
          </button>
          <span className="text-[10px] text-theme-text-muted">~80 MB</span>
        </div>
        <div className="flex flex-col items-center gap-0.5">
          <button
            onClick={() => startTest('full')}
            disabled={running || !selectedWan}
            className="btn-secondary text-sm flex items-center gap-1"
          >
            <Gauge className="w-3.5 h-3.5" /> Full
          </button>
          <span className="text-[10px] text-theme-text-muted">~100-400 MB</span>
        </div>
        <p className="text-[10px] text-theme-text-muted text-center w-full">
          Running many tests in quick succession may trigger rate limiting.
        </p>
      </div>

      {error && (
        <div className="text-xs text-theme-error bg-theme-error/10 rounded px-2 py-1">{error}</div>
      )}

      {/* Chart + results (show when running or after completion) */}
      {(running || finalResult) && (
        <div className="flex flex-col items-center gap-2 w-full">
          {running ? (
            <>
              <SpeedChart
                points={chartPoints}
                phaseLabel={activePhase ? PHASE_LABELS[activePhase] : 'Starting...'}
                running={true}
                direction={chartDirection}
              />
              <span className="text-xs text-theme-text-muted tabular-nums">
                {Math.floor(elapsedSeconds / 60)}:{String(elapsedSeconds % 60).padStart(2, '0')}
              </span>
            </>
          ) : finalResult ? (
            <SpeedtestResults result={finalResult} />
          ) : null}

          <PhaseBar
            activePhase={activePhase}
            progress={progressPct}
            completedValues={completedValues}
          />
        </div>
      )}

      {/* History table */}
      {results.length > 0 && (
        <div className="overflow-x-auto">
          <table className="w-full text-xs">
            <thead>
              <tr className="text-theme-text-muted border-b border-theme-border">
                <th className="text-left py-1 pr-2 font-medium">Time</th>
                <th className="text-left py-1 pr-2 font-medium">WAN</th>
                <th className="text-left py-1 pr-2 font-medium">Mode</th>
                <th className="text-right py-1 pr-2 font-medium">Down</th>
                <th className="text-right py-1 pr-2 font-medium">Up</th>
                <th className="text-right py-1 font-medium">Latency</th>
              </tr>
            </thead>
            <tbody>
              {results.map((r) => (
                <tr key={r.id} className="border-b border-theme-border/50 cursor-pointer hover:bg-theme-bg-tertiary" onClick={() => setModalResult(r)}>
                  <td className="py-1 pr-2 text-theme-text-secondary whitespace-nowrap">
                    {new Date(r.timestamp).toLocaleString(undefined, {
                      month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit',
                    })}
                  </td>
                  <td className="py-1 pr-2 text-theme-text-secondary truncate max-w-[100px]">{r.wan_name}</td>
                  <td className="py-1 pr-2 text-theme-text-secondary">{r.mode}</td>
                  <td className="py-1 pr-2 text-right text-theme-text-primary tabular-nums">{r.download_mbps.toFixed(1)}</td>
                  <td className="py-1 pr-2 text-right text-theme-text-primary tabular-nums">{r.upload_mbps.toFixed(1)}</td>
                  <td className="py-1 text-right text-theme-text-primary tabular-nums">{r.latency_ms.toFixed(0)} ms</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {/* History detail modal */}
      {modalResult && (
        <div className="fixed inset-0 z-50 flex items-center justify-center">
          <div className="absolute inset-0 bg-black/50 backdrop-blur-sm" onClick={() => setModalResult(null)} />
          <div className="bg-theme-bg-primary rounded-lg shadow-xl max-w-md w-full mx-4 p-4 max-h-[80vh] overflow-y-auto relative z-10">
            <div className="flex items-center justify-between mb-3">
              <div className="flex items-center gap-2">
                <span className="text-sm font-semibold text-theme-text-primary">
                  {new Date(modalResult.timestamp).toLocaleString(undefined, {
                    month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit',
                  })}
                </span>
                <span className="text-[10px] px-1.5 py-0.5 rounded bg-theme-bg-tertiary text-theme-text-secondary capitalize">
                  {modalResult.mode}
                </span>
              </div>
              <button onClick={() => setModalResult(null)} className="p-1 hover:bg-theme-bg-tertiary rounded">
                <X className="w-4 h-4 text-theme-text-muted" />
              </button>
            </div>
            <SpeedtestResults result={modalResult} />
          </div>
        </div>
      )}
    </div>
  );
}
