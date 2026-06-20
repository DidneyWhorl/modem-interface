/**
 * ConnectionPanel Component — Item #42 Phase 3 (thin composition root)
 *
 * Composes the APN/PDP panel from focused sub-components. No conflated Connect
 * form, no Show/Hide Details disclosure. Top to bottom:
 *   1. Header (status badge, dual-SIM badge)
 *   2. SimSlotSection (dual-SIM only)
 *   3. "SIM N Connection Settings" divider (dual-SIM only)
 *   4. ApnEditor (APN form + Apply/Reconnect/Disconnect + Save/Manage)
 *   5. PdpContextList (always visible, read-only)
 *   6. ProfileManagerDialog (inline card, extracted to apn/ProfileManagerDialog)
 */

import { useState } from 'react';
import {
  useApnProfiles, useActiveProfile, useActiveModemId, useModemStatus,
  useSimSlots, useUpdateSimSlotConfig, useSwitchSimSlot,
} from '@/hooks';
import { useUIStore } from '@/stores/uiStore';
import {
  Settings, Loader2, RefreshCw, AlertTriangle, X, RotateCcw,
  Play, ArrowLeftRight, Smartphone,
} from 'lucide-react';
import { ApnEditor } from './apn/ApnEditor';
import { PdpContextList } from './apn/PdpContextList';
import { ProfileManagerDialog } from './apn/ProfileManagerDialog';
import type { ApnProfile, DualSimInfo, SimSlotConfig, SimSlotStatus } from '@/types/api';

export function ConnectionPanel() {
  const modemId = useActiveModemId();
  const { data: apnProfiles = [] } = useApnProfiles({ modemId });
  const { data: activeModemProfile } = useActiveProfile();
  const { data: status } = useModemStatus();
  const isConnected = status?.connected ?? false;

  // Dual SIM
  const { data: dualSimInfo, refetch: refetchSimSlots } = useSimSlots();
  const updateSlotConfig = useUpdateSimSlotConfig();
  const switchSlot = useSwitchSimSlot();

  const [showProfileManager, setShowProfileManager] = useState(false);

  return (
    <div className="card p-6 h-full flex flex-col">
      {/* Header */}
      <div className="flex items-center justify-between mb-4">
        <div className="flex items-center gap-2">
          <Settings className="w-5 h-5 text-theme-accent" />
          <h2 className="text-lg font-medium text-theme-text-primary">
            Connection
          </h2>
          {dualSimInfo?.supported && (
            <span className="inline-flex items-center gap-1 px-2 py-0.5 rounded text-[10px] font-medium bg-theme-accent/10 text-theme-accent border border-theme-accent/20">
              <Smartphone className="w-3 h-3" />
              SIM {dualSimInfo.active_slot}
            </span>
          )}
        </div>
        <span className={`inline-flex items-center px-2 py-0.5 rounded text-xs font-medium ${
          isConnected
            ? 'bg-theme-success/15 text-theme-success'
            : 'bg-theme-bg-tertiary text-theme-text-secondary'
        }`}>
          {isConnected ? 'Connected' : 'Disconnected'}
        </span>
      </div>

      {/* Dual SIM Slot Section */}
      {(dualSimInfo?.supported || dualSimInfo?.dual_sim_disabled) && (
        <SimSlotSection
          info={dualSimInfo}
          profiles={apnProfiles}
          switchSlotMutation={switchSlot}
          updateConfigMutation={updateSlotConfig}
          onRefresh={refetchSimSlots}
          isPending={switchSlot.isPending}
        />
      )}

      {/* Active SIM connection settings label */}
      {dualSimInfo?.supported && (
        <div className="flex items-center gap-2 mb-3">
          <div className="flex-1 h-px bg-theme-border" />
          <span className="text-[10px] font-medium text-theme-text-muted uppercase tracking-wider">
            SIM {dualSimInfo.active_slot} Connection Settings
          </span>
          <div className="flex-1 h-px bg-theme-border" />
        </div>
      )}

      {modemId ? (
        <div className="flex-1 min-h-0 flex flex-col gap-5">
          {/* APN editor (form + Apply/Reconnect/Disconnect + Save/Manage) */}
          <ApnEditor
            modemId={modemId}
            onManageProfiles={() => setShowProfileManager(true)}
          />

          {/* PDP contexts — always visible, read-only */}
          <div className="pt-4 border-t border-theme-border">
            <PdpContextList />
          </div>
        </div>
      ) : (
        <div className="flex-1 flex items-center justify-center text-sm text-theme-text-muted text-center px-4 py-8">
          No active modem detected. Connect a modem to configure its APN settings.
        </div>
      )}

      {/* Profile Manager Dialog */}
      {showProfileManager && (
        <ProfileManagerDialog
          profiles={apnProfiles}
          modemProfileId={activeModemProfile?.profile?.profile_id ?? 'generic'}
          onClose={() => setShowProfileManager(false)}
        />
      )}
    </div>
  );
}

