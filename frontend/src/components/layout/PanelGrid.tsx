/**
 * PanelGrid Component
 * 
 * Main layout component using react-grid-layout for:
 * - Free placement of panels anywhere on grid
 * - Drag and drop to any open space
 * - Resize panels
 * - Collapse/expand panels (tap header on mobile)
 * - Auto-compact option
 * - Optional grid lines overlay
 * - Responsive breakpoints
 * - Persisted layouts
 */

import { useState, useCallback, useMemo, useEffect, useRef } from 'react';
import { Responsive, WidthProvider, Layout } from 'react-grid-layout';
import { useQueryClient } from '@tanstack/react-query';
import { useUIStore, type PanelId, PANEL_CONFIGS } from '@/stores/uiStore';
import { useCurrentUser } from '@/contexts/UserContext';
import { X, Minus, Plus, GripHorizontal, ChevronDown, ChevronRight, RefreshCw } from 'lucide-react';
import { ModemSubtext } from '@/components/ui/ModemSubtext';
import {
  signalQueryKey, extendedSignalQueryKey,
  modemStatusQueryKey, deviceInfoQueryKey,
  simStatusQueryKey, gpsQueryKey,
  antennaMetricsQueryKey, configQueryKey,
  bandConfigQueryKey,
  simSlotsQueryKey, wanStatusQueryKey,
  signalHistoryQueryKey,
  speedtestHistoryQueryKey,
  useActiveModemId,
} from '@/hooks';
import {
  refreshSignal, refreshDevice,
  refreshSim, refreshGps,
} from '@/api/modem';

// Import CSS for react-grid-layout
import 'react-grid-layout/css/styles.css';
import 'react-resizable/css/styles.css';

// Panel components
import { ConnectionInfo } from '@/components/modem/ConnectionInfo';
import { ConnectionPanel } from '@/components/modem/ConnectionPanel';
import { DeviceInfo } from '@/components/modem/DeviceInfo';
import { SimCard } from '@/components/sim/SimCard';
import { ATTerminal } from '@/components/terminal/ATTerminal';
import { UpdatePanel } from '@/components/system/UpdatePanel';
import { GpsPanel } from '@/components/modem/GpsPanel';
import { AntennaMetrics } from '@/components/modem/AntennaMetrics';
import { BandLockPanel } from '@/components/modem/BandLockPanel';
import { DebugPanel } from '@/components/modem/DebugPanel';
import { WanManagerPanel } from '@/components/modem/WanManagerPanel';
import { SignalTrending } from '@/components/modem/SignalTrending';
import { SpeedtestPanel } from '@/components/modem/SpeedtestPanel';

const ResponsiveGridLayout = WidthProvider(Responsive);

// Map panel IDs to components
const PANEL_COMPONENTS: Record<PanelId, React.ComponentType> = {
  'connection-info': ConnectionInfo,
  'connection-panel': ConnectionPanel,
  'device-info': DeviceInfo,
  'sim-card': SimCard,
  'at-terminal': ATTerminal,
  'system-update': UpdatePanel,
  'gps': GpsPanel,
  'antenna-metrics': AntennaMetrics,
  'band-lock': BandLockPanel,
  'debug-log': DebugPanel,
  'wan-manager': WanManagerPanel,
  'signal-trending': SignalTrending,
  'speedtest': SpeedtestPanel,
};

// Map panel IDs to their associated react-query keys for fallback refresh (invalidation)
const PANEL_QUERY_KEYS: Partial<Record<PanelId, readonly (readonly string[])[]>> = {
  'connection-info': [signalQueryKey, extendedSignalQueryKey, modemStatusQueryKey],
  'connection-panel': [modemStatusQueryKey, configQueryKey, simSlotsQueryKey, ['modem', 'pdp'] as const],
  'device-info': [deviceInfoQueryKey],
  'sim-card': [simStatusQueryKey],
  'gps': [gpsQueryKey],
  'antenna-metrics': [antennaMetricsQueryKey],
  'wan-manager': [wanStatusQueryKey],
  'signal-trending': [signalHistoryQueryKey('1h')],
  'speedtest': [speedtestHistoryQueryKey],
};

