/**
 * ActiveModemLabel Component
 *
 * Compact header label showing the active modem's name.
 * Doubles as the modem selector dropdown — click to switch modems or rescan USB.
 */

import { ChevronDown, Search, RefreshCw } from 'lucide-react';
import { useState, useRef, useEffect } from 'react';
import { useActiveProfile, useDetectedModems, useSelectModem, useRescanModems, useDiscoverModem } from '@/hooks/queries';
import { useCurrentUser } from '@/contexts/UserContext';
import type { DetectedModemEnhanced } from '@/types/profiles';

export function ActiveModemLabel() {
  const { data: activeProfile } = useActiveProfile();
  const { data: modems } = useDetectedModems();
  const selectMutation = useSelectModem();
  const rescanMutation = useRescanModems();
  const discoverMutation = useDiscoverModem();
  const currentUser = useCurrentUser();
  // null currentUser = auth-disabled (full-access deployment) → allowed.
  // Only an authenticated non-admin is restricted (mirrors backend require_admin 403).
  const canManageModems =
    currentUser == null ||
    currentUser.role === 'admin' ||
    currentUser.role === 'super_admin';
  const [isOpen, setIsOpen] = useState(false);
  const [discoveryDone, setDiscoveryDone] = useState<Set<string>>(new Set());
  const dropdownRef = useRef<HTMLDivElement>(null);

  // Close dropdown on outside click
  useEffect(() => {
    if (!isOpen) return;
    function handleClickOutside(e: MouseEvent) {
      if (dropdownRef.current && !dropdownRef.current.contains(e.target as Node)) {
        setIsOpen(false);
      }
    }
    document.addEventListener('mousedown', handleClickOutside);
    return () => document.removeEventListener('mousedown', handleClickOutside);
  }, [isOpen]);

  const profile = activeProfile?.profile;
  const modemCount = modems?.length ?? 0;
  const hasMultiple = modemCount > 1;

  // Determine display name
  const displayName = profile
    ? profile.is_generic
      ? activeProfile?.detected?.vendor_id && activeProfile?.detected?.product_id
        ? `Unknown [${activeProfile.detected.vendor_id}:${activeProfile.detected.product_id}]`
        : 'Unknown Modem'
      : `${profile.manufacturer} ${profile.model}`
    : 'No Modem';

  const handleSelect = (modemId: string) => {
    selectMutation.mutate(modemId);
    setIsOpen(false);
  };

  const handleRescan = (e: React.MouseEvent) => {
    e.stopPropagation();
    if (rescanMutation.isPending) return;
    rescanMutation.mutate();
  };

  const handleDiscover = (e: React.MouseEvent, modem: DetectedModemEnhanced) => {
    e.stopPropagation();
    if (discoverMutation.isPending) return;

    discoverMutation.mutate(undefined, {
      onSuccess: () => {
        const key = `${modem.vendor_id}:${modem.product_id}`;
        setDiscoveryDone(prev => new Set(prev).add(key));
      },
    });
  };

  return (
    <div ref={dropdownRef} className="relative">
      <button
        onClick={() => setIsOpen(!isOpen)}
        disabled={selectMutation.isPending}
        className="flex items-center gap-1 sm:gap-1.5 px-2 sm:px-2.5 py-1 sm:py-1.5 rounded-full text-caption sm:text-xs font-medium bg-theme-bg-tertiary text-theme-text-secondary hover:text-theme-text-primary hover:bg-theme-bg-hover transition-colors disabled:opacity-50 cursor-pointer max-w-[120px] sm:max-w-[200px]"
        title={displayName}
      >
        <span className="truncate">
          {selectMutation.isPending ? 'Switching...' : displayName}
        </span>
        {hasMultiple && modems && activeProfile && (
          <span className="shrink-0 text-[10px] opacity-70">
            {`${(modems.findIndex(m => m.modem_id === activeProfile.modem_id) + 1) || '?'}/${modemCount}`}
          </span>
        )}
        <ChevronDown className="w-3 h-3 shrink-0" />
      </button>

      {isOpen && (
        <div className="absolute right-0 top-full mt-1 w-64 sm:w-72 bg-theme-bg-popover border border-theme-border rounded-lg shadow-lg z-50 overflow-hidden">
          {/* Modem list header */}
          <div className="px-3 py-1.5 text-[10px] font-semibold uppercase tracking-wider text-theme-text-muted border-b border-theme-border-light">
            {modemCount} {modemCount === 1 ? 'Modem' : 'Modems'} Detected
          </div>

          {modems && modems.map((modem: DetectedModemEnhanced) => {
            const modemKey = `${modem.vendor_id}:${modem.product_id}`;
            const isDiscovered = discoveryDone.has(modemKey);
            const canDiscover = !modem.has_profile && modem.vendor_id && modem.product_id;
            const isActive = activeProfile?.detected?.device_path === modem.device_path;

            const rowContent = (
              <>
                <div className="flex items-center justify-between">
                  <div className="flex items-center gap-2">
                    {isActive && (
                      <span className="w-1.5 h-1.5 rounded-full bg-theme-success shrink-0" />
                    )}
                    <span className="text-sm font-medium text-theme-text-primary">
                      {modem.description}
                    </span>
                  </div>
                  {modem.has_profile ? (
                    <span className="text-[10px] px-1.5 py-0.5 rounded bg-theme-success/20 text-theme-success font-medium">
                      Profile
                    </span>
                  ) : (
                    <span className="text-[10px] px-1.5 py-0.5 rounded bg-theme-warning/20 text-theme-warning font-medium">
                      Generic
                    </span>
                  )}
                </div>
                <div className={`text-xs text-theme-text-muted mt-0.5 ${isActive ? 'ml-3.5' : ''}`}>
                  {modem.device_path}
                  {modem.vendor_id && modem.product_id && (
                    <span className="ml-2 font-mono">[{modem.vendor_id}:{modem.product_id}]</span>
                  )}
                </div>
              </>
            );

            return (
              <div key={modem.device_path} className="border-b border-theme-border-light last:border-0">
                {canManageModems ? (
                  <button
                    onClick={() => handleSelect(modem.modem_id)}
                    className={`w-full text-left px-3 py-2.5 hover:bg-theme-bg-secondary transition-colors ${isActive ? 'bg-theme-bg-secondary' : ''}`}
                  >
                    {rowContent}
                  </button>
                ) : (
                  <div className={`px-3 py-2.5 ${isActive ? 'bg-theme-bg-secondary' : ''}`}>
                    {rowContent}
                  </div>
                )}

                {canManageModems && canDiscover && (
                  <div className="px-3 pb-2">
                    <button
                      onClick={(e) => handleDiscover(e, modem)}
                      disabled={discoverMutation.isPending}
                      className="flex items-center gap-1 text-caption px-2 py-1 rounded bg-theme-accent/15 text-theme-accent hover:bg-theme-accent/25 transition-colors disabled:opacity-50"
                    >
                      <Search className="w-3 h-3" />
                      {discoverMutation.isPending
                        ? 'Discovering...'
                        : isDiscovered
                          ? 'Discovery Saved'
                          : 'Run Discovery'}
                    </button>
                  </div>
                )}
              </div>
            );
          })}

          {/* Rescan button (admin only) */}
          {canManageModems ? (
            <div className="border-t border-theme-border px-3 py-2">
              <button
                onClick={handleRescan}
                disabled={rescanMutation.isPending}
                className="flex items-center gap-1.5 w-full text-caption px-2 py-1.5 rounded bg-theme-bg-tertiary hover:bg-theme-accent-muted text-theme-text-muted hover:text-theme-text-accent transition-colors disabled:opacity-50"
              >
                <RefreshCw className={`w-3 h-3 ${rescanMutation.isPending ? 'animate-spin' : ''}`} />
                {rescanMutation.isPending ? 'Scanning USB...' : 'Rescan USB'}
              </button>
              {rescanMutation.isError && (
                <p className="text-[10px] text-theme-text-muted mt-1 px-2">
                  {rescanMutation.error?.message}
                </p>
              )}
            </div>
          ) : (
            <div className="border-t border-theme-border px-3 py-2">
              <p className="text-[10px] text-theme-text-muted px-2">
                Switching modems requires admin.
              </p>
            </div>
          )}
        </div>
      )}
    </div>
  );
}
