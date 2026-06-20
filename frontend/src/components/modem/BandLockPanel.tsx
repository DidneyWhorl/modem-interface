/**
 * Band Lock Panel
 *
 * Allows users to control network mode (LTE, 5G SA, 5G NSA, Auto, etc.)
 * and select which RF bands are enabled per technology. Changes are staged
 * locally and only applied when the user clicks "Save Changes" with confirmation.
 *
 * Band/mode configuration is profile-driven — the available modes, bands,
 * and AT commands come from the active modem profile.
 */

import { useState, useEffect, useCallback, useMemo } from 'react';
import { useBandConfig, useSetBandConfig, useRestoreBands, useActiveModemId } from '@/hooks';
import { useActiveProfile } from '@/hooks/queries/useModemProfiles';
import type { BandConfigRequest } from '@/types/api';
import { Radio, RefreshCw, RotateCcw, Save, ChevronDown, ChevronRight, Loader2, AlertTriangle, X } from 'lucide-react';
import clsx from 'clsx';

export function BandLockPanel() {
  const { data: activeProfile } = useActiveProfile();
  const modemId = useActiveModemId();
  const modemName = activeProfile?.profile?.model ?? 'Unknown Modem';
  const isGeneric = activeProfile?.profile?.is_generic ?? true;

  const {
    data: config,
    isLoading,
    isFetching,
    error,
    refetch,
  } = useBandConfig({ modemId, enabled: !isGeneric });

  const setBandMutation = useSetBandConfig();
  const restoreMutation = useRestoreBands();

  // Local staged state
  const [pendingMode, setPendingMode] = useState<string | null>(null);
  const [pendingLte, setPendingLte] = useState<Set<number>>(new Set());
  const [pendingNsa, setPendingNsa] = useState<Set<number>>(new Set());
  const [pendingSa, setPendingSa] = useState<Set<number>>(new Set());
  const [pendingNrdc, setPendingNrdc] = useState<Set<number>>(new Set());
  const [pendingNrdcEnabled, setPendingNrdcEnabled] = useState(false);
  const [nrdcExpanded, setNrdcExpanded] = useState(false);
  const [initialized, setInitialized] = useState(false);
  const [confirmAction, setConfirmAction] = useState<'save' | 'restore' | null>(null);

  // Initialize local state from server data
  useEffect(() => {
    if (config && !initialized) {
      setPendingMode(config.active_mode_id ?? config.supported_modes[0]?.id ?? null);
      setPendingLte(new Set(config.active_lte_bands));
      setPendingNsa(new Set(config.active_nsa_bands));
      setPendingSa(new Set(config.active_sa_bands));
      setPendingNrdc(new Set(config.active_nrdc_bands));
      setPendingNrdcEnabled(config.nrdc_enabled ?? false);
      setInitialized(true);
    }
  }, [config, initialized]);

  // Re-initialize when modem profile changes
  useEffect(() => {
    setInitialized(false);
  }, [activeProfile?.profile?.profile_id]);

  // Find the current mode definition
  const currentModeDef = useMemo(
    () => config?.supported_modes.find((m) => m.id === pendingMode) ?? null,
    [config?.supported_modes, pendingMode],
  );

  // Dirty tracking
  const isDirty = useMemo(() => {
    if (!config || !initialized) return false;
    if (pendingMode !== (config.active_mode_id ?? config.supported_modes[0]?.id)) return true;
    if (!setsEqual(pendingLte, new Set(config.active_lte_bands))) return true;
    if (!setsEqual(pendingNsa, new Set(config.active_nsa_bands))) return true;
    if (!setsEqual(pendingSa, new Set(config.active_sa_bands))) return true;
    if (config.has_nrdc) {
      if (!setsEqual(pendingNrdc, new Set(config.active_nrdc_bands))) return true;
      if (pendingNrdcEnabled !== (config.nrdc_enabled ?? false)) return true;
    }
    return false;
  }, [config, initialized, pendingMode, pendingLte, pendingNsa, pendingSa, pendingNrdc, pendingNrdcEnabled]);

  const handleToggleBand = useCallback(
    (_current: Set<number>, setFn: React.Dispatch<React.SetStateAction<Set<number>>>, band: number) => {
      setFn((prev) => {
        const next = new Set(prev);
        if (next.has(band)) {
          // Don't allow removing the last band
          if (next.size > 1) next.delete(band);
        } else {
          next.add(band);
        }
        return next;
      });
    },
    [],
  );

  const handleSelectAll = useCallback(
    (bands: number[], setFn: React.Dispatch<React.SetStateAction<Set<number>>>) => {
      setFn(new Set(bands));
    },
    [],
  );

  const handleSelectNone = useCallback(
    (bands: number[], setFn: React.Dispatch<React.SetStateAction<Set<number>>>) => {
      // Keep at least the first band
      const first = bands[0];
      if (first !== undefined) {
        setFn(new Set<number>([first]));
      }
    },
    [],
  );

  const handleSave = useCallback(async () => {
    if (!config || !pendingMode || !currentModeDef) return;

    const req: BandConfigRequest = {
      mode_id: pendingMode,
      lte_bands: [...pendingLte].sort((a, b) => a - b),
      nsa_bands: [...pendingNsa].sort((a, b) => a - b),
      sa_bands: [...pendingSa].sort((a, b) => a - b),
    };

    if (config.has_nrdc) {
      req.nrdc_bands = [...pendingNrdc].sort((a, b) => a - b);
      req.nrdc_enabled = pendingNrdcEnabled;
    }

    setConfirmAction(null);
    try {
      await setBandMutation.mutateAsync({ modemId: modemId!, config: req });
      // Delay before re-reading — modem needs time to apply AT commands
      await new Promise(r => setTimeout(r, 1000));
      setInitialized(false);
      refetch();
    } catch (e) {
      console.error('Band config apply failed:', e);
    }
  }, [config, pendingMode, currentModeDef, pendingLte, pendingNsa, pendingSa, pendingNrdc, pendingNrdcEnabled, setBandMutation, refetch, modemId]);

  const handleRestore = useCallback(async () => {
    setConfirmAction(null);
    try {
      await restoreMutation.mutateAsync({ modemId: modemId! });
      // Delay before re-reading — modem needs time to apply restore
      await new Promise(r => setTimeout(r, 1000));
      setInitialized(false);
      refetch();
    } catch (e) {
      console.error('Band restore failed:', e);
    }
  }, [restoreMutation, refetch, modemId]);

  const handleRefresh = useCallback(() => {
    setInitialized(false);
    refetch();
  }, [refetch]);

  // --- Render ---

  // Unsupported modem
  if (isGeneric) {
    return (
      <div className="p-4 h-full flex flex-col">
        <PanelHeader modemName={modemName} />
        <div className="flex-1 empty-state">
          <Radio className="w-8 h-8 text-theme-text-muted" />
          <p className="text-sm text-theme-text-secondary">Band control not supported</p>
          <p className="text-xs text-theme-text-muted">A specific modem profile is required for band locking</p>
        </div>
      </div>
    );
  }

  // Loading
  if (isLoading && !config) {
    return (
      <div className="p-4 h-full flex flex-col">
        <PanelHeader modemName={modemName} />
        <div className="flex-1 loading-state">
          <div className="loading-spinner" />
          <span>Loading band configuration...</span>
        </div>
      </div>
    );
  }

  // Error
  if (error && !config) {
    return (
      <div className="p-4 h-full flex flex-col">
        <PanelHeader modemName={modemName} />
        <div className="flex-1 error-state">
          <AlertTriangle className="w-8 h-8 text-theme-error" />
          <p className="text-sm text-theme-text-secondary">Failed to load band configuration</p>
          <button
            onClick={() => refetch()}
            className="btn-ghost text-xs"
          >
            Try again
          </button>
        </div>
      </div>
    );
  }

  if (!config) return null;

  const isSaving = setBandMutation.isPending || restoreMutation.isPending;

  return (
    <div className="p-4 h-full flex flex-col gap-3 overflow-y-auto">
      {/* Header */}
      <div className="flex items-center justify-between">
        <PanelHeader modemName={modemName} />
        <div className="flex items-center gap-1.5">
          {config.has_restore && (
            <button
              onClick={() => setConfirmAction('restore')}
              disabled={isSaving || !!confirmAction}
              title="Reset all bands to default"
              className="btn-icon p-1.5"
            >
              <RotateCcw className="w-4 h-4" />
            </button>
          )}
          <button
            onClick={handleRefresh}
            disabled={isFetching || isSaving}
            title="Refresh"
            className="btn-icon p-1.5"
          >
            <RefreshCw className={clsx('w-4 h-4', isFetching && 'animate-spin')} />
          </button>
        </div>
      </div>

      {/* Mode Selector */}
      <div>
        <div className="text-xs font-medium text-theme-text-secondary mb-1.5 uppercase tracking-wide">
          Network Mode
        </div>
        <div className="flex flex-wrap gap-1.5">
          {config.supported_modes.map((mode) => (
            <button
              key={mode.id}
              onClick={() => setPendingMode(mode.id)}
              disabled={isSaving}
              className={clsx(
                'px-3 py-1.5 rounded-md text-xs font-medium transition-colors',
                pendingMode === mode.id
                  ? 'bg-theme-accent text-white'
                  : 'bg-theme-bg-tertiary text-theme-text-secondary hover:bg-theme-bg-secondary hover:text-theme-text-primary',
                isSaving && 'opacity-50 cursor-not-allowed',
              )}
            >
              {mode.label}
            </button>
          ))}
        </div>
      </div>

      {/* LTE Bands */}
      <BandSection
        title="LTE Bands"
        prefix="B"
        supportedBands={config.supported_lte_bands}
        activeBands={pendingLte}
        disabled={!currentModeDef?.active_sections.lte}
        isSaving={isSaving}
        onToggle={(b) => handleToggleBand(pendingLte, setPendingLte, b)}
        onSelectAll={() => handleSelectAll(config.supported_lte_bands, setPendingLte)}
        onSelectNone={() => handleSelectNone(config.supported_lte_bands, setPendingLte)}
      />

      {/* 5G NSA Bands */}
      <BandSection
        title="5G NSA Bands"
        prefix="n"
        supportedBands={config.supported_nsa_bands}
        activeBands={pendingNsa}
        disabled={!currentModeDef?.active_sections.nsa}
        isSaving={isSaving}
        onToggle={(b) => handleToggleBand(pendingNsa, setPendingNsa, b)}
        onSelectAll={() => handleSelectAll(config.supported_nsa_bands, setPendingNsa)}
        onSelectNone={() => handleSelectNone(config.supported_nsa_bands, setPendingNsa)}
      />

      {/* 5G SA Bands */}
      <BandSection
        title="5G SA Bands"
        prefix="n"
        supportedBands={config.supported_sa_bands}
        activeBands={pendingSa}
        disabled={!currentModeDef?.active_sections.sa}
        isSaving={isSaving}
        onToggle={(b) => handleToggleBand(pendingSa, setPendingSa, b)}
        onSelectAll={() => handleSelectAll(config.supported_sa_bands, setPendingSa)}
        onSelectNone={() => handleSelectNone(config.supported_sa_bands, setPendingSa)}
      />

      {/* NRDC Section (advanced, collapsible) */}
      {config.has_nrdc && (
        <div className="border border-theme-border rounded-lg overflow-hidden">
          <button
            onClick={() => setNrdcExpanded(!nrdcExpanded)}
            className="w-full flex items-center gap-2 px-3 py-2 text-xs font-medium text-theme-text-secondary hover:bg-theme-bg-secondary transition-colors"
          >
            {nrdcExpanded ? (
              <ChevronDown className="w-3.5 h-3.5" />
            ) : (
              <ChevronRight className="w-3.5 h-3.5" />
            )}
            <span className="uppercase tracking-wide">Advanced: NR Dual Connectivity</span>
            <span className="text-theme-text-muted ml-auto text-[10px]">Experimental</span>
          </button>
          {nrdcExpanded && (
            <div className="px-3 pb-3 space-y-2">
              <label className="flex items-center gap-2 text-xs text-theme-text-secondary">
                <input
                  type="checkbox"
                  checked={pendingNrdcEnabled}
                  onChange={(e) => setPendingNrdcEnabled(e.target.checked)}
                  disabled={isSaving}
                  className="rounded"
                />
                NRDC Enabled
              </label>
              <BandSection
                title="NRDC Bands"
                prefix="n"
                supportedBands={config.supported_nrdc_bands}
                activeBands={pendingNrdc}
                disabled={!pendingNrdcEnabled}
                isSaving={isSaving}
                onToggle={(b) => handleToggleBand(pendingNrdc, setPendingNrdc, b)}
                onSelectAll={() => handleSelectAll(config.supported_nrdc_bands, setPendingNrdc)}
                onSelectNone={() => handleSelectNone(config.supported_nrdc_bands, setPendingNrdc)}
                compact
              />
            </div>
          )}
        </div>
      )}

      {/* Confirmation Banner */}
      {confirmAction && (
        <ConfirmBanner
          action={confirmAction}
          rebootWarning={confirmAction === 'save' && !!config.reboot_on_band_change}
          modeSummary={confirmAction === 'save' && currentModeDef ? currentModeDef.label : undefined}
          onConfirm={confirmAction === 'save' ? handleSave : handleRestore}
          onCancel={() => setConfirmAction(null)}
        />
      )}

      {/* Footer */}
      <div className="flex items-center justify-between pt-1 border-t border-theme-border mt-auto">
        <div className="text-xs text-theme-text-muted">
          {isDirty && !confirmAction && <span className="text-theme-warning">Unsaved changes</span>}
          {setBandMutation.isSuccess && !isDirty && (
            <span className="text-theme-success">Applied</span>
          )}
          {setBandMutation.isError && (
            <span className="text-theme-error">
              Error: {(setBandMutation.error as Error)?.message ?? 'Failed'}
            </span>
          )}
        </div>
        <button
          onClick={() => setConfirmAction('save')}
          disabled={!isDirty || isSaving || !!confirmAction}
          className="btn-primary flex items-center gap-1.5 px-3 py-1.5 text-xs"
        >
          {isSaving ? (
            <Loader2 className="w-3.5 h-3.5 animate-spin" />
          ) : (
            <Save className="w-3.5 h-3.5" />
          )}
          Save Changes
        </button>
      </div>
    </div>
  );
}

