/**
 * UI Store (Zustand)
 *
 * Client-only state that doesn't need to be fetched from server:
 * - Panel visibility and grid positions
 * - UI preferences
 * - Theme settings
 * - AT terminal history
 * - Signal update preferences
 * - View presets (synced to server via usePresetSync hook)
 */

import { create } from 'zustand';
import { persist } from 'zustand/middleware';
import type { Layout } from 'react-grid-layout';
import type { ViewPreset } from '@/types/presets';
import { MAX_PRESETS, DEFAULT_PRESET_ICON } from '@/types/presets';

// Panel definitions
export type PanelId =
  | 'connection-info'
  | 'connection-panel'
  | 'device-info'
  | 'sim-card'
  | 'at-terminal'
  | 'system-update'
  | 'gps'
  | 'antenna-metrics'
  | 'band-lock'
  | 'debug-log'
  | 'wan-manager'
  | 'signal-trending'
  | 'speedtest';

export interface PanelConfig {
  id: PanelId;
  title: string;
  icon: string; // Lucide icon name
  defaultVisible: boolean;
  // Default grid position and size
  defaultLayout: { x: number; y: number; w: number; h: number; minW?: number; minH?: number };
}

// Default layout (3 columns):
// Row 0: Connection Info | Device Info
// Row 8: Connection Settings | AT Terminal | SIM Card
export const PANEL_CONFIGS: PanelConfig[] = [
  { id: 'connection-info', title: 'Connection Info', icon: 'Wifi', defaultVisible: true,
    defaultLayout: { x: 0, y: 0, w: 1, h: 8, minW: 1, minH: 3 } },
  { id: 'device-info', title: 'Device Info', icon: 'Smartphone', defaultVisible: true,
    defaultLayout: { x: 1, y: 0, w: 1, h: 4, minW: 1, minH: 1 } },
  { id: 'connection-panel', title: 'APN / PDP Details', icon: 'Settings', defaultVisible: true,
    defaultLayout: { x: 0, y: 4, w: 1, h: 6, minW: 1, minH: 1 } },
  { id: 'at-terminal', title: 'AT Terminal', icon: 'Terminal', defaultVisible: true,
    defaultLayout: { x: 1, y: 4, w: 1, h: 5, minW: 1, minH: 1 } },
  { id: 'sim-card', title: 'SIM Card', icon: 'CreditCard', defaultVisible: true,
    defaultLayout: { x: 2, y: 4, w: 1, h: 4, minW: 1, minH: 1 } },
  { id: 'system-update', title: 'System Update', icon: 'Download', defaultVisible: false,
    defaultLayout: { x: 2, y: 8, w: 1, h: 3, minW: 1, minH: 1 } },
  { id: 'gps', title: 'GPS', icon: 'MapPin', defaultVisible: false,
    defaultLayout: { x: 1, y: 8, w: 1, h: 4, minW: 1, minH: 1 } },
  { id: 'antenna-metrics', title: 'Antenna Metrics', icon: 'Antenna', defaultVisible: false,
    defaultLayout: { x: 2, y: 8, w: 1, h: 4, minW: 1, minH: 1 } },
  { id: 'band-lock', title: 'Band Lock', icon: 'Radio', defaultVisible: false,
    defaultLayout: { x: 0, y: 12, w: 1, h: 8, minW: 1, minH: 3 } },
  { id: 'debug-log', title: 'Debug Log', icon: 'Bug', defaultVisible: false,
    defaultLayout: { x: 0, y: 20, w: 2, h: 6, minW: 1, minH: 3 } },
  { id: 'wan-manager', title: 'CTRL-WAN', icon: 'Network', defaultVisible: false,
    defaultLayout: { x: 0, y: 26, w: 2, h: 8, minW: 1, minH: 4 } },
  { id: 'signal-trending', title: 'Signal Trending', icon: 'TrendingUp', defaultVisible: false,
    defaultLayout: { x: 0, y: 34, w: 2, h: 5, minW: 1, minH: 3 } },
  { id: 'speedtest', title: 'Speed Test', icon: 'Gauge', defaultVisible: false,
    defaultLayout: { x: 0, y: 38, w: 2, h: 6, minW: 1, minH: 4 } },
];

