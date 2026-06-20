/**
 * WAN Manager Page
 *
 * Full-page layout for the WAN Manager panel.
 * Uses the same dark theme and WanManagerPanel component,
 * with a header bar for navigation back to the main modem view.
 */

import { Link } from 'react-router-dom';
import { ArrowLeft, Network } from 'lucide-react';
import { WanManagerPanel } from '@/components/modem/WanManagerPanel';
import { ThemeSwitcher } from '@/components/ui/ThemeSwitcher';
import { AgcccLogo } from '@/components/ui/AgcccLogo';

export function WanManagerPage() {
  return (
    <div className="min-h-screen bg-theme-bg-primary">
      {/* Header */}
      <header className="bg-theme-bg-card backdrop-blur-sm shadow-sm border-b border-theme-border sticky top-0 z-10">
        <div className="px-4 sm:px-6 lg:px-8 py-3">
          <div className="flex items-center justify-between">
            {/* Left: Back link + title */}
            <div className="flex items-center gap-4">
              <Link
                to="/home"
                className="flex items-center gap-2 text-sm text-theme-text-secondary hover:text-theme-text-primary transition-colors"
              >
                <ArrowLeft className="w-4 h-4" />
                <span>Home</span>
              </Link>

              <div className="h-5 w-px bg-theme-border" />

              <div className="flex items-center gap-3">
                <div className="p-1.5 bg-theme-bg-tertiary rounded-lg">
                  <Network className="w-5 h-5 text-theme-text-primary" />
                </div>
                <div>
                  <h1 className="text-lg font-semibold text-theme-text-primary">
                    CTRL-WAN
                  </h1>
                  <p className="text-xs text-theme-text-secondary hidden sm:block">
                    Multi-Modem Priority & Failover
                  </p>
                </div>
              </div>
            </div>

            {/* Right: Logo + theme */}
            <div className="flex items-center gap-3">
              <ThemeSwitcher />
              <div className="p-1.5 bg-theme-bg-tertiary rounded-lg">
                <AgcccLogo size={20} className="text-theme-text-primary" />
              </div>
            </div>
          </div>
        </div>
      </header>

      {/* Content */}
      <main className="px-4 sm:px-6 lg:px-8 py-6 max-w-6xl mx-auto">
        <div className="bg-theme-bg-secondary rounded-2xl p-4 sm:p-6">
          <WanManagerPanel />
        </div>
      </main>

      {/* Footer */}
      <footer className="py-4 border-t border-theme-border-light">
        <div className="px-4 sm:px-6 lg:px-8">
          <p className="text-center text-xs text-theme-text-muted">
            &copy; 2026 Net Solution Shop LLC
          </p>
        </div>
      </footer>
    </div>
  );
}
