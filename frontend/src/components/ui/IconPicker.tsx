/**
 * Icon Picker
 *
 * Grid of lucide icons and number characters for view preset customization.
 */

import clsx from 'clsx';
import {
  BarChart3, Radio, Home, Zap, Wrench, Smartphone, Monitor, Eye, Lock, ClipboardList,
  Settings, Globe, Signal, Bell, TrendingUp, Hammer, Target, Package, Moon, Star,
} from 'lucide-react';
import type { LucideIcon } from 'lucide-react';
import { PRESET_ICON_OPTIONS } from '@/types/presets';

/** Map icon name strings to their lucide component. */
const ICON_MAP: Record<string, LucideIcon> = {
  BarChart3, Radio, Home, Zap, Wrench, Smartphone, Monitor, Eye, Lock, ClipboardList,
  Settings, Globe, Signal, Bell, TrendingUp, Hammer, Target, Package, Moon, Star,
};

interface IconPickerProps {
  selected: string;
  onSelect: (icon: string) => void;
}

export function IconPicker({ selected, onSelect }: IconPickerProps) {
  return (
    <div className="grid grid-cols-6 gap-1">
      {PRESET_ICON_OPTIONS.map((icon) => {
        const LucideComponent = ICON_MAP[icon];
        return (
          <button
            key={icon}
            type="button"
            onClick={() => onSelect(icon)}
            className={clsx(
              'w-8 h-8 flex items-center justify-center rounded text-base transition-colors',
              icon === selected
                ? 'bg-theme-accent/20 border border-theme-accent text-theme-text-accent'
                : 'hover:bg-theme-bg-tertiary border border-transparent text-theme-text-secondary'
            )}
          >
            {LucideComponent ? (
              <LucideComponent className="w-5 h-5" />
            ) : (
              <span className="text-sm font-bold">{icon}</span>
            )}
          </button>
        );
      })}
    </div>
  );
}

/** Render a preset icon by name. Falls back to Home icon for unknown names. */
export function PresetIcon({ name, className }: { name: string; className?: string }) {
  const LucideComponent = ICON_MAP[name];
  if (LucideComponent) {
    return <LucideComponent className={className ?? 'w-4 h-4'} />;
  }
  // Number characters or unknown strings — render as text
  if (/^[0-9]$/.test(name)) {
    return <span className={clsx('font-bold leading-none', className ?? 'text-sm')}>{name}</span>;
  }
  // Fallback for legacy emoji icons stored in localStorage
  return <Home className={className ?? 'w-4 h-4'} />;
}