// Generate default layouts for react-grid-layout
const generateDefaultLayouts = (): { lg: Layout[]; md: Layout[]; sm: Layout[] } => {
  const lgLayout: Layout[] = PANEL_CONFIGS.map(p => ({
    i: p.id,
    ...p.defaultLayout,
  }));

  // Medium: 2 columns
  const mdLayout: Layout[] = [
    { i: 'connection-info', x: 0, y: 0, w: 1, h: 8, minW: 1, minH: 3 },
    { i: 'device-info', x: 1, y: 0, w: 1, h: 4, minW: 1, minH: 1 },
    { i: 'sim-card', x: 1, y: 4, w: 1, h: 4, minW: 1, minH: 1 },
    { i: 'connection-panel', x: 0, y: 8, w: 1, h: 5, minW: 1, minH: 1 },
    { i: 'at-terminal', x: 1, y: 8, w: 1, h: 5, minW: 1, minH: 1 },
    { i: 'system-update', x: 0, y: 13, w: 1, h: 3, minW: 1, minH: 1 },
    { i: 'gps', x: 0, y: 16, w: 1, h: 4, minW: 1, minH: 1 },
    { i: 'antenna-metrics', x: 1, y: 16, w: 1, h: 4, minW: 1, minH: 1 },
    { i: 'debug-log', x: 0, y: 20, w: 2, h: 6, minW: 1, minH: 3 },
    { i: 'signal-trending', x: 0, y: 26, w: 2, h: 5, minW: 1, minH: 3 },
    { i: 'speedtest', x: 0, y: 31, w: 2, h: 6, minW: 1, minH: 4 },
  ];

  // Small: 1 column - stack vertically
  const smLayout: Layout[] = [
    { i: 'connection-info', x: 0, y: 0, w: 1, h: 8, minW: 1, minH: 3 },
    { i: 'device-info', x: 0, y: 8, w: 1, h: 4, minW: 1, minH: 1 },
    { i: 'connection-panel', x: 0, y: 12, w: 1, h: 5, minW: 1, minH: 1 },
    { i: 'at-terminal', x: 0, y: 17, w: 1, h: 5, minW: 1, minH: 1 },
    { i: 'sim-card', x: 0, y: 22, w: 1, h: 4, minW: 1, minH: 1 },
    { i: 'system-update', x: 0, y: 26, w: 1, h: 3, minW: 1, minH: 1 },
    { i: 'gps', x: 0, y: 29, w: 1, h: 4, minW: 1, minH: 1 },
    { i: 'antenna-metrics', x: 0, y: 33, w: 1, h: 4, minW: 1, minH: 1 },
    { i: 'debug-log', x: 0, y: 37, w: 1, h: 6, minW: 1, minH: 3 },
    { i: 'signal-trending', x: 0, y: 43, w: 1, h: 5, minW: 1, minH: 3 },
    { i: 'speedtest', x: 0, y: 48, w: 1, h: 6, minW: 1, minH: 4 },
  ];

  return { lg: lgLayout, md: mdLayout, sm: smLayout };
};

const DEFAULT_LAYOUTS = generateDefaultLayouts();
const DEFAULT_VISIBLE_PANELS: PanelId[] = PANEL_CONFIGS.filter(p => p.defaultVisible).map(p => p.id);
const DEFAULT_PANEL_ORDER: PanelId[] = PANEL_CONFIGS.filter(p => p.id !== 'wan-manager').map(p => p.id);

interface ATHistoryEntry {
  command: string;
  response: string;
  timestamp: number;
  success: boolean;
}

/** Capture current layout state into a ViewPreset snapshot. */
function captureLayoutSnapshot(state: UIState): Pick<ViewPreset, 'layouts' | 'visiblePanels' | 'collapsedPanels' | 'expandedHeights'> {
  return {
    layouts: { lg: [...state.layouts.lg], md: [...state.layouts.md], sm: [...state.layouts.sm] },
    visiblePanels: [...state.visiblePanels],
    collapsedPanels: [...state.collapsedPanels],
    expandedHeights: { ...state.expandedHeights },
  };
}

interface UIState {
  // Panel management - grid layouts per breakpoint
  layouts: { lg: Layout[]; md: Layout[]; sm: Layout[] };
  visiblePanels: PanelId[];
  collapsedPanels: PanelId[]; // Panels that are minimized
  expandedHeights: Record<string, number>; // Original heights before collapse
  sidebarCollapsed: boolean;
  showGridLines: boolean; // Show snap grid overlay
  autoCompact: boolean; // Auto-sort panels vertically

