/**
 * WAN Manager Panel
 *
 * Multi-modem WAN priority management — modem ordering, failover,
 * watchdog configuration, and failover history.
 *
 * All user changes are buffered in a local draft. Nothing hits the
 * router until the user clicks "Save & Apply" and confirms.
 */

import { useState, useCallback, useEffect, useMemo } from 'react';
import { useQueryClient } from '@tanstack/react-query';
import { useWanStatus, useWatchdogLog, wanStatusQueryKey } from '@/hooks/queries/useWanStatus';
import {
  useApplyWanConfig,
  useScanWanModems,
  useClearWatchdogLog,
  useFailbackNow,
  useAcceptFailover,
  useAddEthernetPort,
} from '@/hooks/mutations/useWanConfig';
import { downloadWatchdogLog, clearRestartSuspensions } from '@/api/wan';
import type { WanModemStatusEntry, WanModemState, WatchdogConfig, WanConfig, FailoverOverrideInfo, AvailableEthernetPort, WanEntryType, RoutingMode } from '@/types/api';
import {
  Network, RefreshCw, Search, ChevronDown, ChevronRight,
  GripVertical, Power, PowerOff, Lock, Unlock,
  Loader2, AlertTriangle, Save, Check, X, Download, Trash2, RotateCcw,
  Globe, Plus, Settings, Info,
} from 'lucide-react';
import clsx from 'clsx';
import { WidthProvider, Layout } from 'react-grid-layout';
import RGL from 'react-grid-layout';

import 'react-grid-layout/css/styles.css';

import TrafficSteeringPanel from '@/components/wan/TrafficSteeringPanel';
import SteeringRuleModal from '@/components/wan/SteeringRuleModal';
import type { SteeringRule } from '@/types/steering';

const ReactGridLayout = WidthProvider(RGL);

// ============================================================================
// Draft helpers
// ============================================================================

/** Convert server status entries to a WanConfig-shaped draft. */
function serverToDraft(
  enabled: boolean,
  modems: WanModemStatusEntry[],
  watchdog: WatchdogConfig,
  failoverLocked: boolean,
  failbackTimerMins: number,
  routingMode: RoutingMode,
): WanConfig {
  return {
    enabled,
    modem_priority: modems.map(m => ({
      modem_id: m.modem_id,
      label: m.label,
      interface_name: m.interface_name,
      network_device: m.network_device,
      state: m.state,
      metric: m.metric,
      entry_type: m.entry_type ?? 'modem' as WanEntryType,
      original_bridge: m.original_bridge ?? null,
      mtu: m.mtu ?? null,
      ttl: m.ttl ?? null,
      hop_limit: m.hop_limit ?? null,
      weight: m.weight ?? null,
      proto_override: m.proto_override ?? null,
    })),
    watchdog: { ...watchdog },
    failover_locked: failoverLocked,
    failback_timer_mins: failbackTimerMins,
    routing_mode: routingMode,
  };
}

/** Deep-compare two WanConfig objects to detect unsaved changes. */
function configsEqual(a: WanConfig, b: WanConfig): boolean {
  if (a.enabled !== b.enabled) return false;
  if (a.failover_locked !== b.failover_locked) return false;
  if (a.routing_mode !== b.routing_mode) return false;
  if (a.modem_priority.length !== b.modem_priority.length) return false;
  for (let i = 0; i < a.modem_priority.length; i++) {
    const am = a.modem_priority[i]!;
    const bm = b.modem_priority[i]!;
    if (
      am.modem_id !== bm.modem_id ||
      am.state !== bm.state ||
      am.label !== bm.label ||
      am.interface_name !== bm.interface_name ||
      am.network_device !== bm.network_device ||
      am.mtu !== bm.mtu ||
      am.ttl !== bm.ttl ||
      am.hop_limit !== bm.hop_limit ||
      am.weight !== bm.weight ||
      am.proto_override !== bm.proto_override ||
      am.metric !== bm.metric ||
      am.entry_type !== bm.entry_type ||
      am.original_bridge !== bm.original_bridge
    ) return false;
  }
  const aw = a.watchdog;
  const bw = b.watchdog;
  if (
    aw.enabled !== bw.enabled ||
    aw.check_interval_secs !== bw.check_interval_secs ||
    aw.failure_threshold !== bw.failure_threshold ||
    aw.ping_target !== bw.ping_target ||
    aw.dns_target !== bw.dns_target ||
    aw.http_target !== bw.http_target ||
    aw.log_retention_days !== bw.log_retention_days ||
    aw.restart_on_failure !== bw.restart_on_failure ||
    aw.restart_cooldown_mins !== bw.restart_cooldown_mins ||
    aw.max_restart_attempts !== bw.max_restart_attempts
  ) return false;
  if (a.failback_timer_mins !== b.failback_timer_mins) return false;
  return true;
}

// ============================================================================
// Sub-components
// ============================================================================

function StatusDot({ status }: { status: WanModemStatusEntry['status'] }) {
  const colors: Record<string, string> = {
    online: 'bg-theme-success',
    offline: 'bg-theme-error',
    checking: 'bg-theme-warning animate-pulse',
    standby: 'bg-theme-warning',
    no_sim: 'bg-theme-warning',
  };
  const labels: Record<string, string> = {
    online: 'Online',
    offline: 'Offline',
    checking: 'Checking',
    standby: 'Standby',
    no_sim: 'No SIM',
  };
  return (
    <span
      className={clsx('inline-block w-2.5 h-2.5 rounded-full', colors[status] ?? 'bg-theme-text-muted')}
      title={labels[status] ?? status}
    />
  );
}

function HealthIcon({ ok }: { ok: boolean | null }) {
  if (ok === null) return <span className="text-theme-text-muted text-xs">-</span>;
  return ok
    ? <Check className="w-3.5 h-3.5 text-theme-success" />
    : <X className="w-3.5 h-3.5 text-theme-error" />;
}

// ============================================================================
// Failover Banner
// ============================================================================

interface FailoverBannerProps {
  override: FailoverOverrideInfo;
  failbackTimerMins: number;
  onFailback: () => void;
  onAccept: () => void;
  failbackPending: boolean;
  acceptPending: boolean;
}

function FailoverBanner({
  override: fo, failbackTimerMins,
  onFailback, onAccept,
  failbackPending, acceptPending,
}: FailoverBannerProps) {
  const remaining = fo.stabilization_remaining_secs;

  let subtitleText: string;
  if (failbackTimerMins === 0) {
    subtitleText = 'Manual failback required.';
  } else if (remaining != null && remaining > 0) {
    const mins = Math.floor(remaining / 60);
    const secs = remaining % 60;
    subtitleText = `Auto-failback in: ${mins}m ${secs}s remaining`;
  } else {
    subtitleText = `Waiting for ${fo.original_primary_label} to recover...`;
  }

  return (
    <div className="rounded-lg border border-theme-warning/30 bg-theme-warning/10 p-3 space-y-2">
      <div className="flex items-start gap-2">
        <AlertTriangle className="w-4 h-4 text-theme-warning flex-shrink-0 mt-0.5" />
        <div className="flex-1 min-w-0">
          <p className="text-sm font-medium text-theme-text-primary">
            Failover active — {fo.current_primary_label} is handling internet traffic
          </p>
          <p className="text-xs text-theme-text-secondary mt-0.5">
            {subtitleText}
          </p>
        </div>
      </div>
      <div className="flex gap-2 ml-6">
        <button
          onClick={onFailback}
          disabled={failbackPending || acceptPending}
          className="btn-primary !px-2.5 !py-1 !text-xs flex items-center gap-1"
        >
          {failbackPending
            ? <Loader2 className="w-3 h-3 animate-spin" />
            : <RotateCcw className="w-3 h-3" />
          }
          Failback Now
        </button>
        <button
          onClick={onAccept}
          disabled={failbackPending || acceptPending}
          className="btn-secondary !px-2.5 !py-1 !text-xs flex items-center gap-1"
        >
          {acceptPending
            ? <Loader2 className="w-3 h-3 animate-spin" />
            : <Check className="w-3 h-3" />
          }
          Keep Current
        </button>
      </div>
    </div>
  );
}

