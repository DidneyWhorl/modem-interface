/**
 * CarrierAggregationSection Component
 *
 * Displays carrier aggregation status inline within the Signal panel:
 * - PCC (Primary Component Carrier) row with accent border
 * - SCC (Secondary Component Carrier) rows with color-coded metrics
 * - Column headers: Band | RSRP | RSRQ | SINR
 * - Sentinel value handling (-- for unavailable metrics)
 * - "No carrier aggregation active" state
 * - Network type badge in header
 */

import {
  rsrpToQuality,
  rsrqToQuality,
  sinrToQuality,
  qualityToColor,
  formatSignalValue,
} from '@/lib/signal-utils';
import { Layers } from 'lucide-react';
import type { ExtendedSignalInfo, SignalInfo } from '@/types/api';

interface CarrierAggregationSectionProps {
  extSignal: ExtendedSignalInfo;
}

function CellRow({ label, cell, isPrimary }: { label: string; cell: SignalInfo; isPrimary: boolean }) {
  return (
    <div
      className={`rounded-lg p-2 text-xs grid grid-cols-4 gap-2 items-center ${
        isPrimary
          ? 'bg-theme-bg-secondary border-l-2 border-theme-accent'
          : 'bg-theme-bg-primary'
      }`}
    >
      <span className="text-theme-text-secondary">
        <span className="font-medium">{label}</span> {cell.band || 'N/A'}
      </span>
      <span className={qualityToColor(rsrpToQuality(cell.rsrp))}>
        {formatSignalValue(cell.rsrp, 'dbm')}
      </span>
      <span className={qualityToColor(rsrqToQuality(cell.rsrq))}>
        {formatSignalValue(cell.rsrq, 'db')}
      </span>
      <span className={qualityToColor(sinrToQuality(cell.sinr))}>
        {formatSignalValue(cell.sinr, 'db')}
      </span>
    </div>
  );
}

export function CarrierAggregationSection({ extSignal }: CarrierAggregationSectionProps) {
  const isActive = extSignal.carrier_aggregation && extSignal.secondary_cells.length > 0;
  const sccCount = extSignal.secondary_cells.length;

  return (
    <div className="mt-4 pt-4 border-t border-theme-border">
      {/* Header */}
      <div className="flex items-center justify-between mb-2">
        <div className="flex items-center gap-1 text-xs font-medium text-theme-text-secondary uppercase">
          <Layers className="w-3 h-3" />
          Carrier Aggregation
          {isActive && (
            <span className="normal-case ml-1">
              (1 PCC + {sccCount} SCC{sccCount !== 1 ? 's' : ''})
            </span>
          )}
        </div>
        {extSignal.network_type && (
          <span className="text-xs font-medium text-theme-text-accent">
            {extSignal.network_type}
          </span>
        )}
      </div>

      {!isActive ? (
        <p className="text-xs text-theme-text-muted">No carrier aggregation active</p>
      ) : (
        <>
          {/* Column headers */}
          <div className="grid grid-cols-4 gap-2 px-2 mb-1 text-[10px] text-theme-text-muted uppercase tracking-wide">
            <span>Band</span>
            <span>RSRP</span>
            <span>RSRQ</span>
            <span>SINR</span>
          </div>

          {/* PCC + SCC rows */}
          <div className="space-y-1.5">
            <CellRow label="PCC" cell={extSignal.primary} isPrimary />
            {extSignal.secondary_cells.map((cell, i) => (
              <CellRow key={i} label={`SCC${i + 1}`} cell={cell} isPrimary={false} />
            ))}
          </div>
        </>
      )}
    </div>
  );
}
