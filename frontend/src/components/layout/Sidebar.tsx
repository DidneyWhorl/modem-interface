/**
 * Sidebar Component
 *
 * Collapsible sidebar for:
 * - Toggling panel visibility
 * - Quick access to all panels
 * - Settings popover (theme, grid lines, auto-compact, reset layout, version)
 *
 * Collapsed by default on mobile, expanded on desktop/tablet.
 * Auth controls (user info, change password, sign out) removed — will move to header in Phase 3.
 */

import { useEffect, useRef, useState } from 'react';
import { Link, useLocation } from 'react-router-dom';
import {
  Signal,
  Wifi,
  Settings,
  Smartphone,
  CreditCard,
  Terminal,
  Download,
  MapPin,
  Antenna,
  PanelLeftClose,
  PanelLeft,
  RotateCcw,
  Eye,
  EyeOff,
  Grid3X3,
  ArrowDownUp,
  Radio,
  Bug,
  Network,
  TrendingUp,
  ExternalLink,
  GripVertical,
} from 'lucide-react';
import { useUIStore, type PanelId, type PanelConfig, PANEL_CONFIGS } from '@/stores/uiStore';
import { ThemeSwitcher } from '@/components/ui/ThemeSwitcher';
import { UserProfileButton } from '@/components/ui/UserProfileButton';
import { TelemetryToggle } from '@/components/settings/TelemetryToggle';
import { TelemetryPolling } from '@/components/settings/TelemetryPolling';
import { TunnelConfig } from '@/components/settings/TunnelConfig';
import { LicenseSettings } from '@/components/settings/LicenseSettings';
import type { LicenseStatus } from '@/types/api';
import clsx from 'clsx';

// Map icon names to components
const ICON_MAP: Record<string, React.ElementType> = {
  Signal,
  Wifi,
  Settings,
  Smartphone,
  CreditCard,
  Terminal,
  Download,
  MapPin,
  Antenna,
  Radio,
  Bug,
  Network,
  TrendingUp,
};

// Mobile breakpoint (matches Tailwind sm:)
const MOBILE_BREAKPOINT = 640;

interface SidebarProps {
  user?: { username?: string; role?: string; allowedPanels?: string[] | null } | null;
  onLogout?: () => void;
  licenseInfo?: LicenseStatus | null;
}