// ============================================================================
// Main Panel
// ============================================================================

export function WanManagerPanel() {
  const { data: wanStatus, isLoading, error, refetch, isFetching } = useWanStatus();

  const applyConfigMut = useApplyWanConfig();
  const scanMut = useScanWanModems();
  const clearLogMut = useClearWatchdogLog();
  const failbackMut = useFailbackNow();
  const acceptFailoverMut = useAcceptFailover();
  const addEthernetMut = useAddEthernetPort();

  // Draft state — local copy of server config that all interactions modify
  const [draft, setDraft] = useState<WanConfig | null>(null);
  const [showConfirmApply, setShowConfirmApply] = useState(false);

  // Available Ethernet ports from last scan
  const [availableEthPorts, setAvailableEthPorts] = useState<AvailableEthernetPort[]>([]);

  // UI state
  const [showWatchdog, setShowWatchdog] = useState(false);
  const [showHistory, setShowHistory] = useState(false);
  const [showRecoveryLog, setShowRecoveryLog] = useState(false);
  // Steering modal state: undefined=closed, null=add, SteeringRule=edit (modal added in Task 9)
  const [steeringModalRule, setSteeringModalRule] = useState<SteeringRule | null | undefined>(undefined);
  const [confirmClearLog, setConfirmClearLog] = useState(false);
  // Track which modem card has its remove confirmation open (for grid row height)
  const [removeConfirmId, setRemoveConfirmId] = useState<string | null>(null);
  const [settingsOpenId, setSettingsOpenId] = useState<string | null>(null);

  // Fetch watchdog log only when the recovery log section is visible
  const { data: watchdogLog } = useWatchdogLog(showRecoveryLog);

  // Derive the "server config" from the latest wanStatus for comparison
  const serverConfig = useMemo(() => {
    if (!wanStatus) return null;
    return serverToDraft(
      wanStatus.enabled,
      wanStatus.modems,
      wanStatus.watchdog,
      wanStatus.failover_locked,
      wanStatus.failback_timer_mins,
      wanStatus.routing_mode,
    );
  }, [wanStatus]);

  // Initialize draft from server state on first load or after successful apply
  useEffect(() => {
    if (serverConfig && !draft) {
      setDraft(serverConfig);
    }
  }, [serverConfig, draft]);

  // After a successful apply, reset draft to new server state
  useEffect(() => {
    if (applyConfigMut.isSuccess && serverConfig) {
      setDraft(serverConfig);
      applyConfigMut.reset();
    }
  }, [applyConfigMut.isSuccess, serverConfig, applyConfigMut]);

  const hasUnsavedChanges = useMemo(() => {
    if (!draft || !serverConfig) return false;
    return !configsEqual(draft, serverConfig);
  }, [draft, serverConfig]);

  const totalActiveWeight = useMemo(() => {
    if (!draft || draft.routing_mode !== 'load_balance') return 0;
    return draft.modem_priority
      .filter(m => m.state === 'active')
      .reduce((sum, m) => sum + (m.weight ?? 1), 0);
  }, [draft]);

  // Draft modification helpers
  const updateDraft = useCallback((updater: (prev: WanConfig) => WanConfig) => {
    setDraft(prev => prev ? updater(prev) : prev);
  }, []);

  const handleToggleEnabled = useCallback(() => {
    updateDraft(d => ({ ...d, enabled: !d.enabled }));
  }, [updateDraft]);

  const handleSetRoutingMode = useCallback((mode: RoutingMode) => {
    updateDraft(d => ({ ...d, routing_mode: mode }));
  }, [updateDraft]);

  const handleToggleFailoverLock = useCallback(() => {
    updateDraft(d => ({ ...d, failover_locked: !d.failover_locked }));
  }, [updateDraft]);

  const handleSetModemState = useCallback((modemId: string, newState: WanModemState) => {
    updateDraft(d => ({
      ...d,
      modem_priority: d.modem_priority.map(m =>
        m.modem_id === modemId ? { ...m, state: newState } : m
      ),
    }));
  }, [updateDraft]);

  const handleRemoveModem = useCallback((modemId: string) => {
    updateDraft(d => ({
      ...d,
      modem_priority: d.modem_priority.filter(m => m.modem_id !== modemId),
    }));
  }, [updateDraft]);

  const handleMakePrimary = useCallback((modemId: string) => {
    updateDraft(d => {
      const modems = [...d.modem_priority];
      const idx = modems.findIndex(m => m.modem_id === modemId);
      if (idx <= 0) return d;
      const [moved] = modems.splice(idx, 1);
      modems.unshift(moved!);
      return { ...d, modem_priority: modems };
    });
  }, [updateDraft]);

  const handleUpdateMtu = useCallback((modemId: string, value: number | null) => {
    setDraft(d => d ? {
      ...d,
      modem_priority: d.modem_priority.map(m =>
        m.modem_id === modemId ? { ...m, mtu: value } : m
      ),
    } : d);
  }, []);

  const handleUpdateTtl = useCallback((modemId: string, value: number | null) => {
    setDraft(d => d ? {
      ...d,
      modem_priority: d.modem_priority.map(m =>
        m.modem_id === modemId ? { ...m, ttl: value } : m
      ),
    } : d);
  }, []);

  const handleUpdateHopLimit = useCallback((modemId: string, value: number | null) => {
    setDraft(d => d ? {
      ...d,
      modem_priority: d.modem_priority.map(m =>
        m.modem_id === modemId ? { ...m, hop_limit: value } : m
      ),
    } : d);
  }, []);

  const handleUpdateProtoOverride = useCallback((modemId: string, value: string | null) => {
    setDraft(d => d ? {
      ...d,
      modem_priority: d.modem_priority.map(m =>
        m.modem_id === modemId ? { ...m, proto_override: value } : m
      ),
    } : d);
  }, []);

  const handleUpdateWeight = useCallback((modemId: string, weight: number | null) => {
    updateDraft(d => ({
      ...d,
      modem_priority: d.modem_priority.map(m =>
        m.modem_id === modemId ? { ...m, weight } : m
      ),
    }));
  }, [updateDraft]);

  const handleUpdateWatchdog = useCallback((watchdog: WatchdogConfig) => {
    updateDraft(d => ({ ...d, watchdog }));
  }, [updateDraft]);

  const handleDiscard = useCallback(() => {
    if (serverConfig) {
      setDraft(serverConfig);
    }
  }, [serverConfig]);

  const handleApply = useCallback(() => {
    if (!draft) return;
    applyConfigMut.mutate(draft);
    setShowConfirmApply(false);
  }, [draft, applyConfigMut]);

  // react-grid-layout: handle reorder when layout changes via drag
  const handleGridLayoutChange = useCallback((newLayout: Layout[]) => {
    if (!draft) return;
    // Sort items by their Y position to determine new order
    const sorted = [...newLayout].sort((a, b) => a.y - b.y);
    const newOrder = sorted
      .map(item => draft.modem_priority.find(m => m.modem_id === item.i))
      .filter((m): m is NonNullable<typeof m> => m != null);
    if (newOrder.length !== draft.modem_priority.length) return;
    // Only update if order actually changed
    const orderChanged = newOrder.some((m, i) => m.modem_id !== draft.modem_priority[i]?.modem_id);
    if (orderChanged) {
      updateDraft(d => ({ ...d, modem_priority: newOrder }));
    }
  }, [draft, updateDraft]);

  // Reset draft after failback/accept-failover succeeds (server state changed)
  useEffect(() => {
    if (failbackMut.isSuccess && serverConfig) {
      setDraft(serverConfig);
      failbackMut.reset();
    }
  }, [failbackMut.isSuccess, serverConfig, failbackMut]);

  useEffect(() => {
    if (acceptFailoverMut.isSuccess && serverConfig) {
      setDraft(serverConfig);
      acceptFailoverMut.reset();
    }
  }, [acceptFailoverMut.isSuccess, serverConfig, acceptFailoverMut]);

  // Loading state
  if (isLoading) {
    return (
      <div className="loading-state">
        <Loader2 className="loading-spinner" />
        Loading WAN status...
      </div>
    );
  }

  // Error state
  if (error) {
    return (
      <div className="error-state">
        <AlertTriangle className="w-5 h-5 text-theme-error" />
        <span className="text-theme-error">Failed to load WAN status</span>
        <button onClick={() => refetch()} className="btn-ghost !text-xs">Retry</button>
      </div>
    );
  }

  if (!wanStatus || !draft) return null;

  // Merge draft config with server runtime status for display
  const draftModems: WanModemStatusEntry[] = draft.modem_priority.map((entry) => {
    // Find the server runtime info for this modem
    const serverModem = wanStatus.modems.find(m => m.modem_id === entry.modem_id);
    const isFirstActive = draft.modem_priority.find(m => m.state === 'active')?.modem_id === entry.modem_id;
    return {
      modem_id: entry.modem_id,
      label: entry.label,
      interface_name: entry.interface_name,
      network_device: entry.network_device,
      state: entry.state,
      metric: entry.metric,
      status: serverModem?.status ?? (entry.state === 'active' ? 'offline' : 'standby'),
      last_check: serverModem?.last_check ?? null,
      consecutive_failures: serverModem?.consecutive_failures ?? 0,
      is_primary: isFirstActive,
      entry_type: entry.entry_type ?? 'modem',
      original_bridge: entry.original_bridge ?? null,
      mtu: entry.mtu,
      ttl: entry.ttl,
      hop_limit: entry.hop_limit,
      operator: serverModem?.operator ?? null,
      imei: serverModem?.imei ?? null,
      restart_suspended: serverModem?.restart_suspended ?? false,
      restart_count: serverModem?.restart_count ?? 0,
      weight: entry.weight ?? null,
      proto_override: entry.proto_override ?? null,
    };
  });

  const anyMutating = applyConfigMut.isPending || scanMut.isPending || failbackMut.isPending || acceptFailoverMut.isPending || addEthernetMut.isPending;
  const panelDisabled = anyMutating || !draft.enabled;

  return (
    <div className="space-y-3 p-3 text-sm">
      {/* Header controls */}
      <div className="flex items-center gap-2 flex-wrap">
        {/* Enable toggle */}
        <label className="flex items-center gap-2 cursor-pointer select-none">
          <input
            type="checkbox"
            checked={draft.enabled}
            onChange={handleToggleEnabled}
            disabled={anyMutating}
            className="accent-theme-accent focus-visible:ring-2 focus-visible:ring-theme-accent focus-visible:ring-offset-1"
          />
          <span className="font-medium text-theme-text-primary">Enable CTRL-WAN</span>
        </label>

        {wanStatus?.platform && !wanStatus.platform.policy_routing_enabled && (
          <span
            className="inline-flex items-center gap-1 px-2 py-0.5 rounded text-xs font-medium"
            style={{ backgroundColor: 'oklch(0.8 0.12 70)', color: 'oklch(0.3 0.08 70)' }}
            title={
              wanStatus.platform.mwan3_detected
                ? 'mwan3 detected — disable mwan3 to enable policy-based routing'
                : 'iproute2 unavailable — using metric-based routing'
            }
          >
            Metric Routing Only
          </span>
        )}

        {draft.enabled && (
          <div className="flex rounded overflow-hidden border border-theme-border text-[10px]">
            <button
              onClick={() => handleSetRoutingMode('failover')}
              disabled={applyConfigMut.isPending}
              className={clsx(
                'px-2 py-0.5 flex items-center gap-1 transition-colors border-b-2 outline-none focus-visible:ring-2 focus-visible:ring-theme-accent',
                draft.routing_mode === 'failover'
                  ? 'bg-theme-accent/20 text-theme-accent font-medium border-theme-accent'
                  : 'bg-theme-bg-tertiary text-theme-text-muted hover:bg-theme-bg-secondary border-transparent',
              )}
            >
              Failover
            </button>
            <button
              onClick={() => handleSetRoutingMode('load_balance')}
              disabled={applyConfigMut.isPending}
              className={clsx(
                'px-2 py-0.5 flex items-center gap-1 transition-colors border-b-2 outline-none focus-visible:ring-2 focus-visible:ring-theme-accent',
                draft.routing_mode === 'load_balance'
                  ? 'bg-theme-accent/20 text-theme-accent font-medium border-theme-accent'
                  : 'bg-theme-bg-tertiary text-theme-text-muted hover:bg-theme-bg-secondary border-transparent',
              )}
            >
              Load Balance
            </button>
          </div>
        )}

        {draft.routing_mode === 'load_balance' && (
          <div className="relative group">
            <Info className="w-3.5 h-3.5 text-theme-text-muted cursor-help" />
            <div className="absolute left-1/2 -translate-x-1/2 top-full mt-1.5 w-56 p-2.5 rounded-lg bg-theme-bg-secondary border border-theme-border text-[10px] text-theme-text-secondary leading-relaxed opacity-0 pointer-events-none group-hover:opacity-100 group-hover:pointer-events-auto transition-opacity z-50 shadow-lg">
              <p className="font-medium text-theme-text-primary mb-1">Load Balance Weights</p>
              <p>Traffic is distributed across active WANs proportionally by weight. Higher weight = more traffic.</p>
              <p className="mt-1">Example: Weight 3 and 2 = 60% / 40% split. Standby WANs carry 0%.</p>
            </div>
          </div>
        )}

        <div className="flex-1" />

        {/* Unsaved changes indicator + Save/Discard buttons */}
        <div className="flex items-center gap-1">
          {hasUnsavedChanges && (
            <>
              <span className="text-[10px] text-theme-warning font-medium">Unsaved changes</span>
              <button
                onClick={handleDiscard}
                disabled={anyMutating}
                className="btn-secondary !px-2 !py-1 !text-xs flex items-center gap-1"
                title="Discard changes"
              >
                <RotateCcw className="w-3 h-3" />
                Discard
              </button>
            </>
          )}
          <button
            onClick={() => setShowConfirmApply(true)}
            disabled={!hasUnsavedChanges || anyMutating}
            className="btn-primary !px-2 !py-1 !text-xs flex items-center gap-1"
            title="Save and apply changes"
          >
            {applyConfigMut.isPending
              ? <Loader2 className="w-3 h-3 animate-spin" />
              : <Save className="w-3 h-3" />
            }
            Save &amp; Apply
          </button>
        </div>

        {/* Scan button */}
        <button
          onClick={() => scanMut.mutate(undefined, {
            onSuccess: (data) => {
              // After scan, reset draft to include new modems
              setDraft(serverToDraft(
                data.enabled,
                data.modems,
                data.watchdog,
                data.failover_locked,
                data.failback_timer_mins,
                data.routing_mode,
              ));
              // Store available Ethernet ports from scan response
              setAvailableEthPorts(data.available_ethernet_ports ?? []);
            },
          })}
          disabled={anyMutating}
          className="btn-secondary !px-2 !py-1 !text-xs flex items-center gap-1"
          title="Scan for modems"
        >
          {scanMut.isPending
            ? <Loader2 className="w-3.5 h-3.5 animate-spin" />
            : <Search className="w-3.5 h-3.5" />
          }
          Scan
        </button>

        {/* Refresh */}
        <button
          onClick={() => refetch()}
          disabled={isFetching}
          className="btn-icon"
          title="Refresh status"
        >
          <RefreshCw className={clsx('w-3.5 h-3.5', isFetching && 'animate-spin')} />
        </button>
      </div>

      {/* Save & Apply confirmation dialog */}
      {showConfirmApply && (
        <div className="rounded border border-theme-warning/30 bg-theme-warning/10 p-3">
          <p className="text-xs mb-2 text-theme-text-primary">
            Apply these changes? This will reconfigure the router's network interfaces and may briefly interrupt connectivity.
          </p>
          <div className="flex gap-2">
            <button
              onClick={handleApply}
              disabled={applyConfigMut.isPending}
              className="btn-primary !px-3 !py-1 !text-xs"
            >
              {applyConfigMut.isPending ? 'Applying...' : 'Confirm'}
            </button>
            <button
              onClick={() => setShowConfirmApply(false)}
              className="btn-secondary !px-3 !py-1 !text-xs"
            >
              Cancel
            </button>
          </div>
        </div>
      )}

      {/* Failover banner */}
      {wanStatus.failover_override && wanStatus.failover_override.active && (
        <FailoverBanner
          override={wanStatus.failover_override}
          failbackTimerMins={wanStatus.failback_timer_mins}
          onFailback={() => failbackMut.mutate()}
          onAccept={() => acceptFailoverMut.mutate()}
          failbackPending={failbackMut.isPending}
          acceptPending={acceptFailoverMut.isPending}
        />
      )}

      {/* Empty state */}
      {draftModems.length === 0 && (
        <div className="empty-state">
          <Network className="w-8 h-8 opacity-50" />
          <p>No modems detected in WAN priority list.</p>
          <p className="text-xs mt-1">Click "Scan" to detect connected modems.</p>
        </div>
      )}

      {/* Modem list — react-grid-layout for drag reorder */}
      {draftModems.length > 0 && (
        <ReactGridLayout
          className="wan-modem-grid"
          layout={draftModems.map((modem, idx) => ({
            i: modem.modem_id,
            x: 0,
            y: idx,
            w: 1,
            h: removeConfirmId === modem.modem_id ? 2 : settingsOpenId === modem.modem_id ? 2 : 1,
            isDraggable: modem.state === 'active' && !panelDisabled,
            isResizable: false,
          }))}
          cols={1}
          rowHeight={160}
          margin={[0, 8]}
          containerPadding={[0, 0]}
          compactType="vertical"
          isResizable={false}
          draggableHandle=".wan-drag-handle"
          onLayoutChange={handleGridLayoutChange}
        >
          {draftModems.map((modem, idx) => (
            <div key={modem.modem_id}>
              <ModemCard
                modem={modem}
                index={idx}
                activeCount={draftModems.filter(m => m.state === 'active').length}
                disabled={panelDisabled}
                showRemoveConfirm={removeConfirmId === modem.modem_id}
                onShowRemoveConfirm={(show) => { setRemoveConfirmId(show ? modem.modem_id : null); setSettingsOpenId(null); }}
                onMakePrimary={(id) => handleMakePrimary(id)}
                onSetState={(id, s) => handleSetModemState(id, s)}
                onRemove={(id) => handleRemoveModem(id)}
                showSettings={settingsOpenId === modem.modem_id}
                onToggleSettings={() => {
                  setSettingsOpenId(prev => prev === modem.modem_id ? null : modem.modem_id);
                  setRemoveConfirmId(null);
                }}
                onUpdateMtu={(v: number | null) => handleUpdateMtu(modem.modem_id, v)}
                onUpdateTtl={(v: number | null) => handleUpdateTtl(modem.modem_id, v)}
                onUpdateHopLimit={(v: number | null) => handleUpdateHopLimit(modem.modem_id, v)}
                onUpdateProtoOverride={(v: string | null) => handleUpdateProtoOverride(modem.modem_id, v)}
                routingMode={draft.routing_mode}
                onUpdateWeight={(val) => handleUpdateWeight(modem.modem_id, val)}
                totalActiveWeight={totalActiveWeight}
              />
            </div>
          ))}
        </ReactGridLayout>
      )}

      {draft.routing_mode === 'load_balance' && (() => {
        const activeModems = draft.modem_priority.filter(m => m.state === 'active');
        if (activeModems.length < 2) return null;
        const totalWeight = activeModems.reduce((sum, m) => sum + (m.weight ?? 1), 0);
        return (
          <div className="flex items-center gap-2 px-3 py-2 rounded-lg border border-theme-success/20 bg-theme-success/5 text-caption text-theme-text-secondary mt-2">
            <span className="text-theme-text-muted">Traffic split:</span>
            {activeModems.map((m, i) => {
              const pct = Math.round(((m.weight ?? 1) / totalWeight) * 100);
              const statusEntry = wanStatus?.modems.find(s => s.modem_id === m.modem_id);
              const label = statusEntry?.label ?? m.modem_id;
              return (
                <span key={m.modem_id}>
                  {i > 0 && <span className="text-theme-text-muted mx-1">|</span>}
                  {label} <span className="text-theme-success font-semibold">{pct}%</span>
                </span>
              );
            })}
            <span className="text-theme-text-muted ml-1">
              (weight {activeModems.map(m => m.weight ?? 1).join(':')})
            </span>
          </div>
        );
      })()}

      {/* Available Ethernet Ports (from scan) */}
      {availableEthPorts.length > 0 && (
        <AvailableEthernetSection
          ports={availableEthPorts}
          disabled={panelDisabled}
          onAdd={(portName) => {
            addEthernetMut.mutate({ port_name: portName }, {
              onSuccess: () => {
                // Remove the added port from available list
                setAvailableEthPorts(prev => prev.filter(p => p.port_name !== portName));
                // Refetch status to get the updated priority list
                refetch();
              },
            });
          }}
          addingPort={addEthernetMut.isPending}
        />
      )}

      {/* Failover Lock */}
      <div className="flex items-center gap-2 py-1 border-t border-theme-border">
        <label className="flex items-center gap-2 cursor-pointer select-none flex-1">
          <input
            type="checkbox"
            checked={draft.failover_locked}
            onChange={handleToggleFailoverLock}
            disabled={anyMutating}
            className="accent-theme-accent focus-visible:ring-2 focus-visible:ring-theme-accent focus-visible:ring-offset-1"
          />
          {draft.failover_locked
            ? <Lock className="w-3.5 h-3.5 text-theme-warning" />
            : <Unlock className="w-3.5 h-3.5 text-theme-text-muted" />
          }
          <span className="text-xs text-theme-text-primary">Failover Lock</span>
        </label>
        <span className="text-[10px] text-theme-text-muted max-w-[200px]">
          {draft.failover_locked
            ? 'Auto failover disabled — manual control only'
            : 'Automatic failover enabled'
          }
        </span>
      </div>

      {/* Traffic Steering (collapsible) */}
      <TrafficSteeringPanel
        onAddRule={() => setSteeringModalRule(null)}
        onEditRule={(rule) => setSteeringModalRule(rule)}
      />

      {/* Watchdog Settings (collapsible) */}
      <div className="border-t border-theme-border">
        <button
          onClick={() => setShowWatchdog(!showWatchdog)}
          className="btn-ghost !w-full !px-0 !py-2 !text-xs font-medium flex items-center gap-1"
        >
          {showWatchdog ? <ChevronDown className="w-3.5 h-3.5" /> : <ChevronRight className="w-3.5 h-3.5" />}
          Watchdog Settings
        </button>

        {showWatchdog && (
          <WatchdogSettingsForm
            watchdog={draft.watchdog}
            onChange={handleUpdateWatchdog}
            disabled={anyMutating}
            watchdogLog={watchdogLog}
            showRecoveryLog={showRecoveryLog}
            setShowRecoveryLog={setShowRecoveryLog}
            confirmClearLog={confirmClearLog}
            setConfirmClearLog={setConfirmClearLog}
            clearLogMut={clearLogMut}
            failbackTimerMins={draft.failback_timer_mins}
            onFailbackTimerChange={(v) => updateDraft(d => ({ ...d, failback_timer_mins: v }))}
            serverModems={wanStatus.modems}
          />
        )}
      </div>

      {/* Failover History (collapsible) */}
      {wanStatus.failover_history.length > 0 && (
        <div className="border-t border-theme-border">
          <button
            onClick={() => setShowHistory(!showHistory)}
            className="btn-ghost !w-full !px-0 !py-2 !text-xs font-medium flex items-center gap-1"
          >
            {showHistory ? <ChevronDown className="w-3.5 h-3.5" /> : <ChevronRight className="w-3.5 h-3.5" />}
            Failover History ({wanStatus.failover_history.length})
          </button>

          {showHistory && (
            <div className="max-h-48 overflow-y-auto space-y-1 pb-2 pl-4">
              {wanStatus.failover_history.map((event, i) => (
                <div key={i} className="text-[10px] text-theme-text-muted font-mono leading-tight">
                  <span className="text-theme-text-secondary">{new Date(event.timestamp).toLocaleString()}</span>
                  {' '}
                  <span className="text-theme-error">{event.from_label}</span>
                  {' → '}
                  <span className="text-theme-success">{event.to_label}</span>
                  {' '}
                  <span className="text-theme-text-muted">({event.reason})</span>
                </div>
              ))}
            </div>
          )}
        </div>
      )}

      {/* Steering Rule Modal */}
      {steeringModalRule !== undefined && (
        <SteeringRuleModal
          rule={steeringModalRule}
          onClose={() => setSteeringModalRule(undefined)}
          wanModems={wanStatus.modems ?? []}
        />
      )}
    </div>
  );
}

