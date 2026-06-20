/**
 * SpeedChart — Streaming line chart for speedtest progress.
 *
 * Replaces the arc gauge with an SVG area chart that grows rightward as
 * measurements arrive. Shows the running p90 as a filled area, a pulsing
 * dot at the leading edge, and a large speed readout above.
 */

import type { SpeedDataPoint } from '@/types/api';

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

interface SpeedChartProps {
  points: SpeedDataPoint[];
  phaseLabel: string;
  running: boolean;
  headlineSpeed?: number; // Final speed shown post-test
  direction: 'download' | 'upload' | 'idle';
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const VIEW_W = 320;
const VIEW_H = 120;
const PAD_X = 32;
const PAD_Y = 16;
const PLOT_W = VIEW_W - PAD_X * 2; // 256
const PLOT_H = VIEW_H - PAD_Y * 2; // 88

const NICE_STEPS = [10, 25, 50, 100, 200, 300, 500, 750, 1000];

const COLOR_DOWNLOAD = 'oklch(0.7 0.15 220)';
const COLOR_UPLOAD = 'oklch(0.75 0.15 55)';
const COLOR_IDLE = 'oklch(0.55 0.05 220)';

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function niceMax(rawMax: number): number {
  for (const step of NICE_STEPS) {
    if (step >= rawMax) return step;
  }
  return NICE_STEPS[NICE_STEPS.length - 1]!;
}

function toSvgX(ts: number, minTs: number, rangeTs: number): number {
  if (rangeTs === 0) return PAD_X;
  return PAD_X + ((ts - minTs) / rangeTs) * PLOT_W;
}

function toSvgY(speed: number, maxSpeed: number): number {
  const clamped = Math.min(Math.max(speed, 0), maxSpeed);
  return PAD_Y + PLOT_H - (clamped / maxSpeed) * PLOT_H;
}

function buildLinePath(
  points: SpeedDataPoint[],
  minTs: number,
  rangeTs: number,
  maxSpeed: number,
): string {
  return points
    .map((p, i) => {
      const x = toSvgX(p.timestamp, minTs, rangeTs);
      const y = toSvgY(p.p90, maxSpeed);
      return `${i === 0 ? 'M' : 'L'} ${x.toFixed(2)} ${y.toFixed(2)}`;
    })
    .join(' ');
}

function buildAreaPath(
  points: SpeedDataPoint[],
  minTs: number,
  rangeTs: number,
  maxSpeed: number,
): string {
  if (points.length === 0) return '';
  const line = buildLinePath(points, minTs, rangeTs, maxSpeed);
  const lastX = toSvgX(points[points.length - 1]!.timestamp, minTs, rangeTs).toFixed(2);
  const firstX = toSvgX(points[0]!.timestamp, minTs, rangeTs).toFixed(2);
  const baseY = (PAD_Y + PLOT_H).toFixed(2);
  return `${line} L ${lastX} ${baseY} L ${firstX} ${baseY} Z`;
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

export function SpeedChart({
  points,
  phaseLabel,
  running,
  headlineSpeed,
  direction,
}: SpeedChartProps) {
  // Determine color
  const lineColor =
    direction === 'download'
      ? COLOR_DOWNLOAD
      : direction === 'upload'
      ? COLOR_UPLOAD
      : COLOR_IDLE;

  // Displayed speed value
  const lastP90 = points.length > 0 ? points[points.length - 1]!.p90 : 0;
  const displaySpeed = headlineSpeed !== undefined ? headlineSpeed : lastP90;

  // Y-axis scale
  const rawMax = Math.max(lastP90, headlineSpeed ?? 0, 10);
  const maxSpeed = niceMax(rawMax);

  // Time range for X axis
  const minTs = points.length > 0 ? points[0]!.timestamp : 0;
  const maxTs = points.length > 0 ? points[points.length - 1]!.timestamp : 0;
  const rangeTs = Math.max(maxTs - minTs, 1);

  // SVG paths
  const linePath = points.length >= 2 ? buildLinePath(points, minTs, rangeTs, maxSpeed) : '';
  const areaPath = points.length >= 2 ? buildAreaPath(points, minTs, rangeTs, maxSpeed) : '';

  // Leading dot position
  const dotX = points.length > 0 ? toSvgX(maxTs, minTs, rangeTs) : PAD_X + PLOT_W;
  const dotY = points.length > 0 ? toSvgY(lastP90, maxSpeed) : PAD_Y + PLOT_H;

  // Y-axis ticks: 5 evenly spaced from 0..maxSpeed
  const yTicks = Array.from({ length: 5 }, (_, i) =>
    Math.round((maxSpeed / 4) * i),
  );

  // Unique IDs to avoid SVG gradient collisions between instances
  const gradId = `sc-area-${direction}`;

  return (
    <div className="flex flex-col items-center w-full max-w-sm">
      {/* Speed readout */}
      <div className="text-center mb-1">
        <span className="text-3xl font-bold text-theme-text-primary tabular-nums">
          {displaySpeed.toFixed(1)}
        </span>
        <span className="text-sm text-theme-text-muted ml-1">Mbps</span>
      </div>

      {/* Phase label */}
      <div className="text-xs text-theme-text-muted mb-2">{phaseLabel}</div>

      {/* Chart */}
      <svg
        viewBox={`0 0 ${VIEW_W} ${VIEW_H}`}
        className="w-full"
        style={{ overflow: 'visible' }}
      >
        <defs>
          <linearGradient id={gradId} x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stopColor={lineColor} stopOpacity="0.15" />
            <stop offset="100%" stopColor={lineColor} stopOpacity="0.02" />
          </linearGradient>
        </defs>

        {/* Y-axis grid lines and labels */}
        {yTicks.map((tick) => {
          const y = toSvgY(tick, maxSpeed);
          return (
            <g key={tick}>
              <line
                x1={PAD_X}
                y1={y}
                x2={PAD_X + PLOT_W}
                y2={y}
                stroke="currentColor"
                className="text-theme-border"
                strokeWidth="0.5"
                strokeDasharray="3 3"
              />
              <text
                x={PAD_X - 4}
                y={y}
                textAnchor="end"
                dominantBaseline="middle"
                fontSize="8"
                className="fill-theme-text-muted"
              >
                {tick >= 1000 ? `${(tick / 1000).toFixed(0)}k` : tick}
              </text>
            </g>
          );
        })}

        {/* Chart border lines (bottom + left) */}
        <line
          x1={PAD_X}
          y1={PAD_Y + PLOT_H}
          x2={PAD_X + PLOT_W}
          y2={PAD_Y + PLOT_H}
          stroke="currentColor"
          className="text-theme-border"
          strokeWidth="1"
        />
        <line
          x1={PAD_X}
          y1={PAD_Y}
          x2={PAD_X}
          y2={PAD_Y + PLOT_H}
          stroke="currentColor"
          className="text-theme-border"
          strokeWidth="1"
        />

        {/* Area fill */}
        {areaPath && (
          <path d={areaPath} fill={`url(#${gradId})`} />
        )}

        {/* P90 line */}
        {linePath && (
          <path
            d={linePath}
            fill="none"
            stroke={lineColor}
            strokeWidth="2"
            strokeLinejoin="round"
            strokeLinecap="round"
          />
        )}

        {/* Pulsing leading dot (only when running and we have data) */}
        {running && points.length > 0 && (
          <g>
            {/* Outer pulse ring */}
            <circle cx={dotX} cy={dotY} r="5" fill={lineColor} opacity="0.25">
              <animate
                attributeName="r"
                values="4;8;4"
                dur="1.4s"
                repeatCount="indefinite"
              />
              <animate
                attributeName="opacity"
                values="0.3;0;0.3"
                dur="1.4s"
                repeatCount="indefinite"
              />
            </circle>
            {/* Inner solid dot */}
            <circle cx={dotX} cy={dotY} r="3" fill={lineColor} />
          </g>
        )}

        {/* Static dot when not running but we have data */}
        {!running && points.length > 0 && (
          <circle cx={dotX} cy={dotY} r="3" fill={lineColor} />
        )}
      </svg>
    </div>
  );
}