  // View mode: 'dashboard' = multi-panel grid, 'focus' = single panel at a time
  viewMode: 'dashboard' | 'focus';
  focusedPanel: PanelId | null; // Which panel is shown in focus mode

  // Sidebar panel ordering (persisted)
  panelOrder: PanelId[];
  // Sidebar reorder unlock state (transient — always starts locked)
  sidebarReorderUnlocked: boolean;

  // AT Terminal
  atHistory: ATHistoryEntry[];
  atInputHistory: string[]; // Previous commands for up-arrow recall

  // WebSocket connection state (transient — not persisted)
  wsConnected: boolean;

  // Polling intervals (persisted per-panel, e.g. { 'antenna-metrics': 30000 })
  pollingIntervals: Record<string, number>;

  // User preferences
  theme: 'system' | 'light' | 'dark' | 'fallen';

  // View presets
  presets: ViewPreset[];
  activePresetId: string | null;
  _presetsDirty: boolean; // Transient: needs server sync
  _isSwitchingPreset: boolean; // Transient: suppress auto-snapshot during switch

  // Panel actions
  setLayouts: (layouts: { lg: Layout[]; md: Layout[]; sm: Layout[] }) => void;
  updateLayout: (breakpoint: string, layout: Layout[]) => void;
  togglePanelVisibility: (panelId: PanelId) => void;
  showPanel: (panelId: PanelId) => void;
  hidePanel: (panelId: PanelId) => void;
  togglePanelCollapsed: (panelId: PanelId) => void;
  resetPanelLayout: () => void;
  toggleSidebarCollapsed: () => void;
  toggleGridLines: () => void;
  toggleAutoCompact: () => void;

  // View mode actions
  setViewMode: (mode: 'dashboard' | 'focus') => void;
  setFocusedPanel: (panel: PanelId | null) => void;

  // Sidebar reorder actions
  setPanelOrder: (order: PanelId[]) => void;
  toggleSidebarReorder: () => void;

  // AT Terminal actions
  addATHistoryEntry: (entry: ATHistoryEntry) => void;
  clearATHistory: () => void;
  addATInputHistory: (command: string) => void;

  // WebSocket state actions
  setWsConnected: (connected: boolean) => void;

  // Polling interval actions
  setPollingInterval: (panelId: string, interval: number) => void;

  // Preference actions
  setTheme: (theme: 'system' | 'light' | 'dark' | 'fallen') => void;

  // View preset actions
  initPresetsFromServer: (serverPresets: ViewPreset[] | null) => void;
  createPreset: (name: string, icon: string) => void;
  switchPreset: (presetId: string) => void;
  updatePresetMeta: (presetId: string, name: string, icon: string) => void;
  deletePreset: (presetId: string) => void;
  saveActivePresetSnapshot: () => void;
  markPresetsClean: () => void;
  clearPresets: () => void;
}

// Find first available position in the grid
const findEmptyPosition = (layouts: Layout[], cols: number): { x: number; y: number } => {
  if (layouts.length === 0) return { x: 0, y: 0 };

  // Find the lowest y position that has space
  const occupied = new Set<string>();
  let maxY = 0;

  for (const l of layouts) {
    for (let dx = 0; dx < l.w; dx++) {
      for (let dy = 0; dy < l.h; dy++) {
        occupied.add(`${l.x + dx},${l.y + dy}`);
      }
    }
    maxY = Math.max(maxY, l.y + l.h);
  }

  // Try to find a gap
  for (let y = 0; y <= maxY; y++) {
    for (let x = 0; x < cols; x++) {
      if (!occupied.has(`${x},${y}`)) {
        return { x, y };
      }
    }
  }

  // No gap found, place at bottom
  return { x: 0, y: maxY };
};

