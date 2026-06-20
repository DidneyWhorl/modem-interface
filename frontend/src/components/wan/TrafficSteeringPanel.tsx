/**
 * TrafficSteeringPanel
 *
 * Collapsible panel showing traffic steering rules with drag-to-reorder.
 * Mounts inside WanManagerPanel between Failover Lock and Watchdog Settings.
 */

import { useState, useCallback, useMemo } from 'react';
import { ChevronDown, ChevronRight, Plus } from 'lucide-react';
import { WidthProvider } from 'react-grid-layout';
import RGL from 'react-grid-layout';
import { useSteeringRules } from '@/hooks/queries/useSteeringRules';
import {
  useDeleteSteeringRule,
  useUpdateSteeringRule,
  useReorderSteeringRules,
} from '@/hooks/mutations/useSteeringRules';
import type { SteeringRule } from '@/types/steering';
import SteeringRuleCard from './SteeringRuleCard';

import 'react-grid-layout/css/styles.css';

const ReactGridLayout = WidthProvider(RGL);

interface TrafficSteeringPanelProps {
  onAddRule: () => void;
  onEditRule: (rule: SteeringRule) => void;
}

export default function TrafficSteeringPanel({
  onAddRule,
  onEditRule,
}: TrafficSteeringPanelProps) {
  const [expanded, setExpanded] = useState(false);

  const { data } = useSteeringRules();
  const deleteMut = useDeleteSteeringRule();
  const updateMut = useUpdateSteeringRule();
  const reorderMut = useReorderSteeringRules();

  const rules = useMemo(() => data?.rules ?? [], [data?.rules]);
  const anyMutating = deleteMut.isPending || updateMut.isPending || reorderMut.isPending;

  const handleToggleEnabled = useCallback(
    (id: string, enabled: boolean) => {
      updateMut.mutate({ id, req: { enabled } });
    },
    [updateMut],
  );

  const handleDelete = useCallback(
    (id: string) => {
      deleteMut.mutate(id);
    },
    [deleteMut],
  );

  const handleLayoutChange = useCallback(
    (layout: { i: string; y: number }[]) => {
      const sorted = [...layout].sort((a, b) => a.y - b.y);
      const newOrder = sorted.map((item) => item.i);

      // Only reorder if the order actually changed
      const currentOrder = rules.map((r) => r.id);
      const changed = newOrder.some((id, idx) => id !== currentOrder[idx]);
      if (changed && newOrder.length === rules.length) {
        reorderMut.mutate({ order: newOrder });
      }
    },
    [rules, reorderMut],
  );

  return (
    <div className="border-t border-theme-border">
      <button
        onClick={() => setExpanded(!expanded)}
        className="btn-ghost !w-full !px-0 !py-2 !text-xs font-medium flex items-center gap-1"
      >
        {expanded ? (
          <ChevronDown className="w-3.5 h-3.5" />
        ) : (
          <ChevronRight className="w-3.5 h-3.5" />
        )}
        Traffic Steering
        {rules.length > 0 && (
          <span className="ml-1 px-1.5 py-0.5 text-[10px] rounded-full bg-theme-bg-tertiary text-theme-text-secondary font-normal">
            {rules.length}
          </span>
        )}
      </button>

      {expanded && (
        <div className="space-y-2 pb-2">
          {rules.length === 0 ? (
            <div className="text-center py-4">
              <p className="text-xs text-theme-text-muted">
                No steering rules configured. All traffic follows the default WAN priority order.
              </p>
              <button
                onClick={onAddRule}
                className="btn-secondary !px-3 !py-1.5 !text-xs mt-2 inline-flex items-center gap-1"
              >
                <Plus className="w-3.5 h-3.5" />
                Add Rule
              </button>
            </div>
          ) : (
            <>
              <ReactGridLayout
                className="steering-rule-grid"
                layout={rules.map((rule, idx) => ({
                  i: rule.id,
                  x: 0,
                  y: idx,
                  w: 1,
                  h: 1,
                  isDraggable: !anyMutating,
                  isResizable: false,
                }))}
                cols={1}
                rowHeight={44}
                margin={[0, 4]}
                containerPadding={[0, 0]}
                compactType="vertical"
                isResizable={false}
                draggableHandle=".steering-drag-handle"
                onLayoutChange={handleLayoutChange}
              >
                {rules.map((rule) => (
                  <div key={rule.id}>
                    <SteeringRuleCard
                      rule={rule}
                      onEdit={onEditRule}
                      onDelete={handleDelete}
                      onToggleEnabled={handleToggleEnabled}
                      disabled={anyMutating}
                    />
                  </div>
                ))}
              </ReactGridLayout>

              <div className="flex justify-end">
                <button
                  onClick={onAddRule}
                  disabled={anyMutating}
                  className="btn-secondary !px-2 !py-1 !text-xs inline-flex items-center gap-1"
                >
                  <Plus className="w-3 h-3" />
                  Add Rule
                </button>
              </div>
            </>
          )}
        </div>
      )}
    </div>
  );
}
