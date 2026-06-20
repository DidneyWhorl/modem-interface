/**
 * SignalTrendChart — Reusable single-metric trending chart.
 *
 * Renders a recharts LineChart for one signal metric (RSRP, RSRQ, or SINR).
 */

import {
  ResponsiveContainer,
  LineChart,
  Line,
  XAxis,
  YAxis,
  Tooltip,
  CartesianGrid,
} from 'recharts';
import type { SignalSample } from '@/types/api';

interface SignalTrendChartProps {
  samples: SignalSample[];
  dataKey: keyof SignalSample;
  label: string;
  unit: string;
  color: string;
  domain: [number, number];
}

function formatTime(ts: number): string {
  const d = new Date(ts * 1000);
  return d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
}

function formatTooltipTime(ts: number): string {
  const d = new Date(ts * 1000);
  return d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' });
}

export function SignalTrendChart({ samples, dataKey, label, unit, color, domain }: SignalTrendChartProps) {
  const lastSample = samples.length > 0 ? samples[samples.length - 1] : undefined;
  const currentValue = lastSample !== undefined ? lastSample[dataKey] : null;

  return (
    <div>
      <div className="flex items-center justify-between mb-1 px-1">
        <span className="text-xs font-medium text-theme-text-secondary">{label}</span>
        <span className="text-xs text-theme-text-muted">
          {currentValue !== null ? `${currentValue} ${unit}` : '--'}
        </span>
      </div>
      {samples.length === 0 ? (
        <div className="flex items-center justify-center h-[120px] text-xs text-theme-text-muted">
          Collecting data...
        </div>
      ) : (
        <ResponsiveContainer width="100%" height={120}>
          <LineChart data={samples} margin={{ top: 4, right: 8, bottom: 4, left: -16 }}>
            <CartesianGrid strokeDasharray="3 3" stroke="rgba(128,128,128,0.2)" />
            <XAxis
              dataKey="ts"
              tickFormatter={formatTime}
              tick={{ fontSize: 10, fill: 'var(--color-text-muted)' }}
              stroke="var(--color-border)"
              minTickGap={40}
            />
            <YAxis
              domain={domain}
              tick={{ fontSize: 10, fill: 'var(--color-text-muted)' }}
              stroke="var(--color-border)"
              width={40}
            />
            <Tooltip
              labelFormatter={(ts) => formatTooltipTime(ts as number)}
              formatter={(value: number) => [`${value} ${unit}`, label]}
              contentStyle={{
                backgroundColor: 'var(--color-bg-card)',
                border: '1px solid var(--color-border)',
                borderRadius: '6px',
                fontSize: '12px',
              }}
            />
            <Line
              type="monotone"
              dataKey={dataKey}
              stroke={color}
              strokeWidth={1.5}
              dot={false}
              isAnimationActive={false}
            />
          </LineChart>
        </ResponsiveContainer>
      )}
    </div>
  );
}