export const useUIStore = create<UIState>()(
  persist(
    (set, get) => ({
      // Initial state
      layouts: DEFAULT_LAYOUTS,
      visiblePanels: DEFAULT_VISIBLE_PANELS,
      collapsedPanels: [],
      expandedHeights: {},
      sidebarCollapsed: false,
      showGridLines: false,
      autoCompact: true,
      viewMode: 'dashboard',
      focusedPanel: null,
      panelOrder: DEFAULT_PANEL_ORDER,
      sidebarReorderUnlocked: false,
      atHistory: [],
      atInputHistory: [],
      wsConnected: false,        // Transient: REST polls until WS connects
      pollingIntervals: {},
      theme: 'system',

      // View presets
      presets: [],
      activePresetId: null,
      _presetsDirty: false,
      _isSwitchingPreset: false,

      // Panel actions
      setLayouts: (layouts) => set({ layouts }),

      updateLayout: (breakpoint, layout) => {
        set((state) => ({
          layouts: {
            ...state.layouts,
            [breakpoint]: layout,
          },
        }));
        // Auto-snapshot active preset after layout change
        get().saveActivePresetSnapshot();
      },

      togglePanelVisibility: (panelId) => {
        const state = get();
        if (state.visiblePanels.includes(panelId)) {
          set({ visiblePanels: state.visiblePanels.filter((id) => id !== panelId) });
        } else {
          // Find empty position for the panel
          const visibleLayouts = state.layouts.lg.filter(l =>
            state.visiblePanels.includes(l.i as PanelId)
          );
          const pos = findEmptyPosition(visibleLayouts, 3);
          const config = PANEL_CONFIGS.find(p => p.id === panelId);

          // Add panel at empty position
          const newLayout: Layout = {
            i: panelId,
            x: pos.x,
            y: pos.y,
            w: config?.defaultLayout.w || 1,
            h: config?.defaultLayout.h || 3,
            minW: 1,
            minH: 1,
          };

          set({
            visiblePanels: [...state.visiblePanels, panelId],
            layouts: {
              ...state.layouts,
              lg: [...state.layouts.lg.filter(l => l.i !== panelId), newLayout],
              md: [...state.layouts.md.filter(l => l.i !== panelId), { ...newLayout, w: Math.min(newLayout.w, 2) }],
              sm: [...state.layouts.sm.filter(l => l.i !== panelId), { ...newLayout, x: 0, w: 1 }],
            },
          });
        }
        get().saveActivePresetSnapshot();
      },

      showPanel: (panelId) => {
        const state = get();
        if (state.visiblePanels.includes(panelId)) return;

        const visibleLayouts = state.layouts.lg.filter(l =>
          state.visiblePanels.includes(l.i as PanelId)
        );
        const pos = findEmptyPosition(visibleLayouts, 3);
        const config = PANEL_CONFIGS.find(p => p.id === panelId);

        const newLayout: Layout = {
          i: panelId,
          x: pos.x,
          y: pos.y,
          w: config?.defaultLayout.w || 1,
          h: config?.defaultLayout.h || 3,
          minW: 1,
          minH: 1,
        };

        set({
          visiblePanels: [...state.visiblePanels, panelId],
          layouts: {
            ...state.layouts,
            lg: [...state.layouts.lg.filter(l => l.i !== panelId), newLayout],
            md: [...state.layouts.md.filter(l => l.i !== panelId), { ...newLayout, w: Math.min(newLayout.w, 2) }],
            sm: [...state.layouts.sm.filter(l => l.i !== panelId), { ...newLayout, x: 0, w: 1 }],
          },
        });
        get().saveActivePresetSnapshot();
      },

      hidePanel: (panelId) => {
        set((state) => ({
          visiblePanels: state.visiblePanels.filter((id) => id !== panelId),
        }));
        get().saveActivePresetSnapshot();
      },

      togglePanelCollapsed: (panelId) => {
        const state = get();
        const isCollapsed = state.collapsedPanels.includes(panelId);

        if (isCollapsed) {
          // EXPAND: Restore original height from expandedHeights
          const originalHeight = state.expandedHeights[panelId] || 3;

          const newLayouts = {
            lg: state.layouts.lg.map(l => l.i === panelId ? { ...l, h: originalHeight } : l),
            md: state.layouts.md.map(l => l.i === panelId ? { ...l, h: originalHeight } : l),
            sm: state.layouts.sm.map(l => l.i === panelId ? { ...l, h: originalHeight } : l),
          };

          set({
            collapsedPanels: state.collapsedPanels.filter(id => id !== panelId),
            layouts: newLayouts,
          });
        } else {
          // COLLAPSE: Save current height, set to 1
          const currentLayout = state.layouts.lg.find(l => l.i === panelId);
          const currentHeight = currentLayout?.h || 3;

          const newLayouts = {
            lg: state.layouts.lg.map(l => l.i === panelId ? { ...l, h: 1 } : l),
            md: state.layouts.md.map(l => l.i === panelId ? { ...l, h: 1 } : l),
            sm: state.layouts.sm.map(l => l.i === panelId ? { ...l, h: 1 } : l),
          };

          set({
            collapsedPanels: [...state.collapsedPanels, panelId],
            expandedHeights: {
              ...state.expandedHeights,
              [panelId]: currentHeight,
            },
            layouts: newLayouts,
          });
        }
        get().saveActivePresetSnapshot();
      },

      resetPanelLayout: () => {
        set({
          layouts: generateDefaultLayouts(),
          visiblePanels: DEFAULT_VISIBLE_PANELS,
          collapsedPanels: [],
          expandedHeights: {},
        });
        get().saveActivePresetSnapshot();
      },

      toggleSidebarCollapsed: () =>
        set((state) => ({ sidebarCollapsed: !state.sidebarCollapsed })),

      toggleGridLines: () =>
        set((state) => ({ showGridLines: !state.showGridLines })),

      toggleAutoCompact: () =>
        set((state) => ({ autoCompact: !state.autoCompact })),

      // View mode actions
      setViewMode: (mode) => {
        const state = get();
        if (mode === 'focus' && !state.focusedPanel) {
          // Set focused panel to first visible panel when entering focus mode
          const firstVisible = state.visiblePanels[0] || null;
          set({ viewMode: mode, focusedPanel: firstVisible });
        } else {
          set({ viewMode: mode });
        }
      },
      setFocusedPanel: (panel) => set({ focusedPanel: panel }),

      // Sidebar reorder actions
      setPanelOrder: (order) => set({ panelOrder: order }),
      toggleSidebarReorder: () => set((state) => ({ sidebarReorderUnlocked: !state.sidebarReorderUnlocked })),

      // AT Terminal actions
      addATHistoryEntry: (entry) =>
        set((state) => ({
          atHistory: [...state.atHistory.slice(-99), entry], // Keep last 100
        })),

      clearATHistory: () => set({ atHistory: [] }),

      addATInputHistory: (command) =>
        set((state) => ({
          atInputHistory: [
            ...state.atInputHistory.filter((c) => c !== command).slice(-49),
            command,
          ], // Keep last 50 unique commands
        })),

      // WebSocket state actions
      setWsConnected: (connected) => set({ wsConnected: connected }),

      // Polling interval actions
      setPollingInterval: (panelId, interval) =>
        set((state) => ({
          pollingIntervals: { ...state.pollingIntervals, [panelId]: interval },
        })),

      // Preference actions
      setTheme: (theme) => set({ theme }),

      // === View Preset Actions ===

      initPresetsFromServer: (serverPresets) => {
        if (!serverPresets || serverPresets.length === 0) {
          // Clear any presets from a previous user session so they don't bleed over.
          // ViewPresetBar will auto-create a fresh "Default" preset.
          set({ presets: [], activePresetId: null, _presetsDirty: false });
          return;
        }
        const active = serverPresets[0];
        if (!active) return;

        // Load the first preset's layout into the active state
        set({
          presets: serverPresets,
          activePresetId: active.id,
          layouts: active.layouts,
          visiblePanels: active.visiblePanels,
          collapsedPanels: active.collapsedPanels,
          expandedHeights: active.expandedHeights,
          _presetsDirty: false,
        });
      },

      createPreset: (name, icon) => {
        const state = get();
        if (state.presets.length >= MAX_PRESETS) return;

        const newPreset: ViewPreset = {
          id: crypto.randomUUID(),
          name: name.slice(0, 20),
          icon: icon || DEFAULT_PRESET_ICON,
          ...captureLayoutSnapshot(state),
          createdAt: Date.now(),
        };

        set({
          presets: [...state.presets, newPreset],
          activePresetId: newPreset.id,
          _presetsDirty: true,
        });
      },

      switchPreset: (presetId) => {
        const state = get();
        if (state.activePresetId === presetId) return;

        const target = state.presets.find(p => p.id === presetId);
        if (!target) return;

        // Save current layout into the currently active preset before switching
        const updatedPresets = state.activePresetId
          ? state.presets.map(p =>
              p.id === state.activePresetId
                ? { ...p, ...captureLayoutSnapshot(state) }
                : p
            )
          : state.presets;

        // Load target preset's layout
        set({
          _isSwitchingPreset: true,
          presets: updatedPresets,
          activePresetId: presetId,
          layouts: target.layouts,
          visiblePanels: target.visiblePanels,
          collapsedPanels: target.collapsedPanels,
          expandedHeights: target.expandedHeights,
          _presetsDirty: true,
        });

        // Clear the switching guard after the state has settled
        set({ _isSwitchingPreset: false });
      },

      updatePresetMeta: (presetId, name, icon) => {
        set((state) => ({
          presets: state.presets.map(p =>
            p.id === presetId ? { ...p, name: name.slice(0, 20), icon } : p
          ),
          _presetsDirty: true,
        }));
      },

      deletePreset: (presetId) => {
        const state = get();
        if (state.presets.length <= 1) return; // Must keep at least one

        const remaining = state.presets.filter(p => p.id !== presetId);
        const wasActive = state.activePresetId === presetId;

        if (wasActive && remaining.length > 0) {
          // Switch to first remaining preset
          const next = remaining[0]!;
          set({
            presets: remaining,
            activePresetId: next.id,
            layouts: next.layouts,
            visiblePanels: next.visiblePanels,
            collapsedPanels: next.collapsedPanels,
            expandedHeights: next.expandedHeights,
            _presetsDirty: true,
          });
        } else {
          set({ presets: remaining, _presetsDirty: true });
        }
      },

      saveActivePresetSnapshot: () => {
        const state = get();
        if (state._isSwitchingPreset || !state.activePresetId) return;

        const updatedPresets = state.presets.map(p =>
          p.id === state.activePresetId
            ? { ...p, ...captureLayoutSnapshot(state) }
            : p
        );

        set({ presets: updatedPresets, _presetsDirty: true });
      },

      markPresetsClean: () => set({ _presetsDirty: false }),

      clearPresets: () => set({ presets: [], activePresetId: null, _presetsDirty: false }),
    }),
    {
      name: 'modem-ui-settings',
      version: 11,
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      migrate: (persistedState: any, version: number) => {
        if (version < 2) {
          // v1→v2: old theme values ('light'/'dark'/'system') to named themes
          if (!persistedState.theme || !['default', 'pipboy'].includes(persistedState.theme)) {
            persistedState.theme = 'default';
          }
        }
        if (version < 3) {
          // v2→v3: rename 'default'→'dark', 'pipboy'→'fallen', add system/light
          if (persistedState.theme === 'default') {
            persistedState.theme = 'dark';
          } else if (persistedState.theme === 'pipboy') {
            persistedState.theme = 'fallen';
          }
          if (!['system', 'light', 'dark', 'fallen'].includes(persistedState.theme)) {
            persistedState.theme = 'system';
          }
        }
        if (version < 4) {
          // v3→v4: add system-update panel to existing layouts
          if (persistedState.visiblePanels && !persistedState.visiblePanels.includes('system-update')) {
            persistedState.visiblePanels.push('system-update');
          }
        }
        if (version < 5) {
          // v4→v5: add view presets
          persistedState.presets = [];
          persistedState.activePresetId = null;
        }
        if (version < 6) {
          // v5→v6: add band-lock panel (hidden by default, no layout changes needed)
        }
        if (version < 7) {
          // v6→v7: add wan-manager panel (hidden by default, no layout changes needed)
        }
        if (version < 9) {
          // v8→v9: add view mode (dashboard/focus) and focused panel
          if (!persistedState.viewMode) {
            persistedState.viewMode = 'dashboard';
          }
          if (!persistedState.focusedPanel) {
            persistedState.focusedPanel = null;
          }
        }
        if (version < 8) {
          // v7→v8: remove user-management panel (moved to modal in header)
          const um = 'user-management';
          if (persistedState.visiblePanels) {
            persistedState.visiblePanels = persistedState.visiblePanels.filter((id: string) => id !== um);
          }
          if (persistedState.collapsedPanels) {
            persistedState.collapsedPanels = persistedState.collapsedPanels.filter((id: string) => id !== um);
          }
          if (persistedState.layouts) {
            for (const bp of ['lg', 'md', 'sm']) {
              if (persistedState.layouts[bp]) {
                persistedState.layouts[bp] = persistedState.layouts[bp].filter((l: { i: string }) => l.i !== um);
              }
            }
          }
          // Strip from presets
          if (persistedState.presets) {
            for (const preset of persistedState.presets) {
              if (preset.visiblePanels) {
                preset.visiblePanels = preset.visiblePanels.filter((id: string) => id !== um);
              }
              if (preset.collapsedPanels) {
                preset.collapsedPanels = preset.collapsedPanels.filter((id: string) => id !== um);
              }
              if (preset.layouts) {
                for (const bp of ['lg', 'md', 'sm']) {
                  if (preset.layouts[bp]) {
                    preset.layouts[bp] = preset.layouts[bp].filter((l: { i: string }) => l.i !== um);
                  }
                }
              }
            }
          }
        }
        if (version < 10) {
          // v9→v10: merge connection-status + signal into connection-info, add panelOrder
          const oldIds = ['connection-status', 'signal'];
          const newId = 'connection-info';

          // Replace old panel IDs in visiblePanels
          if (persistedState.visiblePanels) {
            const hadOld = oldIds.some((id: string) => persistedState.visiblePanels.includes(id));
            persistedState.visiblePanels = persistedState.visiblePanels.filter((id: string) => !oldIds.includes(id));
            if (hadOld && !persistedState.visiblePanels.includes(newId)) {
              persistedState.visiblePanels.unshift(newId);
            }
          }

          // Replace in collapsedPanels
          if (persistedState.collapsedPanels) {
            persistedState.collapsedPanels = persistedState.collapsedPanels.filter((id: string) => !oldIds.includes(id));
          }

          // Replace in layouts
          if (persistedState.layouts) {
            for (const bp of ['lg', 'md', 'sm']) {
              if (persistedState.layouts[bp]) {
                // Remove old entries
                persistedState.layouts[bp] = persistedState.layouts[bp].filter(
                  (l: { i: string }) => !oldIds.includes(l.i)
                );
                // Add new combined panel if not present
                if (!persistedState.layouts[bp].some((l: { i: string }) => l.i === newId)) {
                  persistedState.layouts[bp].unshift({
                    i: newId, x: 0, y: 0,
                    w: bp === 'lg' ? 1 : bp === 'md' ? 1 : 1,
                    h: 8, minW: 1, minH: 3,
                  });
                }
              }
            }
          }

          // Replace in focusedPanel
          if (persistedState.focusedPanel && oldIds.includes(persistedState.focusedPanel)) {
            persistedState.focusedPanel = newId;
          }

          // Strip from presets
          if (persistedState.presets) {
            for (const preset of persistedState.presets) {
              if (preset.visiblePanels) {
                const hadOld = oldIds.some((id: string) => preset.visiblePanels.includes(id));
                preset.visiblePanels = preset.visiblePanels.filter((id: string) => !oldIds.includes(id));
                if (hadOld && !preset.visiblePanels.includes(newId)) {
                  preset.visiblePanels.unshift(newId);
                }
              }
              if (preset.collapsedPanels) {
                preset.collapsedPanels = preset.collapsedPanels.filter((id: string) => !oldIds.includes(id));
              }
              if (preset.layouts) {
                for (const bp of ['lg', 'md', 'sm']) {
                  if (preset.layouts[bp]) {
                    preset.layouts[bp] = preset.layouts[bp].filter(
                      (l: { i: string }) => !oldIds.includes(l.i)
                    );
                    if (!preset.layouts[bp].some((l: { i: string }) => l.i === newId)) {
                      preset.layouts[bp].unshift({
                        i: newId, x: 0, y: 0, w: 1, h: 8, minW: 1, minH: 3,
                      });
                    }
                  }
                }
              }
            }
          }

          // Initialize panelOrder (excluding wan-manager, using new panel ID)
          const allPanelIds = [
            'connection-info', 'device-info', 'connection-panel', 'at-terminal',
            'sim-card', 'system-update', 'gps', 'antenna-metrics', 'band-lock',
            'debug-log', 'signal-trending',
          ];
          persistedState.panelOrder = allPanelIds;
        }
        if (version < 11) {
          // v10→v11: add speedtest panel (hidden by default, no layout changes needed)
        }
        return persistedState;
      },
      // Persist layout and preferences (exclude transient flags)
      partialize: (state) => ({
        layouts: state.layouts,
        visiblePanels: state.visiblePanels,
        collapsedPanels: state.collapsedPanels,
        expandedHeights: state.expandedHeights,
        sidebarCollapsed: state.sidebarCollapsed,
        showGridLines: state.showGridLines,
        autoCompact: state.autoCompact,
        viewMode: state.viewMode,
        focusedPanel: state.focusedPanel,
        panelOrder: state.panelOrder,
        atInputHistory: state.atInputHistory,
        pollingIntervals: state.pollingIntervals,
        theme: state.theme,
        presets: state.presets,
        activePresetId: state.activePresetId,
      }),
    }
  )
);