// Panels with on-demand POST refresh endpoints — bypass 60s cache
const PANEL_REFRESH: Partial<Record<PanelId, {
  fn: (modemId: string) => Promise<unknown>;
  queryKey: readonly string[];
}>> = {
  'connection-info': { fn: refreshSignal, queryKey: signalQueryKey },
  'device-info': { fn: refreshDevice, queryKey: deviceInfoQueryKey },
  'sim-card': { fn: refreshSim, queryKey: simStatusQueryKey },
  'gps': { fn: refreshGps, queryKey: gpsQueryKey },
};

// Grid configuration
const GRID_CONFIG = {
  rowHeight: 80,
  margin: [12, 12] as [number, number],
  containerPadding: [12, 12] as [number, number],
  cols: { lg: 3, md: 2, sm: 1 },
};

// Mobile breakpoint (matches Tailwind sm:)
const MOBILE_BREAKPOINT = 640;

export function PanelGrid() {
  const {
    layouts,
    visiblePanels,
    collapsedPanels,
    showGridLines,
    autoCompact,
    updateLayout,
    hidePanel,
    togglePanelCollapsed,
    viewMode,
    focusedPanel,
    setFocusedPanel,
    setViewMode,
  } = useUIStore();

  const currentUser = useCurrentUser();
  const queryClient = useQueryClient();
  const modemId = useActiveModemId();
  const [refreshingPanels, setRefreshingPanels] = useState<Set<PanelId>>(new Set());
  const [refreshErrors, setRefreshErrors] = useState<Partial<Record<PanelId, string>>>({});

  // Filter visible panels by server-side restrictions (allowed_panels)
  const effectiveVisiblePanels = visiblePanels.filter(id => {
    // If server-side allowed_panels is set, enforce it
    if (currentUser?.allowedPanels && !currentUser.allowedPanels.includes(id)) return false;
    return true;
  });

  const [currentBreakpoint, setCurrentBreakpoint] = useState<string>('lg');
  const [isMobile, setIsMobile] = useState(false);

  // Track if we're handling an internal collapse to prevent layout save conflicts
  const isCollapsingRef = useRef(false);

  // Detect mobile on mount and resize — force focus mode on mobile
  useEffect(() => {
    const checkMobile = () => {
      const mobile = window.innerWidth < MOBILE_BREAKPOINT;
      setIsMobile(mobile);
      if (mobile && viewMode !== 'focus') {
        setViewMode('focus');
      }
    };
    checkMobile();
    window.addEventListener('resize', checkMobile);
    return () => window.removeEventListener('resize', checkMobile);
  }, [viewMode, setViewMode]);

  // Filter layouts to only include visible panels (respecting role restrictions)
  const filteredLayouts = useMemo(() => ({
    lg: layouts.lg.filter(l => effectiveVisiblePanels.includes(l.i as PanelId)),
    md: layouts.md.filter(l => effectiveVisiblePanels.includes(l.i as PanelId)),
    sm: layouts.sm.filter(l => effectiveVisiblePanels.includes(l.i as PanelId)),
  }), [layouts, effectiveVisiblePanels]);

  const handleLayoutChange = useCallback((_currentLayout: Layout[], allLayouts: { [key: string]: Layout[] }) => {
    // Skip if we're in the middle of a collapse operation
    if (isCollapsingRef.current) return;

    // Update all breakpoint layouts
    Object.entries(allLayouts).forEach(([breakpoint, layout]) => {
      const existingLayout = layouts[breakpoint as keyof typeof layouts] || [];
      const hiddenPanels = existingLayout.filter(l => !effectiveVisiblePanels.includes(l.i as PanelId));
      updateLayout(breakpoint, [...layout, ...hiddenPanels]);
    });
  }, [layouts, effectiveVisiblePanels, updateLayout]);

  const handleBreakpointChange = useCallback((newBreakpoint: string) => {
    setCurrentBreakpoint(newBreakpoint);
  }, []);

  // Handle collapse with flag to prevent layout conflicts
  const handleCollapse = useCallback((panelId: PanelId) => {
    isCollapsingRef.current = true;
    togglePanelCollapsed(panelId);
    // Reset flag after a short delay to allow layout to settle
    setTimeout(() => {
      isCollapsingRef.current = false;
    }, 100);
  }, [togglePanelCollapsed]);

  // Handle header tap on mobile
  const handleHeaderClick = useCallback((panelId: PanelId, e: React.MouseEvent | React.TouchEvent) => {
    if (isMobile) {
      e.stopPropagation();
      e.preventDefault();
      handleCollapse(panelId);
    }
  }, [isMobile, handleCollapse]);

  // Refresh panel data: POST on-demand endpoint if available, else invalidate queries
  const handleRefresh = useCallback(async (panelId: PanelId) => {
    setRefreshingPanels(prev => new Set(prev).add(panelId));
    setRefreshErrors(prev => { const next = { ...prev }; delete next[panelId]; return next; });

    const refresh = PANEL_REFRESH[panelId];

    if (refresh && modemId) {
      try {
        const data = await refresh.fn(modemId);
        queryClient.setQueryData([...refresh.queryKey], data);
      } catch (err) {
        const msg = err instanceof Error ? err.message : 'Refresh failed';
        setRefreshErrors(prev => ({ ...prev, [panelId]: msg }));
        setTimeout(() => setRefreshErrors(prev => { const next = { ...prev }; delete next[panelId]; return next; }), 3000);
      }
    } else {
      // Fallback: invalidate queries (panels without on-demand endpoints)
      const queryKeys = PANEL_QUERY_KEYS[panelId];
      if (queryKeys) {
        queryKeys.forEach(key => {
          queryClient.invalidateQueries({ queryKey: [...key] });
        });
      }
      // Dynamic query keys that depend on modemId
      if (panelId === 'band-lock' && modemId) {
        queryClient.invalidateQueries({ queryKey: bandConfigQueryKey(modemId) });
      }
    }

    // Clear spin animation after 1s
    setTimeout(() => {
      setRefreshingPanels(prev => {
        const next = new Set(prev);
        next.delete(panelId);
        return next;
      });
    }, 1000);
  }, [queryClient, modemId]);

  if (effectiveVisiblePanels.length === 0 && viewMode === 'dashboard') {
    return (
      <div className="flex items-center justify-center h-64 text-theme-text-muted">
        <div className="text-center">
          <p className="text-lg mb-2">No panels visible</p>
          <p className="text-sm">Use the sidebar to enable panels</p>
        </div>
      </div>
    );
  }

  // ========== FOCUS MODE ==========
  const effectiveViewMode = isMobile ? 'focus' : viewMode;

  if (effectiveViewMode === 'focus') {
    // Determine which panel to show: use focusedPanel, or fall back to first visible, or first available
    const allPanels = PANEL_CONFIGS.filter(p => p.id !== 'wan-manager').map(p => p.id);
    const activePanelId = focusedPanel && allPanels.includes(focusedPanel)
      ? focusedPanel
      : effectiveVisiblePanels[0] || allPanels[0] || null;

    // Sync focusedPanel if it was null or invalid
    if (activePanelId && activePanelId !== focusedPanel) {
      // Use a microtask to avoid setting state during render
      queueMicrotask(() => setFocusedPanel(activePanelId));
    }

    if (!activePanelId) {
      return (
        <div className="flex items-center justify-center h-64 text-theme-text-muted">
          <div className="text-center">
            <p className="text-lg mb-2">No panels available</p>
            <p className="text-sm">Use the sidebar to select a panel</p>
          </div>
        </div>
      );
    }

    const Component = PANEL_COMPONENTS[activePanelId];
    const config = PANEL_CONFIGS.find(p => p.id === activePanelId);

    if (!Component) return null;

    return (
      <div className="p-3">
        <div className="bg-theme-bg-card rounded-2xl shadow-sm border-2 border-theme-border overflow-hidden backdrop-blur-md backdrop-saturate-150 flex flex-col min-h-[calc(100vh-10rem)]">
          {/* Panel Header */}
          <div className="flex items-center justify-between px-3 py-1.5 select-none shrink-0">
            <div className="flex items-center gap-2 min-w-0">
              <span className="text-sm font-semibold text-theme-text-primary truncate">
                {config?.title}
              </span>
              {activePanelId !== 'system-update' && (
                <ModemSubtext />
              )}
            </div>
            <div className="flex items-center gap-1 shrink-0">
              {/* Refresh error */}
              {refreshErrors[activePanelId] && (
                <span className="text-xs text-theme-error mr-1 truncate max-w-[120px]">{refreshErrors[activePanelId]}</span>
              )}
              {/* Refresh Button */}
              {(PANEL_QUERY_KEYS[activePanelId] || PANEL_REFRESH[activePanelId] || activePanelId === 'band-lock') && (
                <button
                  onClick={() => handleRefresh(activePanelId)}
                  className="btn-icon"
                  title="Refresh"
                >
                  <RefreshCw className={`w-4 h-4 ${refreshingPanels.has(activePanelId) ? 'animate-spin' : ''}`} />
                </button>
              )}
            </div>
          </div>
          {/* Panel Content */}
          <div className="flex-1 overflow-auto min-h-0">
            <Component />
          </div>
        </div>
      </div>
    );
  }

  // ========== DASHBOARD MODE (existing behavior) ==========
  const cols = GRID_CONFIG.cols[currentBreakpoint as keyof typeof GRID_CONFIG.cols] || 3;

  return (
    <div className="relative">
      {/* Grid Lines Overlay */}
      {showGridLines && (
        <div
          className="absolute inset-0 pointer-events-none z-0"
          style={{
            backgroundImage: `
              linear-gradient(to right, rgba(100, 150, 255, 0.12) 1px, transparent 1px),
              linear-gradient(to bottom, rgba(100, 150, 255, 0.12) 1px, transparent 1px)
            `,
            backgroundSize: `calc((100% - ${GRID_CONFIG.containerPadding[0] * 2}px) / ${cols}) ${GRID_CONFIG.rowHeight + GRID_CONFIG.margin[1]}px`,
            backgroundPosition: `${GRID_CONFIG.containerPadding[0]}px ${GRID_CONFIG.containerPadding[1]}px`,
          }}
        />
      )}

      <ResponsiveGridLayout
        className="layout"
        layouts={filteredLayouts}
        breakpoints={{ lg: 1200, md: 768, sm: 480 }}
        cols={GRID_CONFIG.cols}
        rowHeight={GRID_CONFIG.rowHeight}
        margin={GRID_CONFIG.margin}
        containerPadding={GRID_CONFIG.containerPadding}
        onLayoutChange={handleLayoutChange}
        onBreakpointChange={handleBreakpointChange}
        draggableHandle=".panel-drag-handle"
        resizeHandles={['se', 's', 'e']}
        compactType={autoCompact ? 'vertical' : null}
        preventCollision={false}
        isResizable={!isMobile}
        isDraggable={!isMobile}
      >
        {effectiveVisiblePanels.map((panelId) => {
          const Component = PANEL_COMPONENTS[panelId];
          const config = PANEL_CONFIGS.find(p => p.id === panelId);
          const isCollapsed = collapsedPanels.includes(panelId);

          if (!Component) return null;

          return (
            <div key={panelId} className="panel-wrapper">
              <div className="relative h-full flex flex-col bg-theme-bg-card rounded-2xl shadow-sm border-2 border-theme-border overflow-hidden backdrop-blur-md backdrop-saturate-150">
                {/* Panel Header - Always visible, draggable on desktop, tappable on mobile */}
                <div
                  className={`panel-drag-handle flex items-center justify-between px-3 py-1.5 select-none shrink-0 ${
                    isCollapsed
                      ? 'bg-theme-bg-tertiary/50'
                      : ''
                  } ${isMobile ? 'cursor-pointer' : 'cursor-move'}`}
                  onClick={(e) => handleHeaderClick(panelId, e)}
                  onTouchEnd={(e) => handleHeaderClick(panelId, e)}
                >
                  <div className="flex items-center gap-2 min-w-0">
                    {/* Drag handle icon - hide on mobile */}
                    <GripHorizontal className="w-3.5 h-3.5 text-theme-text-secondary shrink-0 hidden sm:block" />
                    {/* Collapse indicator on mobile */}
                    {isMobile && (
                      <span className="text-theme-text-muted">
                        {isCollapsed ? (
                          <ChevronRight className="w-4 h-4" />
                        ) : (
                          <ChevronDown className="w-4 h-4" />
                        )}
                      </span>
                    )}
                    <span className="text-sm font-semibold text-theme-text-primary truncate">
                      {config?.title}
                    </span>
                    {panelId !== 'system-update' && (
                      <ModemSubtext />
                    )}
                  </div>

                  {/* Desktop-only buttons */}
                  <div className="hidden sm:flex items-center gap-1 shrink-0">
                    {/* Brief refresh error */}
                    {refreshErrors[panelId] && (
                      <span className="text-xs text-theme-error mr-1 truncate max-w-[120px]">{refreshErrors[panelId]}</span>
                    )}
                    {/* Refresh Button - only for panels with query data */}
                    {(PANEL_QUERY_KEYS[panelId] || PANEL_REFRESH[panelId] || panelId === 'band-lock') && (
                      <button
                        onMouseDown={(e) => e.stopPropagation()}
                        onTouchStart={(e) => e.stopPropagation()}
                        onClick={(e) => {
                          e.stopPropagation();
                          e.preventDefault();
                          handleRefresh(panelId);
                        }}
                        className="btn-icon"
                        title="Refresh"
                      >
                        <RefreshCw className={`w-4 h-4 ${refreshingPanels.has(panelId) ? 'animate-spin' : ''}`} />
                      </button>
                    )}
                    {/* Collapse/Expand Button */}
                    <button
                      onMouseDown={(e) => e.stopPropagation()}
                      onTouchStart={(e) => e.stopPropagation()}
                      onClick={(e) => {
                        e.stopPropagation();
                        e.preventDefault();
                        handleCollapse(panelId);
                      }}
                      className="btn-icon"
                      title={isCollapsed ? 'Expand' : 'Minimize'}
                    >
                      {isCollapsed ? (
                        <Plus className="w-5 h-5" />
                      ) : (
                        <Minus className="w-5 h-5" />
                      )}
                    </button>
                    {/* Close Button */}
                    <button
                      onMouseDown={(e) => e.stopPropagation()}
                      onTouchStart={(e) => e.stopPropagation()}
                      onClick={(e) => {
                        e.stopPropagation();
                        e.preventDefault();
                        hidePanel(panelId);
                      }}
                      className="btn-icon-danger"
                      title="Hide panel"
                    >
                      <X className="w-5 h-5" />
                    </button>
                  </div>
                </div>

                {/* Panel Content - Hidden when collapsed */}
                {!isCollapsed && (
                  <div className="flex-1 overflow-auto min-h-0">
                    <Component />
                  </div>
                )}
              </div>
            </div>
          );
        })}
      </ResponsiveGridLayout>
    </div>
  );
}
