/**
 * ApnEditor — Item #42 Phase 3
 *
 * Self-contained APN configuration editor.
 *
 * Props:
 *   modemId: string  (from useActiveModemId() in parent)
 *
 * Actions:
 *   Apply     — diff-aware write (live or reboot when MBN changes)
 *   Reconnect — radio cycle, no APN change
 *   Disconnect — radio off
 *   Refresh   — refetch current_config from modem
 *
 * Design rules enforced:
 *   - OKLCH design tokens only (no hex / arbitrary colors)
 *   - No emojis
 *   - Labels above every field
 *   - One primary action (Apply) dominant
 *   - Dirty-state gating on Apply
 *   - Password placeholder rule: never send the masked sentinel
 *   - No mode strings (ECM/QMI/MBIM/RmNet) anywhere
 */

import { useState, useEffect, useCallback, useRef } from 'react';
import { RefreshCw, Loader2, PowerOff, RotateCcw, Save, List, Ban, AlertTriangle, X } from 'lucide-react';
import { usePdpDetails } from '@/hooks/queries/usePdpDetails';
import { useApplyApn, useReconnect } from '@/hooks/mutations/useApnApply';
import { useDisconnect, useActiveProfile, useCreateApnProfile, useDeactivateMbnProfile } from '@/hooks';
import { rebootModem } from '@/api/modem';
import { ApnFormFields } from './ApnFormFields';
import { AdvancedFields } from './AdvancedFields';
import { MbnSelector, MBN_AUTO_VALUE } from './MbnSelector';
import { ConfirmBanner } from './ConfirmBanner';
import { ResultFeedback } from './ResultFeedback';
import { SaveProfileDialog } from './SaveProfileDialog';
import type { AuthType, IpType, ApnApplyResult } from '@/types/api';
import type { ApnFormValues } from './ApnFormFields';
import type { AdvancedFieldValues } from './AdvancedFields';

// ============================================================================
// Types
// ============================================================================

interface ApnEditorProps {
  modemId: string;
  /** Opens the Manage Profiles dialog (owned by ConnectionPanel). */
  onManageProfiles: () => void;
}

type ConfirmKind = 'mbn-reboot' | 'reconnect-dirty' | 'mbn-deactivate';

interface FormState {
  cid: number;
  apn: string;
  ip_type: IpType;
  auth_type: AuthType;
  username: string;
  password: string;
  passwordEdited: boolean;
  mbnValue: string;
}

// ============================================================================
// Helpers
// ============================================================================

/** Derive the initial MBN dropdown value from pdp data. */
function deriveMbnValue(
  mbnSupported: boolean,
  mbnSelectedProfile: string | null,
): string {
  if (!mbnSupported) return MBN_AUTO_VALUE;
  if (mbnSelectedProfile) return mbnSelectedProfile;
  // auto-select or null (no info) → Auto
  return MBN_AUTO_VALUE;
}

/** Resolve fallback CID when current_config.cid is null. */
function resolveCid(configCid: number | null, pdpContexts: { cid: string }[]): number {
  if (configCid != null) return configCid;
  const sorted = [...pdpContexts]
    .map((c) => parseInt(c.cid, 10))
    .filter((n) => !isNaN(n))
    .sort((a, b) => a - b);
  return sorted[0] ?? 1;
}

/** Build a stable snapshot string for dirty-state comparison. */
function stateKey(s: FormState): string {
  return JSON.stringify({
    cid: s.cid,
    apn: s.apn,
    ip_type: s.ip_type,
    auth_type: s.auth_type,
    username: s.username,
    mbnValue: s.mbnValue,
    // Include passwordEdited + value only when edited (cleared counts as a change)
    pw: s.passwordEdited ? s.password : '__unchanged__',
  });
}

// ============================================================================
// Component
// ============================================================================

