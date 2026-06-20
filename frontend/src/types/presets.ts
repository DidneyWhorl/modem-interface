/**
 * View Preset Types
 *
 * Layout presets allow users to save and switch between different
 * panel arrangements with a single click.
 */

import type { Layout } from 'react-grid-layout';
import type { PanelId } from '@/stores/uiStore';

export interface ViewPreset {
  id: string;
  name: string;
  icon: string;
  layouts: { lg: Layout[]; md: Layout[]; sm: Layout[] };
  visiblePanels: PanelId[];
  collapsedPanels: PanelId[];
  expandedHeights: Record<string, number>;
  createdAt: number;
}

export const MAX_PRESETS = 10;
export const MAX_PRESET_NAME_LENGTH = 20;

export const PRESET_ICON_OPTIONS = [
  'BarChart3', 'Radio', 'Home', 'Zap', 'Wrench', 'Smartphone', 'Monitor', 'Eye', 'Lock', 'ClipboardList',
  'Settings', 'Globe', 'Signal', 'Bell', 'TrendingUp', 'Hammer', 'Target', 'Package', 'Moon', 'Star',
  '1', '2', '3', '4', '5', '6', '7', '8', '9', '0',
] as const;

export const DEFAULT_PRESET_ICON = 'Home';
