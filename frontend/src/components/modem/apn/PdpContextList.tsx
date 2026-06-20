/**
 * PdpContextList — Item #42 Phase 3
 *
 * Read-only list of the modem's PDP contexts. Always visible (no disclosure).
 * Reads from the shared usePdpDetails() query so it stays in sync with the
 * ApnEditor and MBN section without duplicating the query config or issuing
 * independent AT calls.
 *
 * Each row shows: CID, PDP type, APN, and an active/inactive badge derived
 * from pdp_contexts[].active.
 *
 * Design rules:
 *   - OKLCH design tokens only (no hex)
 *   - No emojis
 *   - Active state conveyed by icon + label + color (not color alone)
 *   - Sensible empty state
 */

import { Network, Check, Circle, Loader2 } from 'lucide-react';
import { usePdpDetails } from '@/hooks/queries/usePdpDetails';

export function PdpContextList() {
  const { data: pdp, isFetching } = usePdpDetails();
  const contexts = pdp?.pdp_contexts ?? [];

  return (
    <div className="space-y-2">
      {/* Section header */}
      <div className="flex items-center gap-1.5">
        <Network className="w-3.5 h-3.5 text-theme-text-secondary" aria-hidden="true" />
        <h3 className="text-xs font-semibold text-theme-text-secondary uppercase tracking-wide">
          PDP Contexts
        </h3>
      </div>

      {contexts.length > 0 ? (
        <ul className="space-y-1.5 list-none" aria-label="PDP contexts">
          {contexts.map((ctx, i) => {
            const isActive = ctx.active;
            return (
              <li
                key={`${ctx.cid}-${i}`}
                className="bg-theme-bg-primary rounded-lg p-2 text-xs grid grid-cols-[auto_1fr_auto] gap-2 items-center"
              >
                <span className="font-medium text-theme-text-primary whitespace-nowrap">
                  CID {ctx.cid}
                </span>
                <span className="min-w-0 flex items-baseline gap-2">
                  <span className="text-theme-text-secondary font-mono truncate">
                    {ctx.apn || '--'}
                  </span>
                  <span className="text-theme-text-muted shrink-0">
                    {ctx.pdp_type || '--'}
                  </span>
                </span>
                <span
                  className={`shrink-0 inline-flex items-center gap-1 px-1.5 py-0.5 rounded text-[10px] font-medium ${
                    isActive
                      ? 'bg-theme-success/20 text-theme-success'
                      : 'bg-theme-bg-tertiary text-theme-text-muted'
                  }`}
                >
                  {isActive ? (
                    <Check className="w-3 h-3" aria-hidden="true" />
                  ) : (
                    <Circle className="w-2.5 h-2.5" aria-hidden="true" />
                  )}
                  {isActive ? 'Active' : 'Inactive'}
                </span>
              </li>
            );
          })}
        </ul>
      ) : isFetching ? (
        <div className="flex items-center justify-center gap-2 text-xs text-theme-text-muted py-3">
          <Loader2 className="w-3.5 h-3.5 animate-spin" aria-hidden="true" />
          Loading PDP contexts...
        </div>
      ) : (
        <p className="text-xs text-theme-text-muted text-center py-3">
          No PDP contexts reported.
        </p>
      )}
    </div>
  );
}