export function ApnEditor({ modemId, onManageProfiles }: ApnEditorProps) {
  const {
    data: pdp,
    isFetching: pdpFetching,
    isError: pdpError,
    refetch: refetchPdp,
  } = usePdpDetails();
  const applyApn = useApplyApn();
  const reconnectMutation = useReconnect();
  const { mutate: disconnect, isPending: isDisconnecting } = useDisconnect();
  const { data: activeModemProfile } = useActiveProfile();
  const createProfile = useCreateApnProfile();
  const deactivateMbn = useDeactivateMbnProfile();

  // "Save as Custom Profile" inline dialog
  const [showSaveDialog, setShowSaveDialog] = useState(false);

  // Form state
  const [form, setForm] = useState<FormState>({
    cid: 1,
    apn: '',
    ip_type: 'ipv4',
    auth_type: 'none',
    username: '',
    password: '',
    passwordEdited: false,
    mbnValue: MBN_AUTO_VALUE,
  });

  // Snapshot of the loaded config — used for dirty detection
  const loadedKeyRef = useRef<string>('');

  // Which confirmation banner is shown
  const [confirmKind, setConfirmKind] = useState<ConfirmKind | null>(null);

  // Apply result (cleared on any form edit)
  const [lastResult, setLastResult] = useState<ApnApplyResult | null>(null);

  // APN validation error
  const [apnError, setApnError] = useState<string | undefined>();

  // ============================================================================
  // Sync form from PDP data
  // ============================================================================

  const syncFormFromPdp = useCallback(
    (data: typeof pdp) => {
      if (!data) return;
      const { current_config, pdp_contexts, mbn_supported, mbn_selected_profile } = data;
      const resolvedCid = resolveCid(current_config.cid, pdp_contexts);
      const mbnValue = deriveMbnValue(mbn_supported, mbn_selected_profile);

      const next: FormState = {
        cid: resolvedCid,
        apn: current_config.apn,
        ip_type: current_config.ip_type,
        auth_type: current_config.auth_type,
        username: current_config.username,
        password: '',
        passwordEdited: false,
        mbnValue,
      };

      setForm(next);
      loadedKeyRef.current = stateKey(next);
      setLastResult(null);
      setApnError(undefined);
    },
    [],
  );

  // Sync on initial PDP load
  useEffect(() => {
    if (pdp) syncFormFromPdp(pdp);
  }, [pdp, syncFormFromPdp]);

  // Auto-fetch on mount
  useEffect(() => {
    refetchPdp();
  }, [refetchPdp]);

  // ============================================================================
  // Dirty state
  // ============================================================================

  const isDirty = stateKey(form) !== loadedKeyRef.current;

  // ============================================================================
  // Form change handlers
  // ============================================================================

  function handleCoreChange<K extends keyof ApnFormValues>(field: K, value: ApnFormValues[K]) {
    setForm((f) => ({ ...f, [field]: value }));
    setLastResult(null);
    if (field === 'apn') setApnError(undefined);
  }

  function handleAdvancedChange(next: Partial<AdvancedFieldValues>) {
    setForm((f) => ({ ...f, ...next }));
    setLastResult(null);
  }

  function handleMbnChange(value: string) {
    setForm((f) => ({ ...f, mbnValue: value }));
    setLastResult(null);
  }

  // ============================================================================
  // Validation
  // ============================================================================

  function validate(): boolean {
    if (!form.apn.trim()) {
      setApnError('APN is required.');
      return false;
    }
    if (form.apn.length > 100) {
      setApnError('APN must be 100 characters or fewer.');
      return false;
    }
    return true;
  }

  // ============================================================================
  // Determine whether MBN is changing (triggers reboot warning)
  // ============================================================================

  function isMbnChanging(): boolean {
    if (!pdp?.mbn_supported) return false;
    const loadedMbn = deriveMbnValue(pdp.mbn_supported, pdp.mbn_selected_profile);
    return form.mbnValue !== loadedMbn;
  }

  // ============================================================================
  // Apply
  // ============================================================================

  function handleApplyClick() {
    if (!validate()) return;
    if (isMbnChanging()) {
      setConfirmKind('mbn-reboot');
    } else {
      submitApply();
    }
  }

  function submitApply() {
    setConfirmKind(null);

    const mbnSupported = pdp?.mbn_supported ?? false;
    // Build mbn_profile field:
    //   - unsupported: omit
    //   - Auto: null (or "__auto__" — backend accepts both)
    //   - named: the string
    const mbnProfile: string | null | undefined = mbnSupported
      ? form.mbnValue === MBN_AUTO_VALUE
        ? null
        : form.mbnValue
      : undefined;

    // Build password field per the placeholder rule:
    //   - not edited → omit (leave stored password unchanged)
    //   - edited, typed a value → that string
    //   - edited, cleared → "" (explicitly clears the stored password)
    const passwordField: string | undefined = form.passwordEdited
      ? form.password
      : undefined;

    applyApn.mutate(
      {
        modemId,
        req: {
          cid: form.cid,
          apn: form.apn.trim(),
          ip_type: form.ip_type,
          auth_type: form.auth_type,
          username: form.username || undefined,
          password: passwordField,
          mbn_profile: mbnProfile,
        },
      },
      {
        onSuccess: (result) => {
          setLastResult(result);
          // useApplyApn invalidates pdpDetailsQueryKey on success, which triggers
          // a refetch automatically and re-syncs the form via the pdp useEffect.
          // No explicit refetchPdp() needed here.
        },
        onError: () => {
          // Form is preserved on error — user can retry
        },
      },
    );
  }

  // ============================================================================
  // Reconnect
  // ============================================================================

  function handleReconnectClick() {
    if (isDirty) {
      setConfirmKind('reconnect-dirty');
    } else {
      submitReconnect();
    }
  }

  function submitReconnect() {
    setConfirmKind(null);
    reconnectMutation.mutate(
      { modemId },
      {
        onSuccess: () => {
          setLastResult(null);
          // useReconnect invalidates pdpDetailsQueryKey on success — no explicit refetch needed.
        },
      },
    );
  }

  // ============================================================================
  // Disconnect
  // ============================================================================

  function handleDisconnectClick() {
    disconnect(undefined, {
      onSuccess: () => {
        setLastResult(null);
        // useDisconnect does NOT invalidate pdpDetailsQueryKey, so we must
        // refetch explicitly here to re-sync the form after radio off.
        refetchPdp();
      },
    });
  }

  // ============================================================================
  // Deactivate current MBN carrier profile (mirrors the old MbnSection flow)
  // ============================================================================

  async function deactivateMbnProfile(andReboot: boolean) {
    setConfirmKind(null);
    try {
      await deactivateMbn.mutateAsync({ modemId });
      if (andReboot) {
        try { await rebootModem(modemId); } catch { /* reboot endpoint handles state */ }
      } else {
        setTimeout(() => refetchPdp(), 500);
      }
    } catch (e) {
      console.error('MBN deactivate failed:', e);
    }
  }

  // ============================================================================
  // Save as Custom Profile — captures the live form
  // ============================================================================

  async function handleSaveProfile(name: string) {
    const modemProfileId = activeModemProfile?.profile?.profile_id ?? 'generic';
    // ApnProfileRequest.mbn_profile is string | undefined (undefined = Auto).
    const mbnProfile =
      pdp?.mbn_supported && form.mbnValue !== MBN_AUTO_VALUE ? form.mbnValue : undefined;

    try {
      await createProfile.mutateAsync({
        modemId,
        req: {
          name,
          modem_profile_id: modemProfileId,
          connection: {
            cid: form.cid,
            apn: form.apn.trim(),
            username: form.username || undefined,
            // Password placeholder rule (mirrors handleApply):
            //   - not edited → omit the field (undefined) so the backend
            //     captures/preserves the live modem password. The frontend
            //     never holds the value (only has_password).
            //   - edited (including deliberately cleared to "") → send the
            //     literal value, which explicitly sets/clears the password.
            password: form.passwordEdited ? form.password : undefined,
            auth_type: form.auth_type,
            ip_type: form.ip_type,
          },
          mbn_profile: mbnProfile,
        },
      });
      setShowSaveDialog(false);
    } catch (e) {
      console.error('Failed to save APN profile:', e);
    }
  }

  // ============================================================================
  // Confirm banner handlers
  // ============================================================================

  function handleConfirm() {
    if (confirmKind === 'mbn-reboot') submitApply();
    else if (confirmKind === 'reconnect-dirty') submitReconnect();
  }

  function handleConfirmCancel() {
    setConfirmKind(null);
  }

  // ============================================================================
  // Derived pending state
  // ============================================================================

  const isApplying = applyApn.isPending;
  const isReconnecting = reconnectMutation.isPending;
  const isDeactivatingMbn = deactivateMbn.isPending;
  const isAnyBusy = isApplying || isReconnecting || isDisconnecting || isDeactivatingMbn || pdpFetching;

  // ============================================================================
  // Loading / empty state
  // ============================================================================

  if (pdpFetching && !pdp) {
    return (
      <div className="flex items-center justify-center gap-2 py-8 text-sm text-theme-text-muted">
        <Loader2 className="w-4 h-4 animate-spin" />
        Loading APN configuration...
      </div>
    );
  }

  if (pdpError && !pdp) {
    return (
      <div className="error-state">
        <p className="text-sm text-theme-error font-medium">
          Couldn&apos;t load APN configuration from the modem.
        </p>
        <p className="text-xs text-theme-text-muted">
          Check that the modem is connected and responding.
        </p>
        <button
          type="button"
          onClick={() => refetchPdp()}
          disabled={pdpFetching}
          className="btn-secondary mt-2 flex items-center gap-2"
        >
          {pdpFetching ? (
            <>
              <Loader2 className="w-4 h-4 animate-spin" />
              Retrying...
            </>
          ) : (
            <>
              <RefreshCw className="w-4 h-4" />
              Retry
            </>
          )}
        </button>
      </div>
    );
  }

  // ============================================================================
  // Render
  // ============================================================================

  const mbnProfiles = pdp?.mbn_profiles ?? [];
  const mbnSupported = pdp?.mbn_supported ?? false;
  const hasStoredPassword = pdp?.current_config.has_password ?? false;

  // Currently-active carrier profile (if any) — gates the Deactivate control.
  const activeMbnProfile = (pdp?.mbn_profiles ?? []).find((p) => p.selected || p.activated) ?? null;

  const confirmBannerProps =
    confirmKind === 'mbn-reboot'
      ? {
          title: 'Carrier profile change requires a reboot',
          body: 'The modem will reboot to apply the new carrier profile. It will reconnect automatically after rebooting.',
          confirmLabel: 'Apply and Reboot',
          isPending: isApplying,
          variant: 'warning' as const,
        }
      : confirmKind === 'reconnect-dirty'
      ? {
          title: 'Unsaved edits will not be applied',
          body: 'Reconnect cycles the radio using the APN already saved on the modem. Your current edits will not be applied. Apply first if you want the new settings to take effect.',
          confirmLabel: 'Reconnect Anyway',
          isPending: isReconnecting,
          variant: 'caution' as const,
        }
      : null;

  return (
    <div className="space-y-5">
      {/* Header row — Refresh button */}
      <div className="flex items-center justify-between">
        <h3 className="text-sm font-semibold text-theme-text-primary">APN Configuration</h3>
        <button
          type="button"
          onClick={() => refetchPdp()}
          disabled={isAnyBusy}
          className="btn-icon"
          aria-label="Refresh APN configuration from modem"
          title="Refresh from modem"
        >
          <RefreshCw className={`w-4 h-4 ${pdpFetching ? 'animate-spin' : ''}`} />
        </button>
      </div>

      {/* Core fields */}
      <ApnFormFields
        values={{ cid: form.cid, apn: form.apn, ip_type: form.ip_type, auth_type: form.auth_type }}
        onChange={handleCoreChange}
        disabled={isAnyBusy}
        apnError={apnError}
      />

      {/* Advanced (username + password) */}
      <AdvancedFields
        values={{ username: form.username, password: form.password, passwordEdited: form.passwordEdited }}
        hasStoredPassword={hasStoredPassword}
        onChange={handleAdvancedChange}
        disabled={isAnyBusy}
      />

      {/* MBN carrier profile */}
      <MbnSelector
        profiles={mbnProfiles}
        supported={mbnSupported}
        value={form.mbnValue}
        onChange={handleMbnChange}
        disabled={isAnyBusy}
      />

      {/* Deactivate the currently-active carrier profile (MBN-capable + active only) */}
      {mbnSupported && activeMbnProfile !== null && (
        <button
          type="button"
          onClick={() => setConfirmKind('mbn-deactivate')}
          disabled={isAnyBusy || !!confirmKind}
          className="flex items-center justify-center gap-1.5 px-3 py-1.5 text-xs font-medium
                     rounded-md transition-colors
                     border border-theme-border text-theme-text-secondary
                     hover:bg-theme-bg-tertiary hover:text-theme-text-primary
                     disabled:opacity-40 disabled:cursor-not-allowed"
          title="Clear the active carrier profile on the modem"
        >
          {isDeactivatingMbn ? (
            <Loader2 className="w-3.5 h-3.5 animate-spin" aria-hidden="true" />
          ) : (
            <Ban className="w-3.5 h-3.5" aria-hidden="true" />
          )}
          Deactivate Current Profile
        </button>
      )}

      {/* Deactivate confirmation — Deactivate vs Deactivate & Reboot */}
      {confirmKind === 'mbn-deactivate' && (
        <div className="rounded-lg border p-2.5 space-y-2 bg-theme-warning/10 border-theme-warning/30">
          <div className="flex items-start gap-2">
            <AlertTriangle className="w-3.5 h-3.5 mt-0.5 shrink-0 text-theme-warning" aria-hidden="true" />
            <div className="flex-1 text-xs">
              <p className="font-medium text-theme-text-primary">
                Deactivate current carrier profile?
              </p>
              <p className="text-theme-text-secondary mt-1 text-[10px] leading-relaxed">
                This clears the active carrier profile. The modem will fall back to default
                behavior. A reboot is recommended for changes to take full effect.
              </p>
            </div>
            <button
              type="button"
              onClick={handleConfirmCancel}
              className="p-0.5 text-theme-text-muted hover:text-theme-text-primary"
              aria-label="Dismiss"
            >
              <X className="w-3 h-3" aria-hidden="true" />
            </button>
          </div>
          <div className="flex items-center gap-2 justify-end flex-wrap">
            <button
              type="button"
              onClick={handleConfirmCancel}
              className="px-2.5 py-1 rounded-md text-[10px] font-medium text-theme-text-secondary hover:bg-theme-bg-tertiary transition-colors"
            >
              Cancel
            </button>
            <button
              type="button"
              onClick={() => deactivateMbnProfile(false)}
              disabled={isDeactivatingMbn}
              className="px-2.5 py-1 rounded-md text-[10px] font-medium text-white bg-theme-warning hover:opacity-90 transition-colors disabled:opacity-50"
            >
              Deactivate
            </button>
            <button
              type="button"
              onClick={() => deactivateMbnProfile(true)}
              disabled={isDeactivatingMbn}
              className="flex items-center gap-1 px-2.5 py-1 rounded-md text-[10px] font-medium text-white bg-theme-accent hover:opacity-90 transition-colors disabled:opacity-50"
            >
              <RotateCcw className="w-2.5 h-2.5" aria-hidden="true" />
              Deactivate & Reboot
            </button>
          </div>
        </div>
      )}

      {/* Confirmation banner (MBN reboot or reconnect-dirty) */}
      {confirmKind && confirmBannerProps && (
        <ConfirmBanner
          {...confirmBannerProps}
          onConfirm={handleConfirm}
          onCancel={handleConfirmCancel}
        />
      )}

      {/* Result feedback */}
      {lastResult && !confirmKind && (
        <ResultFeedback
          tone={
            !lastResult.success
              ? 'error'
              : lastResult.had_errors
                ? 'warning'
                : 'success'
          }
          title={lastResult.message}
          stepLog={lastResult.step_log}
          rebooted={lastResult.rebooted}
        />
      )}

      {/* Action buttons */}
      <div className="pt-4 border-t border-theme-border space-y-2">
        {/* Primary: Apply */}
        <button
          type="button"
          onClick={handleApplyClick}
          disabled={!isDirty || isAnyBusy || !!confirmKind}
          className="btn-primary w-full flex items-center justify-center gap-2"
          aria-busy={isApplying}
        >
          {isApplying ? (
            <>
              <Loader2 className="w-4 h-4 animate-spin" />
              Applying...
            </>
          ) : (
            'Apply'
          )}
        </button>

        {/* Secondary row: Reconnect + Disconnect */}
        <div className="grid grid-cols-2 gap-2">
          <button
            type="button"
            onClick={handleReconnectClick}
            disabled={isAnyBusy || !!confirmKind}
            className="btn-secondary flex items-center justify-center gap-2"
            aria-busy={isReconnecting}
            title="Cycle the radio to force re-registration"
          >
            {isReconnecting ? (
              <>
                <Loader2 className="w-4 h-4 animate-spin" />
                Reconnecting...
              </>
            ) : (
              <>
                <RotateCcw className="w-4 h-4" />
                Reconnect
              </>
            )}
          </button>

          <button
            type="button"
            onClick={handleDisconnectClick}
            disabled={isAnyBusy || !!confirmKind}
            className="btn-danger flex items-center justify-center gap-2"
            aria-busy={isDisconnecting}
            title="Turn the radio off"
          >
            {isDisconnecting ? (
              <>
                <Loader2 className="w-4 h-4 animate-spin" />
                Disconnecting...
              </>
            ) : (
              <>
                <PowerOff className="w-4 h-4" />
                Disconnect
              </>
            )}
          </button>
        </div>

        {/* Profile actions: Save as Custom Profile + Manage Profiles */}
        <div className="grid grid-cols-2 gap-2">
          <button
            type="button"
            onClick={() => setShowSaveDialog(true)}
            disabled={!form.apn.trim() || isAnyBusy || showSaveDialog || !!confirmKind}
            className="btn-secondary flex items-center justify-center gap-1.5 text-xs"
            title="Save the current settings as a reusable custom profile"
          >
            <Save className="w-3.5 h-3.5" aria-hidden="true" />
            Save as Custom Profile
          </button>
          <button
            type="button"
            onClick={onManageProfiles}
            className="btn-secondary flex items-center justify-center gap-1.5 text-xs"
            title="View, apply, rename, and delete saved profiles"
          >
            <List className="w-3.5 h-3.5" aria-hidden="true" />
            Manage Profiles
          </button>
        </div>

        {/* Save dialog */}
        {showSaveDialog && (
          <SaveProfileDialog
            onSave={handleSaveProfile}
            onCancel={() => setShowSaveDialog(false)}
            isPending={createProfile.isPending}
          />
        )}
      </div>
    </div>
  );
}