// ============================================================================
// WatchdogSettingsForm sub-component
// ============================================================================

interface WatchdogSettingsFormProps {
  watchdog: WatchdogConfig;
  onChange: (w: WatchdogConfig) => void;
  disabled: boolean;
  watchdogLog: { entries: { timestamp: string; action: string; details: string }[]; last_recovery: { timestamp: string; action: string; details: string } | null } | undefined;
  showRecoveryLog: boolean;
  setShowRecoveryLog: (v: boolean) => void;
  confirmClearLog: boolean;
  setConfirmClearLog: (v: boolean) => void;
  clearLogMut: { mutate: (v: undefined, opts?: { onSuccess?: () => void }) => void; isPending: boolean };
  failbackTimerMins: number;
  onFailbackTimerChange: (v: number) => void;
  serverModems: WanModemStatusEntry[];
}

function WatchdogSettingsForm({
  watchdog, onChange, disabled,
  watchdogLog, showRecoveryLog, setShowRecoveryLog,
  confirmClearLog, setConfirmClearLog, clearLogMut,
  failbackTimerMins, onFailbackTimerChange,
  serverModems,
}: WatchdogSettingsFormProps) {
  const queryClient = useQueryClient();
  const update = (partial: Partial<WatchdogConfig>) => onChange({ ...watchdog, ...partial });

  return (
    <div className="space-y-2 pb-2 pl-4">
      <label className="flex items-center gap-2 text-xs text-theme-text-primary">
        <input
          type="checkbox"
          checked={watchdog.enabled}
          onChange={e => update({ enabled: e.target.checked })}
          className="accent-theme-accent focus-visible:ring-2 focus-visible:ring-theme-accent focus-visible:ring-offset-1"
          disabled={disabled}
        />
        Watchdog enabled
      </label>

      <div className="grid grid-cols-3 gap-2 text-xs">
        <label>
          <span className="text-theme-text-secondary">Check interval (s)</span>
          <input
            type="number" min={5} max={300}
            value={watchdog.check_interval_secs}
            onChange={e => update({ check_interval_secs: +e.target.value })}
            className="input-compact w-full mt-0.5"
            disabled={disabled}
          />
        </label>
        <label>
          <span className="text-theme-text-secondary">Failure threshold</span>
          <input
            type="number" min={1} max={10}
            value={watchdog.failure_threshold}
            onChange={e => update({ failure_threshold: +e.target.value })}
            className="input-compact w-full mt-0.5"
            disabled={disabled}
          />
        </label>
        <label>
          <span className="text-theme-text-secondary">Log retention (days)</span>
          <input
            type="number" min={1} max={30}
            value={watchdog.log_retention_days}
            onChange={e => update({ log_retention_days: +e.target.value })}
            className="input-compact w-full mt-0.5"
            disabled={disabled}
          />
        </label>
      </div>

      <label className="block text-xs">
        <span className="text-theme-text-secondary">Ping target</span>
        <input
          type="text"
          value={watchdog.ping_target}
          onChange={e => update({ ping_target: e.target.value })}
          className="input-compact w-full mt-0.5"
          disabled={disabled}
        />
      </label>

      <label className="block text-xs">
        <span className="text-theme-text-secondary">DNS target</span>
        <input
          type="text"
          value={watchdog.dns_target}
          onChange={e => update({ dns_target: e.target.value })}
          className="input-compact w-full mt-0.5"
          disabled={disabled}
        />
      </label>

      <label className="block text-xs">
        <span className="text-theme-text-secondary">HTTP check target</span>
        <input
          type="text"
          value={watchdog.http_target}
          onChange={e => update({ http_target: e.target.value })}
          className="input-compact w-full mt-0.5"
          disabled={disabled}
        />
      </label>

      {/* Restart failed modems */}
      <div className="border-t border-theme-border mt-2 pt-2">
        <label className="flex items-center gap-2 text-xs text-theme-text-primary">
          <input
            type="checkbox"
            checked={watchdog.restart_on_failure}
            onChange={e => update({ restart_on_failure: e.target.checked })}
            className="accent-theme-accent focus-visible:ring-2 focus-visible:ring-theme-accent focus-visible:ring-offset-1"
            disabled={disabled}
          />
          Restart failed modems
        </label>
        <p className="text-[10px] text-theme-text-muted mt-0.5 ml-5 mb-1">
          Restarts any modem that fails the failure threshold via its own AT port. Each modem has its own cooldown timer.
        </p>
        {watchdog.restart_on_failure && (
          <div className="grid grid-cols-2 gap-3 text-xs mt-1 ml-5" style={{ maxWidth: '70%' }}>
            <label>
              <span className="text-theme-text-secondary">Cooldown after restart (min)</span>
              <input
                type="number" min={1} max={60}
                value={watchdog.restart_cooldown_mins}
                onChange={e => update({ restart_cooldown_mins: +e.target.value })}
                className="input-compact w-full mt-0.5"
                disabled={disabled}
              />
            </label>
            <label>
              <span className="text-theme-text-secondary">Max restart attempts</span>
              <input
                type="number" min={1} max={50}
                value={watchdog.max_restart_attempts}
                onChange={e => update({ max_restart_attempts: +e.target.value })}
                className="input-compact w-full mt-0.5"
                disabled={disabled}
              />
            </label>
          </div>
        )}
        {watchdog.restart_on_failure && serverModems.some(m => m.restart_suspended) && (
          <div className="ml-5 mt-2">
            <button
              type="button"
              onClick={async () => {
                try {
                  await clearRestartSuspensions();
                  queryClient.invalidateQueries({ queryKey: wanStatusQueryKey });
                } catch { /* ignore */ }
              }}
              className="text-xs px-2 py-1 rounded bg-amber-100 text-amber-700 hover:bg-amber-200 dark:bg-amber-900/30 dark:text-amber-400 dark:hover:bg-amber-900/50 transition-colors"
            >
              Clear restart suspensions
            </button>
          </div>
        )}
      </div>

      {/* Failback timer */}
      <div className="border-t border-theme-border mt-2 pt-2">
        <label className="block text-xs">
          <span className="text-theme-text-secondary">Auto-failback timer</span>
          <select
            value={failbackTimerMins}
            onChange={e => onFailbackTimerChange(Number(e.target.value))}
            className="select-compact w-full mt-0.5"
            disabled={disabled}
          >
            <option value={15}>15 minutes</option>
            <option value={30}>30 minutes</option>
            <option value={60}>1 hour</option>
            <option value={360}>6 hours</option>
            <option value={720}>12 hours</option>
            <option value={0}>Never (manual only)</option>
          </select>
        </label>
        <p className="text-[10px] text-theme-text-muted mt-0.5">
          After a failover, how long the original primary must stay healthy before automatic failback.
        </p>
      </div>

      {/* Recovery Log (expandable sub-section) */}
      <div className="border-t border-theme-border mt-2 pt-1">
        <div className="flex items-center gap-1">
          <button
            onClick={() => setShowRecoveryLog(!showRecoveryLog)}
            className="btn-ghost !px-0 !py-1 !text-xs font-medium flex items-center gap-1 flex-1"
          >
            {showRecoveryLog ? <ChevronDown className="w-3.5 h-3.5" /> : <ChevronRight className="w-3.5 h-3.5" />}
            Recovery Log
            {watchdogLog && watchdogLog.entries.length > 0 && (
              <span className="text-[10px] text-theme-text-muted ml-1">({watchdogLog.entries.length})</span>
            )}
          </button>
          {showRecoveryLog && (
            <div className="flex items-center gap-1">
              <button
                onClick={() => downloadWatchdogLog()}
                className="btn-icon !p-1"
                title="Download log"
              >
                <Download className="w-3 h-3" />
              </button>
              <button
                onClick={() => setConfirmClearLog(true)}
                className="btn-icon-danger !p-1"
                title="Clear log"
              >
                <Trash2 className="w-3 h-3" />
              </button>
            </div>
          )}
        </div>

        {/* Last recovery summary */}
        {watchdogLog?.last_recovery ? (
          <div className="text-[10px] text-theme-text-muted mb-1 pl-5">
            Last recovery: <span className="text-theme-warning">{watchdogLog.last_recovery.action}</span>
            {' '}{watchdogLog.last_recovery.details}
            {' — '}
            <span className="text-theme-text-secondary">
              {new Date(watchdogLog.last_recovery.timestamp).toLocaleString()}
            </span>
          </div>
        ) : (
          <div className="text-[10px] text-theme-text-muted mb-1 pl-5">
            No recovery events recorded
          </div>
        )}

        {/* Clear confirmation */}
        {confirmClearLog && (
          <div className="rounded border border-theme-warning/30 bg-theme-warning/10 p-2 mb-1 ml-5">
            <p className="text-[10px] text-theme-text-primary mb-1">Clear all recovery log entries?</p>
            <div className="flex gap-2">
              <button
                onClick={() => {
                  clearLogMut.mutate(undefined, { onSuccess: () => setConfirmClearLog(false) });
                }}
                disabled={clearLogMut.isPending}
                className="btn-danger !px-2 !py-0.5 !text-[10px] min-h-[28px]"
              >
                {clearLogMut.isPending ? 'Clearing...' : 'Clear'}
              </button>
              <button
                onClick={() => setConfirmClearLog(false)}
                className="btn-secondary !px-2 !py-0.5 !text-[10px] min-h-[28px]"
              >
                Cancel
              </button>
            </div>
          </div>
        )}

        {/* Log entries */}
        {showRecoveryLog && watchdogLog && (
          <div className="max-h-48 overflow-y-auto space-y-0.5 pb-1 pl-5">
            {watchdogLog.entries.length === 0 ? (
              <div className="text-[10px] text-theme-text-muted py-2">No entries within retention window</div>
            ) : (
              [...watchdogLog.entries].reverse().map((entry, i) => (
                <div key={i} className="text-[10px] font-mono leading-tight">
                  <span className="text-theme-text-secondary">
                    {new Date(entry.timestamp).toLocaleString()}
                  </span>
                  {' '}
                  <span className={entry.action === 'FAILOVER' ? 'text-theme-warning' : 'text-theme-success'}>
                    {entry.action}
                  </span>
                  {' '}
                  <span className="text-theme-text-muted">{entry.details}</span>
                </div>
              ))
            )}
          </div>
        )}
      </div>
    </div>
  );
}

