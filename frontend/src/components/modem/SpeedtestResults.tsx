/**
 * SpeedtestResults
 *
 * Post-test detailed results panel. Renders hero speeds, latency metrics,
 * AIM quality scores, and connection info. Gracefully omits any section
 * whose data is absent (Quick mode produces minimal results).
 */

import type { SpeedtestResult } from '@/types/api';

// ---------------------------------------------------------------------------
// Helper: MetricRow
// ---------------------------------------------------------------------------

interface MetricRowProps {
  label: string;
  value: string;
  valueClass?: string;
}

function MetricRow({ label, value, valueClass }: MetricRowProps) {
  return (
    <div className="flex justify-between items-baseline gap-2 text-xs">
      <span className="text-theme-text-muted">{label}</span>
      <span className={`font-semibold tabular-nums ${valueClass ?? 'text-theme-text-primary'}`}>
        {value}
      </span>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Helper: ScoreBadge
// ---------------------------------------------------------------------------

type AimRating = 'bad' | 'poor' | 'average' | 'good' | 'great';

const SCORE_COLOR: Record<AimRating, string> = {
  great:   'text-green-400',
  good:    'text-emerald-400',
  average: 'text-yellow-400',
  poor:    'text-orange-400',
  bad:     'text-red-400',
};

function scoreColor(rating: string): string {
  return SCORE_COLOR[rating as AimRating] ?? 'text-theme-text-secondary';
}

interface ScoreBadgeProps {
  label: string;
  rating: string;
}

function ScoreBadge({ label, rating }: ScoreBadgeProps) {
  return (
    <div className="flex flex-col items-center gap-0.5">
      <span className="text-[10px] text-theme-text-muted uppercase tracking-wide">{label}</span>
      <span className={`text-sm font-semibold capitalize ${scoreColor(rating)}`}>{rating}</span>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Helper: bufferbloat color
// ---------------------------------------------------------------------------

function bufferbloatClass(ms: number): string {
  if (ms > 50) return 'text-orange-400';
  if (ms > 20) return 'text-yellow-400';
  return 'text-green-400';
}

// ---------------------------------------------------------------------------
// Main Component
// ---------------------------------------------------------------------------

interface SpeedtestResultsProps {
  result: SpeedtestResult;
}

export function SpeedtestResults({ result }: SpeedtestResultsProps) {
  const {
    download_mbps,
    upload_mbps,
    latency_ms,
    jitter_ms,
    download_loaded_latency_ms,
    upload_loaded_latency_ms,
    bufferbloat_ms,
    tcp_loss_ratio,
    scores,
    connection,
    bytes_consumed,
    mode,
    server,
  } = result;

  return (
    <div className="space-y-3">
      {/* 1. Hero speeds */}
      <div className="grid grid-cols-2 gap-3">
        {[
          { label: 'Download', value: download_mbps },
          { label: 'Upload', value: upload_mbps },
        ].map(({ label, value }) => (
          <div
            key={label}
            className="rounded-lg bg-theme-bg-secondary px-4 py-3 flex flex-col items-center"
          >
            <span className="text-xs text-theme-text-muted mb-1">{label}</span>
            <div className="flex items-baseline gap-1">
              <span className="text-3xl font-bold text-theme-text-primary tabular-nums">
                {value.toFixed(1)}
              </span>
              <span className="text-sm text-theme-text-muted">Mbps</span>
            </div>
          </div>
        ))}
      </div>

      {/* 2. Latency panel */}
      <div className="rounded-lg bg-theme-bg-secondary px-4 py-3 space-y-1.5">
        <div className="grid grid-cols-2 gap-x-6 gap-y-1.5">
          <MetricRow label="Latency" value={`${latency_ms.toFixed(0)} ms`} />
          <MetricRow label="Jitter" value={`${jitter_ms.toFixed(1)} ms`} />
          {download_loaded_latency_ms != null && (
            <MetricRow label="DL Loaded" value={`${download_loaded_latency_ms.toFixed(0)} ms`} />
          )}
          {upload_loaded_latency_ms != null && (
            <MetricRow label="UL Loaded" value={`${upload_loaded_latency_ms.toFixed(0)} ms`} />
          )}
          {bufferbloat_ms != null && (
            <MetricRow
              label="Bufferbloat"
              value={`+${bufferbloat_ms.toFixed(0)} ms`}
              valueClass={bufferbloatClass(bufferbloat_ms)}
            />
          )}
          {tcp_loss_ratio != null && (
            <MetricRow
              label="Packet Loss"
              value={`${(tcp_loss_ratio * 100).toFixed(2)}%`}
            />
          )}
        </div>
      </div>

      {/* 3. AIM Quality Scores */}
      {scores != null && (
        <div className="rounded-lg bg-theme-bg-secondary px-4 py-3">
          <p className="text-[10px] text-theme-text-muted uppercase tracking-wide mb-2">
            AIM Quality
          </p>
          <div className="grid grid-cols-3 gap-2">
            <ScoreBadge label="Streaming" rating={scores.streaming} />
            <ScoreBadge label="Gaming" rating={scores.gaming} />
            <ScoreBadge label="Video Calls" rating={scores.video_calls} />
          </div>
        </div>
      )}

      {/* 4. Connection Info */}
      {connection != null && (
        <div className="rounded-lg bg-theme-bg-secondary px-4 py-3 space-y-1.5">
          <p className="text-[10px] text-theme-text-muted uppercase tracking-wide mb-2">
            Connection
          </p>
          <div className="grid grid-cols-2 gap-x-6 gap-y-1.5">
            {(connection.city ?? connection.colo) != null && (
              <MetricRow label="Server" value={connection.city ?? connection.colo ?? ''} />
            )}
            {connection.asn_name != null && (
              <MetricRow label="ISP" value={connection.asn_name} />
            )}
            {connection.ip != null && (
              <MetricRow label="IP" value={connection.ip} />
            )}
            {connection.asn != null && (
              <MetricRow label="ASN" value={`AS${connection.asn}`} />
            )}
          </div>
        </div>
      )}

      {/* 5. Summary row */}
      <div className="grid grid-cols-3 gap-2 text-center">
        <div>
          <div className="text-[10px] text-theme-text-muted uppercase tracking-wide mb-0.5">
            Server
          </div>
          <div className="text-xs font-semibold text-theme-text-primary truncate">{server}</div>
        </div>
        <div>
          <div className="text-[10px] text-theme-text-muted uppercase tracking-wide mb-0.5">
            Data Used
          </div>
          <div className="text-xs font-semibold text-theme-text-primary tabular-nums">
            {(bytes_consumed / 1_048_576).toFixed(1)} MB
          </div>
        </div>
        <div>
          <div className="text-[10px] text-theme-text-muted uppercase tracking-wide mb-0.5">
            Mode
          </div>
          <div className="text-xs font-semibold text-theme-text-primary capitalize">{mode}</div>
        </div>
      </div>
    </div>
  );
}