export function Sidebar({ user, onLogout, licenseInfo }: SidebarProps) {
  const {
    visiblePanels,
    togglePanelVisibility,
    resetPanelLayout,
    sidebarCollapsed,
    toggleSidebarCollapsed,
    showGridLines,
    toggleGridLines,
    autoCompact,
    toggleAutoCompact,
    viewMode,
    focusedPanel,
    setFocusedPanel,
    panelOrder,
    setPanelOrder,
    sidebarReorderUnlocked,
    toggleSidebarReorder,
  } = useUIStore();

  const initializedRef = useRef(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const settingsBtnRef = useRef<HTMLButtonElement>(null);

  // Drag-and-drop state for sidebar reordering
  const [dragIndex, setDragIndex] = useState<number | null>(null);
  const [dragOverIndex, setDragOverIndex] = useState<number | null>(null);

  // Set initial sidebar state based on screen size (only once on mount)
  useEffect(() => {
    if (initializedRef.current) return;
    initializedRef.current = true;

    const isMobile = window.innerWidth < MOBILE_BREAKPOINT;
    const store = useUIStore.getState();

    // On mobile, collapse sidebar. On desktop/tablet, expand it.
    if (isMobile && !store.sidebarCollapsed) {
      toggleSidebarCollapsed();
    } else if (!isMobile && store.sidebarCollapsed) {
      toggleSidebarCollapsed();
    }
  }, [toggleSidebarCollapsed]);

  // Close settings modal on Escape key
  useEffect(() => {
    if (!settingsOpen) return;
    const handler = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setSettingsOpen(false);
    };
    document.addEventListener('keydown', handler);
    return () => document.removeEventListener('keydown', handler);
  }, [settingsOpen]);

  const location = useLocation();
  const isWanActive = location.pathname === '/wan-manager';
  const allowedPanels = user?.allowedPanels ?? null; // null = unrestricted

  // Build ordered panel list using panelOrder, filtering by allowed panels
  const orderedPanelConfigs = (() => {
    const configMap = new Map(PANEL_CONFIGS.map(p => [p.id, p]));
    // Start with panelOrder, then append any new panels not yet in the order
    const allIds = PANEL_CONFIGS.filter(p => p.id !== 'wan-manager').map(p => p.id);
    const ordered = [
      ...panelOrder.filter(id => allIds.includes(id)),
      ...allIds.filter(id => !panelOrder.includes(id)),
    ];
    return ordered
      .map(id => configMap.get(id))
      .filter((p): p is PanelConfig => {
        if (!p || p.id === 'wan-manager') return false;
        if (allowedPanels && !allowedPanels.includes(p.id)) return false;
        return true;
      });
  })();

  const isVisible = (id: PanelId) => visiblePanels.includes(id);

  return (
    <div
      className={clsx(
        'fixed left-2 top-2 z-40 flex flex-col',
        'bg-theme-bg-secondary rounded-2xl border-2 border-theme-border',
        'transition-all duration-300',
        sidebarCollapsed ? 'w-14' : 'w-56'
      )}
      style={{ height: 'calc(100vh - 1rem)' }}
    >
      {/* Header — user profile IS the header */}
      <div className="border-b border-theme-border">
        {sidebarCollapsed ? (
          /* Collapsed: user icon centered + collapse button below */
          <div className="flex flex-col items-center gap-1 py-2">
            {user && onLogout && (
              <UserProfileButton
                user={{ username: user.username || 'User', role: user.role || 'admin' }}
                onLogout={onLogout}
                showUsername={false}
                licenseInfo={licenseInfo}
              />
            )}
            <button
              onClick={toggleSidebarCollapsed}
              className="p-2.5 sm:p-1.5 min-w-[44px] min-h-[44px] sm:min-w-0 sm:min-h-0 flex items-center justify-center text-theme-text-secondary hover:text-theme-text-primary rounded hover:bg-theme-bg-tertiary focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-theme-accent"
              title="Expand sidebar"
            >
              <PanelLeft className="w-5 h-5" />
            </button>
          </div>
        ) : (
          /* Expanded: user profile row + collapse button */
          <div className="flex items-center gap-2 px-2 py-2">
            <div className="flex-1 min-w-0">
              {user && onLogout && (
                <UserProfileButton
                  user={{ username: user.username || 'User', role: user.role || 'admin' }}
                  onLogout={onLogout}
                  showUsername
                  licenseInfo={licenseInfo}
                />
              )}
            </div>
            <button
              onClick={toggleSidebarCollapsed}
              className="p-2.5 sm:p-1.5 min-w-[44px] min-h-[44px] sm:min-w-0 sm:min-h-0 flex items-center justify-center text-theme-text-secondary hover:text-theme-text-primary rounded hover:bg-theme-bg-tertiary shrink-0 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-theme-accent"
              title="Collapse sidebar"
            >
              <PanelLeftClose className="w-5 h-5" />
            </button>
          </div>
        )}
      </div>

      {/* Panel List */}
      <div className="flex-1 overflow-y-auto py-1">
        {/* Section header */}
        {!sidebarCollapsed && (
          <div className="text-[10px] uppercase tracking-wider text-theme-text-muted font-medium px-3 py-1.5 mt-1">
            Panels
          </div>
        )}
        {orderedPanelConfigs.map((panel, index) => {
          const Icon = ICON_MAP[panel.icon] || Signal;
          const visible = isVisible(panel.id);
          const isFocused = viewMode === 'focus' && focusedPanel === panel.id;
          const isDragging = dragIndex === index;
          const isDragOver = dragOverIndex === index;

          return (
            <div
              key={panel.id}
              draggable={sidebarReorderUnlocked && !sidebarCollapsed}
              onDragStart={(e) => {
                setDragIndex(index);
                e.dataTransfer.effectAllowed = 'move';
              }}
              onDragOver={(e) => {
                e.preventDefault();
                e.dataTransfer.dropEffect = 'move';
                setDragOverIndex(index);
              }}
              onDrop={(e) => {
                e.preventDefault();
                if (dragIndex !== null && dragIndex !== index) {
                  const ids = orderedPanelConfigs.map(p => p.id);
                  const [moved] = ids.splice(dragIndex, 1);
                  if (moved) {
                    ids.splice(index, 0, moved);
                    setPanelOrder(ids);
                  }
                }
                setDragIndex(null);
                setDragOverIndex(null);
              }}
              onDragEnd={() => {
                setDragIndex(null);
                setDragOverIndex(null);
              }}
              className={clsx(
                isDragging && 'opacity-40',
                isDragOver && dragIndex !== null && dragIndex !== index && (
                  dragIndex < index
                    ? 'border-b-2 border-theme-accent'
                    : 'border-t-2 border-theme-accent'
                )
              )}
            >
              <button
                onClick={() => {
                  if (sidebarReorderUnlocked) return; // Suppress clicks during reorder
                  if (viewMode === 'focus') {
                    setFocusedPanel(panel.id);
                  } else {
                    togglePanelVisibility(panel.id);
                  }
                }}
                className={clsx(
                  'w-full flex items-center py-1.5',
                  'hover:bg-theme-bg-tertiary/50 transition-colors',
                  sidebarCollapsed ? 'justify-center px-1' : 'gap-3 px-3',
                  sidebarReorderUnlocked && !sidebarCollapsed && 'cursor-grab active:cursor-grabbing',
                  viewMode === 'focus'
                    ? isFocused
                      ? 'text-theme-text-primary bg-theme-accent/15'
                      : 'text-theme-text-secondary'
                    : visible ? 'text-theme-text-primary' : 'text-theme-text-muted'
                )}
                title={sidebarCollapsed ? panel.title : undefined}
              >
                {/* Drag handle — only when reorder unlocked and expanded */}
                {sidebarReorderUnlocked && !sidebarCollapsed && (
                  <GripVertical className="w-3.5 h-3.5 text-theme-text-muted shrink-0" />
                )}

                <div className={clsx(
                  'flex items-center justify-center rounded-lg shrink-0',
                  sidebarCollapsed ? 'w-10 h-10' : 'w-8 h-8',
                  viewMode === 'focus'
                    ? isFocused
                      ? 'bg-theme-accent-muted text-theme-text-accent'
                      : 'bg-theme-bg-tertiary/50 text-theme-text-secondary'
                    : visible
                      ? 'bg-theme-accent-muted text-theme-text-accent'
                      : 'bg-theme-bg-tertiary/50 text-theme-text-muted'
                )}>
                  <Icon className={sidebarCollapsed ? 'w-6 h-6' : 'w-4 h-4'} />
                </div>

                {!sidebarCollapsed && (
                  <>
                    <span className="flex-1 text-left text-sm font-medium truncate">
                      {panel.title}
                    </span>
                    {!sidebarReorderUnlocked && viewMode === 'dashboard' && (
                      visible ? (
                        <Eye className="w-4 h-4 text-theme-success" />
                      ) : (
                        <EyeOff className="w-4 h-4 text-theme-text-muted" />
                      )
                    )}
                    {!sidebarReorderUnlocked && viewMode === 'focus' && isFocused && (
                      <div className="w-1.5 h-1.5 rounded-full bg-theme-accent shrink-0" />
                    )}
                  </>
                )}
              </button>
            </div>
          );
        })}
      </div>

      {/* Navigation Links */}
      <div className="border-t border-theme-border py-1">
        {!sidebarCollapsed && (
          <div className="text-[10px] uppercase tracking-wider text-theme-text-muted font-medium px-3 py-1.5">
            Navigation
          </div>
        )}
        <Link
          to="/wan-manager"
          className={clsx(
            'w-full flex items-center py-1.5',
            'hover:bg-theme-bg-tertiary/50 transition-colors',
            sidebarCollapsed ? 'justify-center px-1' : 'gap-3 px-3',
            isWanActive
              ? 'text-theme-text-primary bg-theme-bg-tertiary/60'
              : 'text-theme-text-secondary hover:text-theme-text-primary'
          )}
          title={sidebarCollapsed ? 'CTRL-WAN' : undefined}
        >
          <div className={clsx(
            'flex items-center justify-center rounded-lg shrink-0',
            sidebarCollapsed ? 'w-10 h-10' : 'w-8 h-8',
            isWanActive
              ? 'bg-theme-accent-muted text-theme-text-accent'
              : 'bg-theme-bg-tertiary/50 text-theme-text-secondary'
          )}>
            <Network className={sidebarCollapsed ? 'w-6 h-6' : 'w-4 h-4'} />
          </div>
          {!sidebarCollapsed && (
            <>
              <span className="flex-1 text-left text-sm font-medium truncate">
                CTRL-WAN
              </span>
              <ExternalLink className="w-3.5 h-3.5 text-theme-text-muted" />
            </>
          )}
        </Link>
      </div>

      {/* Footer - Settings Button */}
      <div className="border-t border-theme-border p-2 relative">
        <button
          ref={settingsBtnRef}
          onClick={() => setSettingsOpen(!settingsOpen)}
          className={clsx(
            'w-full flex items-center gap-2 px-3 py-2.5 sm:py-2 min-h-[44px] sm:min-h-0',
            'rounded-lg transition-colors text-sm',
            settingsOpen
              ? 'text-theme-text-accent bg-theme-accent-muted'
              : 'text-theme-text-secondary hover:text-theme-text-primary hover:bg-theme-bg-tertiary/50',
            sidebarCollapsed && 'justify-center'
          )}
          title="View Settings"
        >
          <Settings className={sidebarCollapsed ? 'w-6 h-6' : 'w-4 h-4'} />
          {!sidebarCollapsed && <span>View Settings</span>}
        </button>

        {/* Settings Modal */}
        {settingsOpen && (
          <>
            {/* Backdrop */}
            <div
              className="fixed inset-0 bg-black/40 z-50"
              onClick={() => setSettingsOpen(false)}
            />
            {/* Modal */}
            <div className="fixed inset-0 z-50 flex items-center justify-center pointer-events-none">
              <div
                className="w-80 max-h-[90vh] overflow-y-auto bg-theme-bg-popover border border-theme-border rounded-2xl shadow-lg p-5 pointer-events-auto"
                onClick={(e) => e.stopPropagation()}
              >
            {/* Header */}
            <div className="text-base font-medium text-theme-text-primary mb-3 px-1">
              View Settings
            </div>

            {/* Theme Switcher */}
            <ThemeSwitcher alwaysExpanded />

            {/* Auto-Compact Toggle */}
            <button
              onClick={toggleAutoCompact}
              className={clsx(
                'w-full flex items-center gap-2 px-3 py-2.5 sm:py-2 min-h-[44px] sm:min-h-0',
                'rounded-lg transition-colors text-sm',
                'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-theme-accent focus-visible:ring-offset-1',
                autoCompact
                  ? 'text-theme-success bg-theme-success/20 hover:bg-theme-success/30'
                  : 'text-theme-text-secondary hover:text-theme-text-primary hover:bg-theme-bg-tertiary/50'
              )}
              title={autoCompact ? 'Disable auto-sort' : 'Enable auto-sort (panels stack vertically)'}
            >
              <ArrowDownUp className="w-4 h-4" />
              <span>{autoCompact ? 'Auto-Sort: On' : 'Auto-Sort: Off'}</span>
            </button>

            {/* Grid Lines Toggle */}
            <button
              onClick={toggleGridLines}
              className={clsx(
                'w-full flex items-center gap-2 px-3 py-2.5 sm:py-2 min-h-[44px] sm:min-h-0',
                'rounded-lg transition-colors text-sm',
                'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-theme-accent focus-visible:ring-offset-1',
                showGridLines
                  ? 'text-theme-text-accent bg-theme-accent-muted hover:bg-theme-accent-muted'
                  : 'text-theme-text-secondary hover:text-theme-text-primary hover:bg-theme-bg-tertiary/50'
              )}
              title={showGridLines ? 'Hide grid lines' : 'Show grid lines'}
            >
              <Grid3X3 className="w-4 h-4" />
              <span>{showGridLines ? 'Grid: On' : 'Grid: Off'}</span>
            </button>

            {/* Reorder Panels Toggle */}
            <button
              onClick={toggleSidebarReorder}
              className={clsx(
                'w-full flex items-center gap-2 px-3 py-2.5 sm:py-2 min-h-[44px] sm:min-h-0',
                'rounded-lg transition-colors text-sm',
                'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-theme-accent focus-visible:ring-offset-1',
                sidebarReorderUnlocked
                  ? 'text-theme-text-accent bg-theme-accent-muted hover:bg-theme-accent-muted'
                  : 'text-theme-text-secondary hover:text-theme-text-primary hover:bg-theme-bg-tertiary/50'
              )}
              title={sidebarReorderUnlocked ? 'Lock panel order' : 'Unlock to reorder panels'}
            >
              <GripVertical className="w-4 h-4" />
              <span>{sidebarReorderUnlocked ? 'Reorder: Unlocked' : 'Reorder Panels'}</span>
            </button>

            {/* Divider */}
            <div className="border-t border-theme-border my-2" />

            {/* Reset Layout */}
            <button
              onClick={() => {
                resetPanelLayout();
                setSettingsOpen(false);
              }}
              className={clsx(
                'w-full flex items-center gap-2 px-3 py-2.5 sm:py-2 min-h-[44px] sm:min-h-0',
                'text-theme-text-secondary hover:text-theme-text-primary hover:bg-theme-bg-tertiary/50 rounded-lg',
                'transition-colors text-sm',
                'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-theme-accent focus-visible:ring-offset-1'
              )}
              title="Reset to default layout"
            >
              <RotateCcw className="w-4 h-4" />
              <span>Reset Layout</span>
            </button>

            {/* Telemetry Toggle (licensed devices only) */}
            <TelemetryToggle licenseInfo={licenseInfo} />

            {/* Telemetry Polling Controls (licensed + telemetry enabled) */}
            <TelemetryPolling licensed={licenseInfo?.state === 'valid'} />

            {/* Remote Access Tunnel */}
            <TunnelConfig licenseInfo={licenseInfo} />

            {/* Divider */}
            <div className="border-t border-theme-border my-2" />

            {/* Cloud License (opt-in — never gates the dashboard) */}
            <LicenseSettings licenseInfo={licenseInfo} />

            {/* Divider */}
            <div className="border-t border-theme-border my-2" />

            {/* Version Info */}
            <div className="px-1">
              <p className="text-[10px] text-theme-text-muted leading-relaxed">
                v{__APP_VERSION__} ({__GIT_HASH__})<br />
                Built {new Date(__BUILD_TIME__).toLocaleString()}
              </p>
            </div>
              </div>
            </div>
          </>
        )}
      </div>
    </div>
  );
}