// ============================================================================
// Available Ethernet Ports section
// ============================================================================

interface AvailableEthernetSectionProps {
  ports: AvailableEthernetPort[];
  disabled: boolean;
  onAdd: (portName: string) => void;
  addingPort: boolean;
}

function AvailableEthernetSection({ ports, disabled, onAdd, addingPort }: AvailableEthernetSectionProps) {
  const [expanded, setExpanded] = useState(true);

  return (
    <div className="border-t border-theme-border pt-1">
      <button
        onClick={() => setExpanded(!expanded)}
        className="btn-ghost !w-full !px-0 !py-2 !text-xs font-medium flex items-center gap-1"
      >
        {expanded ? <ChevronDown className="w-3.5 h-3.5" /> : <ChevronRight className="w-3.5 h-3.5" />}
        <Globe className="w-3.5 h-3.5 text-theme-accent" />
        Available Ethernet Ports ({ports.length})
      </button>

      {expanded && (
        <div className="space-y-1.5 pb-2 pl-4">
          {ports.map((port) => (
            <div
              key={port.port_name}
              className="flex items-center gap-2 rounded-lg border border-theme-border bg-theme-bg-card backdrop-blur-xl p-2"
            >
              <span
                className={clsx(
                  'inline-block w-2 h-2 rounded-full flex-shrink-0',
                  port.link_status === 'up' ? 'bg-theme-success' : 'bg-theme-error',
                )}
                title={port.link_status === 'up' ? 'Link up' : 'Link down'}
              />
              <span className="text-xs font-medium text-theme-text-primary">{port.port_name}</span>
              <span className="text-[10px] text-theme-text-muted font-mono">{port.bridge}</span>
              <span className={clsx(
                'text-[10px]',
                port.link_status === 'up' ? 'text-theme-success' : 'text-theme-text-muted',
              )}>
                {port.link_status}
              </span>
              <div className="flex-1" />
              <button
                onClick={() => onAdd(port.port_name)}
                disabled={disabled || addingPort}
                className="btn-primary !px-2 !py-0.5 !text-[10px] min-h-[28px] flex items-center gap-1"
              >
                {addingPort
                  ? <Loader2 className="w-3 h-3 animate-spin" />
                  : <Plus className="w-3 h-3" />
                }
                Add to WAN
              </button>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

// ============================================================================
// ModemCard sub-component
// ============================================================================

interface ModemCardProps {
  modem: WanModemStatusEntry;
  index: number;
  activeCount: number;
  disabled: boolean;
  showRemoveConfirm: boolean;
  onShowRemoveConfirm: (show: boolean) => void;
  onMakePrimary: (id: string) => void;
  onSetState: (id: string, state: WanModemState) => void;
  onRemove: (id: string) => void;
  showSettings: boolean;
  onToggleSettings: () => void;
  onUpdateMtu: (value: number | null) => void;
  onUpdateTtl: (value: number | null) => void;
  onUpdateHopLimit: (value: number | null) => void;
  onUpdateProtoOverride: (value: string | null) => void;
  routingMode: RoutingMode;
  onUpdateWeight: (value: number | null) => void;
  totalActiveWeight: number;
}

function ModemCard({ modem, index, disabled, showRemoveConfirm, onShowRemoveConfirm, onMakePrimary, onSetState, onRemove, showSettings, onToggleSettings, onUpdateMtu, onUpdateTtl, onUpdateHopLimit, onUpdateProtoOverride, routingMode, onUpdateWeight, totalActiveWeight }: ModemCardProps) {
  const isEthernet = modem.entry_type === 'ethernet';
  const idDisplay = modem.imei ?? modem.modem_id;

  const isActive = modem.state === 'active';
  const hasCustomSettings = modem.mtu != null || modem.ttl != null || modem.hop_limit != null || modem.proto_override != null;

  // Position number among active modems (1-based)
  const activePosition = isActive ? index + 1 : null;

  // Remove confirmation message depends on entry type
  const removeMessage = isEthernet && modem.original_bridge
    ? `Remove and revert to LAN? This port will be returned to ${modem.original_bridge} on Save & Apply.`
    : isEthernet
      ? 'Remove from CTRL-WAN priority? The interface will remain configured but unmanaged.'
      : `Remove ${modem.label} from CTRL-WAN? Its network interface will be torn down when you Save & Apply.`;

  return (
    <div
      className={clsx(
        'rounded-2xl border-2 p-2.5 space-y-1.5 h-full bg-theme-bg-card backdrop-blur-xl',
        modem.is_primary
          ? 'border-theme-accent/40'
          : 'border-theme-border',
        !isActive && 'opacity-70',
      )}
    >
      {/* Top row: drag handle + status + name + badges */}
      <div className="flex items-center gap-2">
        <GripVertical
          className={clsx(
            'wan-drag-handle w-4 h-4 flex-shrink-0 text-theme-text-muted',
            disabled || !isActive ? 'opacity-30 cursor-default' : 'cursor-grab active:cursor-grabbing',
          )}
          aria-label="Drag to reorder"
        />
        <StatusDot status={modem.status} />
        {isEthernet && (
          <Globe className="w-3.5 h-3.5 text-theme-accent flex-shrink-0" />
        )}
        <span className="font-medium text-sm truncate flex-1 text-theme-text-primary">
          {modem.label}
          {modem.operator && (
            <span className="text-theme-text-muted font-normal"> — {modem.operator}</span>
          )}
        </span>
        {isEthernet && (
          <span className="text-[10px] px-1.5 py-0.5 rounded bg-theme-accent-muted text-theme-accent font-bold uppercase tracking-wide">
            ETH
          </span>
        )}
        {modem.is_primary && (
          <span className="text-[10px] px-1.5 py-0.5 rounded bg-theme-success/20 text-theme-success font-bold uppercase tracking-wide">
            Active Internet
          </span>
        )}
        {routingMode === 'load_balance' && isActive && !modem.is_primary && (
          <span className="text-[10px] px-1.5 py-0.5 rounded bg-theme-accent-muted text-theme-accent font-semibold uppercase tracking-wide">
            Balancing
          </span>
        )}
        {routingMode !== 'load_balance' && isActive && !modem.is_primary && activePosition !== null && (
          <span className="text-[10px] px-1.5 py-0.5 rounded bg-theme-accent-muted text-theme-accent font-semibold">
            #{activePosition} Failover
          </span>
        )}
        {!isActive && (
          <span className="text-[10px] px-1.5 py-0.5 rounded bg-theme-warning/15 text-theme-warning font-semibold uppercase">
            Standby
          </span>
        )}
        {modem.restart_suspended && (
          <span className="text-[10px] px-1.5 py-0.5 rounded bg-amber-100 text-amber-700 dark:bg-amber-900/30 dark:text-amber-400 font-medium whitespace-nowrap">
            Restart suspended
          </span>
        )}
        {routingMode === 'load_balance' && isActive && (
          <div className="flex items-center gap-2 px-3 py-1 rounded-lg border border-theme-accent/30 bg-theme-accent/5">
            <span className="text-xs text-theme-text-secondary uppercase tracking-wide font-medium">Weight</span>
            <input
              type="number"
              min={1}
              max={10}
              value={modem.weight ?? 1}
              onChange={(e) => {
                const v = parseInt(e.target.value, 10);
                onUpdateWeight(isNaN(v) ? null : Math.min(10, Math.max(1, v)));
              }}
              className="w-10 h-7 bg-transparent border border-theme-accent/30 rounded text-center text-sm font-semibold text-theme-text-primary outline-none focus:border-theme-accent"
              disabled={disabled}
            />
            <span className="text-sm text-theme-success font-semibold ml-1">{totalActiveWeight > 0 ? Math.round(((modem.weight ?? 1) / totalActiveWeight) * 100) : 0}%</span>
          </div>
        )}
        {routingMode === 'load_balance' && !isActive && (
          <span className="text-sm text-theme-text-muted font-medium">0%</span>
        )}
        <span className="text-[10px] text-theme-text-secondary font-mono">
          M:{modem.metric}
        </span>
      </div>

      {/* Info row */}
      <div className="flex items-center gap-3 text-[10px] text-theme-text-secondary font-mono">
        {!isEthernet && <span title={modem.imei ? 'IMEI' : 'Modem ID'}>{idDisplay}</span>}
        <span>{modem.interface_name}{modem.network_device ? ` / ${modem.network_device}` : ''}</span>
        {modem.consecutive_failures > 0 && (
          <span className="text-theme-error">{modem.consecutive_failures} fails</span>
        )}
      </div>

      {/* Converted LAN port origin */}
      {isEthernet && modem.original_bridge && (
        <div className="text-[10px] text-theme-text-secondary">
          (converted from {modem.original_bridge})
        </div>
      )}

      {/* Primary subtitle */}
      {modem.is_primary && (
        <div className="text-[10px] text-theme-success">
          {isEthernet
            ? 'This port is the router\'s default internet route'
            : 'This modem is the router\'s default internet route'
          }
        </div>
      )}

      {/* Health check indicators / No SIM notice */}
      {modem.status === 'no_sim' ? (
        <div className="text-[10px] text-theme-warning">
          No SIM card detected — health checks skipped
        </div>
      ) : modem.last_check && (
        <div className="flex items-center gap-3 text-[10px] text-theme-text-secondary">
          <span className="flex items-center gap-0.5">
            Ping <HealthIcon ok={modem.last_check.ping_ok} />
          </span>
          <span className="flex items-center gap-0.5">
            DNS4 <HealthIcon ok={modem.last_check.dns_v4_ok} />
          </span>
          <span className="flex items-center gap-0.5">
            DNS6 <HealthIcon ok={modem.last_check.dns_v6_ok} />
          </span>
          <span className="flex items-center gap-0.5">
            HTTP <HealthIcon ok={modem.last_check.http_ok} />
          </span>
        </div>
      )}

      {/* Action buttons */}
      <div className="flex items-center gap-1 pt-1">
        {/* State selector: Active / Standby */}
        <div className="flex rounded overflow-hidden border border-theme-border text-[10px]">
          <button
            onClick={() => onSetState(modem.modem_id, 'active')}
            disabled={disabled}
            className={clsx(
              'px-2 py-0.5 flex items-center gap-1 transition-colors border-b-2 outline-none focus-visible:ring-2 focus-visible:ring-theme-accent',
              isActive
                ? 'bg-theme-success/20 text-theme-success font-medium border-theme-success'
                : 'bg-theme-bg-tertiary text-theme-text-muted hover:bg-theme-bg-secondary border-transparent',
            )}
            title="Active — participates in failover rotation"
          >
            <Power className="w-3 h-3" />
            Active
          </button>
          <button
            onClick={() => onSetState(modem.modem_id, 'standby')}
            disabled={disabled}
            className={clsx(
              'px-2 py-0.5 flex items-center gap-1 transition-colors border-b-2 outline-none focus-visible:ring-2 focus-visible:ring-theme-accent',
              !isActive
                ? 'bg-theme-warning/20 text-theme-warning font-medium border-theme-warning'
                : 'bg-theme-bg-tertiary text-theme-text-muted hover:bg-theme-bg-secondary border-transparent',
            )}
            title="Standby — last resort if all active modems fail"
          >
            <PowerOff className="w-3 h-3" />
            Standby
          </button>
        </div>

        {/* Set as Internet Source */}
        {!modem.is_primary && isActive && (
          <button
            onClick={() => {
              if (window.confirm('Route all internet traffic through this modem? This takes effect on Save & Apply.')) {
                onMakePrimary(modem.modem_id);
              }
            }}
            disabled={disabled}
            className="btn-ghost !px-2 !py-0.5 !text-[10px] min-h-[28px] !text-theme-accent hover:!text-theme-accent-hover"
            title="Route all internet traffic through this modem"
          >
            Set as Internet Source
          </button>
        )}

        <div className="flex-1" />

        {/* Settings button */}
        <button
          onClick={onToggleSettings}
          disabled={disabled}
          className={clsx(
            'btn-ghost !px-2 !py-0.5 !text-[10px] min-h-[28px] flex items-center gap-1',
            showSettings
              ? '!text-theme-accent !bg-theme-accent-muted font-medium'
              : ''
          )}
          title="Interface settings (MTU, TTL)"
        >
          <Settings size={12} />
          Settings
        </button>

        {/* Settings summary when collapsed */}
        {!showSettings && hasCustomSettings && (
          <span className="text-[9px] text-theme-accent font-mono">
            {[
              modem.mtu != null ? `MTU:${modem.mtu}` : null,
              modem.ttl != null ? `TTL:${modem.ttl}` : null,
              modem.hop_limit != null ? `HL:${modem.hop_limit}` : null,
              modem.proto_override ? `proto:${modem.proto_override}` : null,
            ].filter(Boolean).join(' ')}
          </span>
        )}

        {/* Remove button */}
        <button
          onClick={() => onShowRemoveConfirm(true)}
          disabled={disabled}
          className="btn-icon-danger !p-1"
          title="Remove from CTRL-WAN"
        >
          <Trash2 className="w-3.5 h-3.5" />
        </button>
      </div>

      {/* Settings panel */}
      {showSettings && (
        <div className="pt-1.5 mt-1 border-t border-theme-border space-y-1.5">
          <div className="grid grid-cols-3 gap-2">
            <label className="text-[10px] text-theme-text-secondary">
              <span className="block mb-0.5">MTU</span>
              <input
                type="number"
                min={576}
                max={9000}
                placeholder="auto"
                value={modem.mtu ?? ''}
                onChange={(e) => {
                  const v = e.target.value;
                  onUpdateMtu(v === '' ? null : parseInt(v, 10) || null);
                }}
                onBlur={(e) => {
                  const v = e.target.value;
                  if (v !== '') {
                    const n = parseInt(v, 10);
                    if (!n || n < 576) onUpdateMtu(576);
                    else if (n > 9000) onUpdateMtu(9000);
                  }
                }}
                disabled={disabled}
                className="input-compact w-full !text-caption"
              />
            </label>
            <label className="text-[10px] text-theme-text-secondary">
              <span className="block mb-0.5">TTL (IPv4)</span>
              <input
                type="number"
                min={1}
                max={255}
                placeholder="off"
                value={modem.ttl ?? ''}
                onChange={(e) => {
                  const v = e.target.value;
                  onUpdateTtl(v === '' ? null : parseInt(v, 10) || null);
                }}
                onBlur={(e) => {
                  const v = e.target.value;
                  if (v !== '') {
                    const n = parseInt(v, 10);
                    if (!n || n < 1) onUpdateTtl(1);
                    else if (n > 255) onUpdateTtl(255);
                  }
                }}
                disabled={disabled}
                className="input-compact w-full !text-caption"
              />
            </label>
            <label className="text-[10px] text-theme-text-secondary">
              <span className="block mb-0.5">HL (IPv6)</span>
              <input
                type="number"
                min={1}
                max={255}
                placeholder="off"
                value={modem.hop_limit ?? ''}
                onChange={(e) => {
                  const v = e.target.value;
                  onUpdateHopLimit(v === '' ? null : parseInt(v, 10) || null);
                }}
                onBlur={(e) => {
                  const v = e.target.value;
                  if (v !== '') {
                    const n = parseInt(v, 10);
                    if (!n || n < 1) onUpdateHopLimit(1);
                    else if (n > 255) onUpdateHopLimit(255);
                  }
                }}
                disabled={disabled}
                className="input-compact w-full !text-caption"
              />
            </label>
          </div>
          <label className="text-[10px] text-theme-text-secondary block">
            <span className="block mb-0.5">Protocol override</span>
            <input
              type="text"
              maxLength={32}
              placeholder="auto"
              value={modem.proto_override ?? ''}
              onChange={(e) => {
                const v = e.target.value;
                onUpdateProtoOverride(v === '' ? null : v);
              }}
              onBlur={(e) => {
                const v = e.target.value.trim();
                onUpdateProtoOverride(v === '' ? null : v);
              }}
              disabled={disabled}
              className="input-compact w-full !text-caption"
            />
            <span className="block mt-0.5 text-[9px] text-theme-text-muted">
              Advanced: leave blank for automatic. Custom UCI proto value (see OpenWrt netifd documentation).
            </span>
          </label>
        </div>
      )}

      {/* Remove confirmation */}
      {showRemoveConfirm && (
        <div className="rounded border border-theme-error/30 bg-theme-error/10 p-2">
          <p className="text-[10px] text-theme-text-primary mb-1.5">
            {removeMessage}
          </p>
          <div className="flex gap-2">
            <button
              onClick={() => { onRemove(modem.modem_id); onShowRemoveConfirm(false); }}
              className="btn-danger !px-2 !py-0.5 !text-[10px] min-h-[28px]"
            >
              Remove
            </button>
            <button
              onClick={() => onShowRemoveConfirm(false)}
              className="btn-secondary !px-2 !py-0.5 !text-[10px] min-h-[28px]"
            >
              Cancel
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