// =============================================================================
// Sub-components
// =============================================================================

function PanelHeader({ modemName }: { modemName: string }) {
  return (
    <div className="flex items-center gap-2">
      <Radio className="w-5 h-5 text-theme-text-muted" />
      <h2 className="text-lg font-medium text-theme-text-primary">Band Lock</h2>
      <span className="text-sm text-theme-text-secondary font-medium">— {modemName}</span>
    </div>
  );
}

interface BandSectionProps {
  title: string;
  prefix: string;
  supportedBands: number[];
  activeBands: Set<number>;
  disabled: boolean;
  isSaving: boolean;
  onToggle: (band: number) => void;
  onSelectAll: () => void;
  onSelectNone: () => void;
  compact?: boolean;
}

function BandSection({
  title,
  prefix,
  supportedBands,
  activeBands,
  disabled,
  isSaving,
  onToggle,
  onSelectAll,
  onSelectNone,
  compact,
}: BandSectionProps) {
  const activeCount = supportedBands.filter((b) => activeBands.has(b)).length;

  return (
    <div className={clsx('relative rounded-lg', disabled && 'opacity-40')}>
      {!compact && (
        <div className="flex items-center justify-between mb-1.5">
          <div className="text-xs font-medium text-theme-text-secondary uppercase tracking-wide">
            {title}{' '}
            <span className="text-theme-text-muted normal-case tracking-normal">
              ({activeCount}/{supportedBands.length})
            </span>
          </div>
          {!disabled && (
            <div className="flex items-center gap-2 text-[10px]">
              <button
                onClick={onSelectAll}
                disabled={isSaving}
                className="text-theme-text-muted hover:text-theme-text-secondary transition-colors"
              >
                All
              </button>
              <span className="text-theme-border">|</span>
              <button
                onClick={onSelectNone}
                disabled={isSaving}
                className="text-theme-text-muted hover:text-theme-text-secondary transition-colors"
              >
                None
              </button>
            </div>
          )}
        </div>
      )}
      <div className="flex flex-wrap gap-2">
        {supportedBands.map((band) => {
          const active = activeBands.has(band);
          return (
            <button
              key={band}
              onClick={() => !disabled && onToggle(band)}
              disabled={disabled || isSaving}
              className={clsx(
                'w-[4.25rem] px-2 py-2 rounded text-[13px] font-mono font-medium transition-colors text-center',
                disabled && 'cursor-not-allowed',
                !disabled && active && 'bg-theme-accent text-white',
                !disabled && !active && 'bg-theme-bg-tertiary text-theme-text-muted hover:bg-theme-bg-secondary',
              )}
            >
              {prefix}{band}
            </button>
          );
        })}
      </div>
      {disabled && (
        <div className="absolute inset-0 flex items-center justify-center">
          <span className="text-[10px] text-theme-text-muted bg-theme-bg-primary/80 px-2 py-0.5 rounded">
            Disabled by mode
          </span>
        </div>
      )}
    </div>
  );
}

