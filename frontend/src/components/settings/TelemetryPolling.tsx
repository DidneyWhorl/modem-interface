/**
 * TelemetryPolling Component
 *
 * Controls for telemetry heartbeat polling mode:
 * - Normal (30 min) / Fast (2/5/10 min) toggle
 * - Poll Now one-shot button
 * - Auto-revert countdown display
 *
 * Only visible when telemetry is locally enabled.
 */

import { useCallback, useEffect, useRef, useState } from 'react';
import { Activity, Zap } from 'lucide-react';
import clsx from 'clsx';
import {
  getTelemetryConfig,
  getPollingState,
  updatePollingMode,
  triggerPollNow,
  type PollingState,
} from '@/api/telemetry';

interface TelemetryPollingProps {
  /** License must be valid to show polling controls */
  licensed: boolean;
}

const INTERVAL_LABELS: Record<number, string> = {
  120: '2 min',
  300: '5 min',
  600: '10 min',
};

function formatRemaining(secs: number): string {
  const m = Math.floor(secs / 60);
  const s = secs % 60;
  if (m > 0) return `${m}m ${s}s`;
  return `${s}s`;
}

export function TelemetryPolling({ licensed }: TelemetryPollingProps) {
  const [polling, setPolling] = useState<PollingState | null>(null);
  const [telemetryEnabled, setTelemetryEnabled] = useState(false);
  const [loading, setLoading] = useState(false);
  const countdownRef = useRef<ReturnType<typeof setInterval>>();

  // Fetch telemetry config and polling state on mount
  useEffect(() => {
    if (!licensed) return;
    let cancelled = false;
    getTelemetryConfig()
      .then((cfg) => {
        if (cancelled) return;
        setTelemetryEnabled(cfg.local_enabled);
        if (cfg.local_enabled) {
          return getPollingState().then((s) => { if (!cancelled) setPolling(s); });
        }
      })
      .catch(() => { /* silent */ });
    return () => { cancelled = true; };
  }, [licensed]);

  // Countdown timer for fast mode remaining
  useEffect(() => {
    if (countdownRef.current) clearInterval(countdownRef.current);
    if (!polling || polling.mode !== 'fast' || !polling.fast_mode_remaining_secs) return;

    countdownRef.current = setInterval(() => {
      setPolling((prev) => {
        if (!prev || !prev.fast_mode_remaining_secs) return prev;
        const next = prev.fast_mode_remaining_secs - 1;
        if (next <= 0) {
          return { ...prev, mode: 'normal', interval_secs: 1800, fast_mode_remaining_secs: null };
        }
        return { ...prev, fast_mode_remaining_secs: next };
      });
    }, 1000);

    return () => { if (countdownRef.current) clearInterval(countdownRef.current); };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- countdown only restarts on mode change, not on every tick
  }, [polling?.mode]);

  const handleSetFast = useCallback(async (interval: number) => {
    if (loading) return;
    setLoading(true);
    try {
      const updated = await updatePollingMode('fast', interval);
      setPolling(updated);
    } catch { /* silent */ }
    finally { setLoading(false); }
  }, [loading]);

  const handleSetNormal = useCallback(async () => {
    if (loading) return;
    setLoading(true);
    try {
      const updated = await updatePollingMode('normal');
      setPolling(updated);
    } catch { /* silent */ }
    finally { setLoading(false); }
  }, [loading]);

  const handlePollNow = useCallback(async () => {
    if (loading) return;
    setLoading(true);
    try {
      await triggerPollNow();
    } catch { /* silent */ }
    finally { setLoading(false); }
  }, [loading]);

  if (!telemetryEnabled || !polling) return null;

  const isFast = polling.mode === 'fast' && polling.fast_mode_remaining_secs != null;

  return (
    <div className="mt-1">
      {/* Current mode display */}
      <div className="flex items-center gap-2 px-3 py-1.5 text-xs text-theme-text-secondary">
        <Activity className="w-3.5 h-3.5" />
        <span>
          {isFast
            ? `Fast (${INTERVAL_LABELS[polling.interval_secs] ?? `${polling.interval_secs}s`}) — ${formatRemaining(polling.fast_mode_remaining_secs!)} left`
            : 'Normal (30 min)'}
        </span>
      </div>

      {/* Fast mode interval buttons */}
      {!isFast && (
        <div className="flex gap-1 px-3 py-1">
          {(polling.options || [120, 300, 600]).map((opt) => (
            <button
              key={opt}
              onClick={() => handleSetFast(opt)}
              disabled={loading}
              className={clsx(
                'px-2 py-1 text-[10px] rounded border transition-colors',
                'border-theme-border text-theme-text-secondary',
                'hover:border-theme-accent hover:text-theme-text-accent',
                'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-theme-accent',
                loading && 'opacity-50 cursor-wait'
              )}
            >
              {INTERVAL_LABELS[opt] ?? `${opt}s`}
            </button>
          ))}
        </div>
      )}

      {/* Back to Normal button (when in fast mode) */}
      {isFast && (
        <button
          onClick={handleSetNormal}
          disabled={loading}
          className={clsx(
            'w-full flex items-center gap-2 px-3 py-1.5 text-xs',
            'text-theme-warning hover:text-theme-text-primary transition-colors',
            loading && 'opacity-50 cursor-wait'
          )}
        >
          Back to Normal
        </button>
      )}

      {/* Poll Now button */}
      <button
        onClick={handlePollNow}
        disabled={loading}
        className={clsx(
          'w-full flex items-center gap-2 px-3 py-1.5 text-xs',
          'text-theme-text-secondary hover:text-theme-text-accent transition-colors',
          'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-theme-accent',
          loading && 'opacity-50 cursor-wait'
        )}
      >
        <Zap className="w-3.5 h-3.5" />
        <span>Poll Now</span>
      </button>
    </div>
  );
}
