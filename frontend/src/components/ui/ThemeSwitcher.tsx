/**
 * ThemeSwitcher Component
 *
 * Cycles through themes: System → Light → Dark → Fallen
 */

import { useUIStore } from '@/stores/uiStore';
import { Monitor, Sun, Moon, Radiation } from 'lucide-react';
import clsx from 'clsx';

const THEME_CYCLE = ['system', 'light', 'dark', 'fallen'] as const;

const THEME_CONFIG = {
  system: { label: 'Theme: System', icon: Monitor },
  light:  { label: 'Theme: Light',  icon: Sun },
  dark:   { label: 'Theme: Dark',   icon: Moon },
  fallen: { label: 'Theme: Fallen', icon: Radiation },
} as const;

interface ThemeSwitcherProps {
  alwaysExpanded?: boolean;
}

export function ThemeSwitcher({ alwaysExpanded }: ThemeSwitcherProps = {}) {
  const theme = useUIStore((s) => s.theme);
  const setTheme = useUIStore((s) => s.setTheme);
  const sidebarCollapsed = useUIStore((s) => s.sidebarCollapsed);

  const cycleTheme = () => {
    const currentIndex = THEME_CYCLE.indexOf(theme);
    const nextIndex = (currentIndex + 1) % THEME_CYCLE.length;
    setTheme(THEME_CYCLE[nextIndex] ?? 'system');
  };

  const isFallen = theme === 'fallen';
  const config = THEME_CONFIG[theme];
  const Icon = config.icon;

  return (
    <button
      onClick={cycleTheme}
      className={clsx(
        'w-full flex items-center gap-2 px-3 py-3 sm:py-2 min-h-[44px] sm:min-h-0',
        'rounded-lg transition-colors text-sm',
        isFallen
          ? 'text-theme-success bg-theme-success/20 hover:bg-theme-success/30'
          : 'text-theme-text-secondary hover:text-theme-text-primary hover:bg-theme-bg-tertiary/50',
        !alwaysExpanded && sidebarCollapsed && 'justify-center'
      )}
      title={config.label}
    >
      <Icon className="w-4 h-4" />
      {(alwaysExpanded || !sidebarCollapsed) && (
        <span>{config.label}</span>
      )}
    </button>
  );
}