interface ConfirmBannerProps {
  action: 'save' | 'restore';
  rebootWarning: boolean;
  modeSummary?: string;
  onConfirm: () => void;
  onCancel: () => void;
}

function ConfirmBanner({ action, rebootWarning, modeSummary, onConfirm, onCancel }: ConfirmBannerProps) {
  return (
    <div className={clsx(
      'rounded-lg border p-3 space-y-2',
      rebootWarning
        ? 'bg-theme-error/10 border-theme-error/30'
        : 'bg-theme-warning/10 border-theme-warning/30',
    )}>
      <div className="flex items-start gap-2">
        <AlertTriangle className={clsx('w-4 h-4 mt-0.5 shrink-0', rebootWarning ? 'text-theme-error' : 'text-theme-warning')} />
        <div className="flex-1 text-xs">
          {action === 'save' ? (
            <>
              <p className="font-medium text-theme-text-primary">Apply band configuration?</p>
              {modeSummary && (
                <p className="text-theme-text-secondary mt-0.5">Mode: {modeSummary}</p>
              )}
              {rebootWarning && (
                <p className="text-theme-error mt-1 font-medium">The modem will reboot to apply these changes.</p>
              )}
            </>
          ) : (
            <>
              <p className="font-medium text-theme-text-primary">Restore all bands to factory default?</p>
              <p className="text-theme-text-secondary mt-0.5">This will reset all band selections and mode.</p>
            </>
          )}
        </div>
        <button onClick={onCancel} className="p-0.5 text-theme-text-muted hover:text-theme-text-primary">
          <X className="w-3.5 h-3.5" />
        </button>
      </div>
      <div className="flex items-center gap-2 justify-end">
        <button
          onClick={onCancel}
          className="btn-secondary px-3 py-1 text-xs"
        >
          Cancel
        </button>
        <button
          onClick={onConfirm}
          className={clsx(
            rebootWarning
              ? 'btn-danger px-3 py-1 text-xs bg-theme-error text-white hover:bg-theme-error/90'
              : 'btn-primary px-3 py-1 text-xs',
          )}
        >
          {action === 'save' ? 'Apply' : 'Restore'}
        </button>
      </div>
    </div>
  );
}

// =============================================================================
// Helpers
// =============================================================================

function setsEqual(a: Set<number>, b: Set<number>): boolean {
  if (a.size !== b.size) return false;
  for (const val of a) {
    if (!b.has(val)) return false;
  }
  return true;
}