// =============================================================================
// Dual SIM Slot Section
// =============================================================================

interface SimSlotSectionProps {
  info: DualSimInfo;
  profiles: ApnProfile[];
  switchSlotMutation: ReturnType<typeof useSwitchSimSlot>;
  updateConfigMutation: ReturnType<typeof useUpdateSimSlotConfig>;
  onRefresh: () => void;
  isPending: boolean;
}

function SimSlotSection({
  info,
  profiles,
  switchSlotMutation,
  updateConfigMutation,
  onRefresh,
  isPending,
}: SimSlotSectionProps) {
  const theme = useUIStore((s) => s.theme);
  const [switchConfirm, setSwitchConfirm] = useState<{ slot: number; applyProfile: boolean } | null>(null);
  const [showSlotConfig, setShowSlotConfig] = useState(true);
  const isDisabled = !!info.dual_sim_disabled;
  const [toggling, setToggling] = useState(false);

  const handleToggle = () => {
    setToggling(true);
    updateConfigMutation.mutate({ dual_sim_disabled: !isDisabled }, {
      onSettled: () => {
        setToggling(false);
        onRefresh();
      },
    });
  };

  // Current slot config (derived from info)
  const activeSlot = info.active_slot;
  const inactiveSlot = activeSlot === 1 ? 2 : 1;
  const inactiveSlotInfo = info.slots.find(s => s.slot === inactiveSlot);

  const isSwitching = switchSlotMutation.isPending;
  const switchResult = switchSlotMutation.data;

  const handleSwitch = (applyProfile: boolean) => {
    setSwitchConfirm(null);
    switchSlotMutation.mutate({
      target_slot: inactiveSlot,
      apply_profile: applyProfile,
    }, {
      onSuccess: () => {
        setTimeout(() => onRefresh(), 1000);
      },
    });
  };

  const handleSlotProfileChange = (slot: number, profileId: string | undefined) => {
    const current: SimSlotConfig = {
      slot1_profile_id: info.slots.find(s => s.slot === 1)?.assigned_profile_id ?? undefined,
      slot2_profile_id: info.slots.find(s => s.slot === 2)?.assigned_profile_id ?? undefined,
    };
    if (slot === 1) current.slot1_profile_id = profileId;
    if (slot === 2) current.slot2_profile_id = profileId;
    updateConfigMutation.mutate(current, {
      onSuccess: () => onRefresh(),
    });
  };

  const simStateLabel = (slotInfo: SimSlotStatus | undefined) => {
    if (!slotInfo?.sim_status) return 'Unknown';
    const s = slotInfo.sim_status;
    if (!s.present) return 'No SIM';
    switch (s.state) {
      case 'ready': return 'Ready';
      case 'pin_required': return 'PIN Required';
      case 'puk_required': return 'PUK Required';
      case 'not_inserted': return 'No SIM';
      default: return s.state;
    }
  };

  return (
    <div className="mb-3 rounded-lg border border-theme-border bg-theme-bg-secondary overflow-hidden">
      {/* Header row */}
      <div className="flex items-center justify-between px-3 py-2 bg-theme-bg-tertiary">
        <div className="flex items-center gap-1.5">
          <Smartphone className="w-3.5 h-3.5 text-theme-accent" />
          <span className="text-xs font-medium text-theme-text-primary uppercase tracking-wide">
            Dual SIM
          </span>
        </div>
        <div className="flex items-center gap-1">
          {!isDisabled && (
            <>
              <button
                onClick={() => setShowSlotConfig(!showSlotConfig)}
                className="p-1 rounded text-theme-text-muted hover:text-theme-accent hover:bg-theme-bg-primary transition-colors"
                title="Configure slot profiles"
              >
                <Settings className="w-3 h-3" />
              </button>
              <button
                onClick={onRefresh}
                className="p-1 rounded text-theme-text-muted hover:text-theme-accent hover:bg-theme-bg-primary transition-colors"
                title="Refresh"
              >
                <RefreshCw className="w-3 h-3" />
              </button>
            </>
          )}
          <button
            onClick={handleToggle}
            disabled={toggling}
            className={`ml-0.5 relative inline-flex h-4 w-7 shrink-0 cursor-pointer rounded-full border-2 border-transparent transition-colors duration-200 ease-in-out focus:outline-none focus-visible:ring-2 focus-visible:ring-theme-accent focus-visible:ring-offset-1 focus-visible:ring-offset-theme-bg-primary disabled:opacity-50 ${
              isDisabled ? 'bg-theme-bg-primary' : 'bg-theme-accent'
            }`}
            title={isDisabled ? 'Enable Dual SIM' : 'Disable Dual SIM'}
          >
            <span
              className={`pointer-events-none inline-block h-3 w-3 transform rounded-full bg-white shadow ring-0 transition duration-200 ease-in-out ${
                isDisabled ? 'translate-x-0' : 'translate-x-3'
              }`}
            />
          </button>
        </div>
      </div>

      {/* Slot tabs — hidden when dual SIM is disabled */}
      {!isDisabled && <div className="px-3 py-2">
        <div className="flex items-center gap-2">
          {info.slots.map((slot) => {
            const isActive = slot.slot === activeSlot;
            return (
              <div
                key={slot.slot}
                className={`
                  flex-1 rounded-lg px-3 py-2 text-xs transition-colors
                  ${isActive
                    ? 'bg-theme-accent/10 border border-theme-accent/30'
                    : 'bg-theme-bg-primary border border-theme-border'
                  }
                `}
              >
                <div className="flex items-center justify-between">
                  <span className={`font-medium ${isActive ? 'text-theme-accent' : 'text-theme-text-secondary'}`}>
                    SIM {slot.slot}
                  </span>
                  {isActive && (
                    <span className="inline-flex items-center px-1.5 py-0.5 rounded text-[9px] font-medium bg-theme-success/20 text-theme-success">
                      Active
                    </span>
                  )}
                </div>
                <div className="mt-1 text-[10px] text-theme-text-muted">
                  {isActive ? simStateLabel(slot) : 'Standby'}
                </div>
                {slot.assigned_profile_name && (
                  <div className="mt-0.5 text-[10px] text-theme-text-secondary truncate">
                    {slot.assigned_profile_name}
                  </div>
                )}
              </div>
            );
          })}
        </div>

        {/* Switch buttons */}
        {inactiveSlotInfo?.assigned_profile_id && (
          <p className="mt-2 text-[9px] text-theme-text-muted leading-relaxed">
            <strong>Switch</strong> changes the active SIM only. <strong>Switch & Apply</strong> also configures APN/carrier settings and reboots.
          </p>
        )}
        <div className={`${inactiveSlotInfo?.assigned_profile_id ? 'mt-1.5' : 'mt-2'} flex items-center gap-2`}>
          <button
            onClick={() => setSwitchConfirm({ slot: inactiveSlot, applyProfile: false })}
            disabled={isSwitching || isPending}
            title={`Quick switch to SIM ${inactiveSlot} without changing APN or carrier settings. The new SIM will use whatever settings are already on the modem.`}
            className={`flex-1 flex items-center justify-center gap-1.5 px-3 py-1.5
                       text-xs font-medium rounded-lg transition-colors
                       disabled:opacity-40 disabled:cursor-not-allowed
                       border border-theme-border text-theme-text-secondary
                       hover:bg-theme-bg-tertiary hover:text-theme-text-primary`}
          >
            {isSwitching ? (
              <Loader2 className="w-3.5 h-3.5 animate-spin" />
            ) : (
              <ArrowLeftRight className="w-3.5 h-3.5" />
            )}
            Switch to SIM {inactiveSlot}
          </button>
          {inactiveSlotInfo?.assigned_profile_id && (
            <button
              onClick={() => setSwitchConfirm({ slot: inactiveSlot, applyProfile: true })}
              disabled={isSwitching || isPending}
              title={`Switch to SIM ${inactiveSlot} and apply its assigned APN profile and carrier settings. This will reconfigure the modem and reboot it to fully activate the new SIM's settings.`}
              className={`flex-1 flex items-center justify-center gap-1.5 px-3 py-1.5
                         text-xs font-medium rounded-lg transition-colors
                         disabled:opacity-40 disabled:cursor-not-allowed
                         ${theme === 'fallen'
                           ? 'border border-theme-accent text-theme-accent hover:bg-theme-accent-muted'
                           : 'bg-theme-accent hover:opacity-90 text-white'
                         }`}
            >
              {isSwitching ? (
                <Loader2 className="w-3.5 h-3.5 animate-spin" />
              ) : (
                <Play className="w-3.5 h-3.5" />
              )}
              Switch & Apply
            </button>
          )}
        </div>
      </div>}

      {/* Switch confirmation */}
      {!isDisabled && switchConfirm && (
        <div className="mx-3 mb-2 rounded-lg border p-2.5 space-y-2 bg-theme-warning/10 border-theme-warning/30">
          <div className="flex items-start gap-2">
            <AlertTriangle className="w-3.5 h-3.5 mt-0.5 shrink-0 text-theme-warning" />
            <div className="flex-1 text-xs">
              <p className="font-medium text-theme-text-primary">
                Switch to SIM {switchConfirm.slot}?
              </p>
              <p className="text-theme-text-secondary mt-1 text-[10px] leading-relaxed">
                {switchConfirm.applyProfile
                  ? <>This will switch the SIM slot, apply the assigned APN profile, and <strong>reboot the modem</strong>. The modem will reconnect automatically.</>
                  : <>This will switch the active SIM slot immediately (no reboot). The new SIM will need a moment to initialize.</>
                }
              </p>
            </div>
            <button
              onClick={() => setSwitchConfirm(null)}
              className="p-0.5 text-theme-text-muted hover:text-theme-text-primary"
            >
              <X className="w-3 h-3" />
            </button>
          </div>
          <div className="flex items-center gap-2 justify-end">
            <button
              onClick={() => setSwitchConfirm(null)}
              className="px-2.5 py-1 rounded-md text-[10px] font-medium text-theme-text-secondary hover:bg-theme-bg-tertiary transition-colors"
            >
              Cancel
            </button>
            <button
              onClick={() => handleSwitch(switchConfirm.applyProfile)}
              disabled={isSwitching}
              className="flex items-center gap-1 px-2.5 py-1 rounded-md text-[10px] font-medium text-white bg-theme-accent hover:opacity-90 transition-colors disabled:opacity-50"
            >
              {isSwitching ? (
                <Loader2 className="w-2.5 h-2.5 animate-spin" />
              ) : switchConfirm.applyProfile ? (
                <RotateCcw className="w-2.5 h-2.5" />
              ) : (
                <ArrowLeftRight className="w-2.5 h-2.5" />
              )}
              {switchConfirm.applyProfile ? 'Switch & Reboot' : 'Switch'}
            </button>
          </div>
        </div>
      )}

      {/* Switch result feedback */}
      {!isDisabled && switchResult && !isSwitching && (
        <div className={`mx-3 mb-2 p-2 rounded-lg text-[10px] ${
          switchResult.success
            ? 'bg-theme-success/10 text-theme-success'
            : 'bg-theme-error/10 text-theme-error'
        }`}>
          {switchResult.message}
        </div>
      )}

      {/* Slot profile config (expandable) */}
      {!isDisabled && showSlotConfig && (
        <div className="border-t border-theme-border px-3 py-2 space-y-2">
          <span className="text-[10px] font-medium text-theme-text-secondary uppercase tracking-wide">
            Slot Profile Assignments
          </span>
          {[1, 2].map((slotNum) => {
            const slotInfo = info.slots.find(s => s.slot === slotNum);
            return (
              <div key={slotNum} className="flex items-center gap-2">
                <span className="text-xs text-theme-text-secondary w-12 shrink-0">
                  SIM {slotNum}
                </span>
                <select
                  value={slotInfo?.assigned_profile_id ?? ''}
                  onChange={(e) => handleSlotProfileChange(slotNum, e.target.value || undefined)}
                  disabled={updateConfigMutation.isPending}
                  className="select-compact flex-1"
                >
                  <option value="">-- None --</option>
                  {profiles.map((p) => (
                    <option key={p.id} value={p.id}>{p.name}</option>
                  ))}
                </select>
              </div>
            );
          })}
          {updateConfigMutation.isPending && (
            <div className="flex items-center gap-1 text-[10px] text-theme-text-muted">
              <Loader2 className="w-3 h-3 animate-spin" />
              Saving...
            </div>
          )}
        </div>
      )}
    </div>
  );
}
