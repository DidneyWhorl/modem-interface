/**
 * SteeringRuleCard
 *
 * Compact single-row card for a traffic steering rule.
 * Displays match criteria, target WAN, and action buttons.
 */

import { useState } from 'react';
import { GripVertical, ArrowRight, Pencil, Trash2 } from 'lucide-react';
import clsx from 'clsx';
import type { SteeringRule, PortMatch, RuleStatus } from '@/types/steering';

interface SteeringRuleCardProps {
  rule: SteeringRule;
  onEdit: (rule: SteeringRule) => void;
  onDelete: (id: string) => void;
  onToggleEnabled: (id: string, enabled: boolean) => void;
  disabled?: boolean;
}

function formatPort(port: PortMatch): string {
  if (Array.isArray(port)) {
    return `${port[0]}-${port[1]}`;
  }
  return String(port);
}

function buildMatchSummary(rule: SteeringRule): string {
  const parts: string[] = [];

  if (rule.protocol) {
    parts.push(rule.protocol.toUpperCase());
  }

  if (rule.destination_port) {
    parts.push(formatPort(rule.destination_port));
  } else if (rule.source_port) {
    parts.push(`sport ${formatPort(rule.source_port)}`);
  }

  if (rule.source_ip && rule.source_ip.length > 0) {
    parts.push(`from ${rule.source_ip.join(', ')}`);
  }

  if (rule.destination_ip && rule.destination_ip.length > 0) {
    parts.push(`to ${rule.destination_ip.join(', ')}`);
  }

  return parts.length > 0 ? parts.join(' ') : 'All traffic';
}

const statusConfig: Record<RuleStatus, { color: string; label: string }> = {
  active: { color: 'bg-emerald-500', label: 'Rule is active and enforced' },
  dormant: { color: 'bg-amber-500', label: 'Target WAN is unavailable — rule dormant' },
  blocked: { color: 'bg-red-500', label: 'Rule is blocked or has errors' },
};

export default function SteeringRuleCard({
  rule,
  onEdit,
  onDelete,
  onToggleEnabled,
  disabled,
}: SteeringRuleCardProps) {
  const [confirmDelete, setConfirmDelete] = useState(false);
  const status = statusConfig[rule.status] ?? statusConfig.dormant;
  const targetLabel = rule.target_wan_label ?? rule.target_wan;

  return (
    <div
      className={clsx(
        'border border-theme-border rounded-lg p-2 flex items-center gap-2',
        'bg-theme-bg-secondary',
        !rule.enabled && 'opacity-50',
      )}
    >
      {/* Drag handle */}
      <div className="steering-drag-handle cursor-grab text-theme-text-muted hover:text-theme-text-secondary">
        <GripVertical className="w-4 h-4" />
      </div>

      {/* Status dot */}
      <div className="relative group">
        <div className={clsx('w-2 h-2 rounded-full flex-shrink-0', status.color)} />
        <div className="absolute bottom-full left-1/2 -translate-x-1/2 mb-1 px-2 py-1 text-[10px] bg-theme-bg-primary border border-theme-border rounded shadow-lg whitespace-nowrap opacity-0 group-hover:opacity-100 transition-opacity pointer-events-none z-10">
          {status.label}
        </div>
      </div>

      {/* Rule name */}
      <span className="font-medium text-xs text-theme-text-primary truncate min-w-0 max-w-[120px]">
        {rule.name}
      </span>

      {/* Match summary */}
      <span className="text-[10px] text-theme-text-secondary truncate min-w-0 flex-1">
        {buildMatchSummary(rule)}
      </span>

      {/* Target WAN */}
      <div className="flex items-center gap-1 flex-shrink-0">
        <ArrowRight className="w-3 h-3 text-theme-text-muted" />
        <span className="text-[10px] text-theme-text-secondary truncate max-w-[80px]">
          {targetLabel}
        </span>
      </div>

      {/* Enable/disable toggle */}
      <label className="flex-shrink-0 cursor-pointer">
        <input
          type="checkbox"
          checked={rule.enabled}
          onChange={(e) => onToggleEnabled(rule.id, e.target.checked)}
          disabled={disabled}
          className="accent-theme-accent w-3.5 h-3.5"
        />
      </label>

      {/* Edit */}
      <button
        onClick={() => onEdit(rule)}
        disabled={disabled}
        className="btn-ghost !p-1 text-theme-text-muted hover:text-theme-text-primary"
        title="Edit rule"
      >
        <Pencil className="w-3.5 h-3.5" />
      </button>

      {/* Delete */}
      {confirmDelete ? (
        <div className="flex items-center gap-1 flex-shrink-0">
          <span className="text-[10px] text-theme-error">Delete?</span>
          <button
            onClick={() => { onDelete(rule.id); setConfirmDelete(false); }}
            className="btn-ghost !p-0.5 text-theme-error"
            title="Confirm delete"
          >
            <Trash2 className="w-3 h-3" />
          </button>
          <button
            onClick={() => setConfirmDelete(false)}
            className="btn-ghost !p-0.5 text-theme-text-muted"
            title="Cancel"
          >
            <span className="text-[10px]">No</span>
          </button>
        </div>
      ) : (
        <button
          onClick={() => setConfirmDelete(true)}
          disabled={disabled}
          className="btn-ghost !p-1 text-theme-text-muted hover:text-theme-error"
          title="Delete rule"
        >
          <Trash2 className="w-3.5 h-3.5" />
        </button>
      )}
    </div>
  );
}
