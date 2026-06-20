/**
 * Main App Component
 *
 * Gates the dashboard behind authentication.
 * Shows setup page on first run, login page when unauthenticated,
 * and the full dashboard when authenticated.
 *
 * Uses React Router for multi-page navigation:
 * - /home — main modem panel view
 * - /wan-manager — dedicated WAN Manager page
 * - /login — auth page
 */

import { useState, useRef, useEffect, useCallback } from 'react';
import { Routes, Route, Navigate, useLocation, useNavigate } from 'react-router-dom';
import { useQueryClient } from '@tanstack/react-query';
import { useWebSocket, useTheme, useAuth, usePresetSync, useActiveModemId } from '@/hooks';
import { modemHealthQueryKey } from '@/hooks/useWebSocket';
import { modemStatusQueryKey } from '@/hooks/queries/useModemStatus';
import type { UserInfo } from '@/hooks/useAuth';
import type { ModemHealth, ModemStatus } from '@/types/api';
import { UserContext } from '@/contexts/UserContext';
import { useUIStore } from '@/stores/uiStore';
import { Sidebar, PanelGrid } from '@/components/layout';
import { LoginPage } from '@/components/auth/LoginPage';
import { SetupPage } from '@/components/auth/SetupPage';
import { AgcccLogo } from '@/components/ui/AgcccLogo';
import { ViewPresetBar } from '@/components/ui/ViewPresetBar';
import { WanManagerPage } from '@/pages/WanManagerPage';
import { powerDownModem, rebootModem, setAirplaneMode, getAirplaneMode } from '@/api/modem';
import { getLicenseStatus } from '@/api/license';
import type { LicenseStatus } from '@/types/api';
import { ActiveModemLabel } from '@/components/modem/ActiveModemLabel';
import { WifiOff, Power, RotateCcw, Plane, Loader2, ChevronDown, LayoutGrid, Maximize2 } from 'lucide-react';
import clsx from 'clsx';

function App() {
  // Apply active theme to document root
  useTheme();

  // License state is informational only — the license/portal is OPTIONAL and
  // never blocks the dashboard. Unlicensed is a normal, fully-usable state.
  // We fetch the status purely so Settings can surface the device token and a
  // "cloud features: active / not activated" affordance.
  const [licenseInfo, setLicenseInfo] = useState<LicenseStatus | null>(null);

  useEffect(() => {
    let cancelled = false;
    getLicenseStatus()
      .then((status) => { if (!cancelled) setLicenseInfo(status); })
      .catch(() => { /* license is optional — ignore errors */ });
    return () => { cancelled = true; };
  }, []);

  // Authentication state
  const { state: authState, user, login, logout, setup } = useAuth();
  const location = useLocation();
  const navigate = useNavigate();

  // Loading state (auth only — license never gates the dashboard)
  if (authState === 'loading') {
    return (
      <div className="min-h-screen bg-theme-bg-primary flex items-center justify-center">
        <div className="text-theme-text-secondary text-sm">Loading...</div>
      </div>
    );
  }

  // First-run setup
  if (authState === 'setup_required') {
    return <SetupPage onSetup={setup} />;
  }

  // Login required — redirect to /login preserving intended destination
  if (authState === 'unauthenticated') {
    // If we're already on /login, show the login page
    if (location.pathname === '/login') {
      const handleLogin = async (username: string, password: string): Promise<string | null> => {
        const result = await login(username, password);
        if (result === null) {
          // Login succeeded — navigate to intended destination or /home
          const params = new URLSearchParams(location.search);
          const redirectTo = params.get('redirect') || '/home';
          navigate(redirectTo, { replace: true });
        }
        return result;
      };
      return <LoginPage onLogin={handleLogin} />;
    }

    // Redirect to /login with the current path as redirect param
    const redirectParam = location.pathname !== '/' && location.pathname !== '/login'
      ? `?redirect=${encodeURIComponent(location.pathname)}`
      : '';
    return <Navigate to={`/login${redirectParam}`} replace />;
  }

  // Authenticated — render routes
  return (
    <Routes>
      <Route path="/home" element={<Dashboard onLogout={logout} user={user} licenseInfo={licenseInfo} />} />
      <Route path="/wan-manager" element={
        <UserContext.Provider value={user}>
          <WanManagerPage />
        </UserContext.Provider>
      } />
      <Route path="/login" element={<Navigate to="/home" replace />} />
      <Route path="/" element={<Navigate to="/home" replace />} />
      <Route path="*" element={<Navigate to="/home" replace />} />
    </Routes>
  );
}

// ============================================================================
// Modem Control Menu
// ============================================================================

function ModemControlMenu() {
  const [open, setOpen] = useState(false);
  const [airplaneMode, setAirplaneModeState] = useState<boolean | null>(null);
  const [loading, setLoading] = useState<string | null>(null); // 'power-down' | 'reboot' | 'airplane'
  const [radioWarmup, setRadioWarmup] = useState(false); // Show "radio warming up" after enabling
  const [rebootPending, setRebootPending] = useState(false); // Keep animation until modem reconnects
  const menuRef = useRef<HTMLDivElement>(null);
  const queryClient = useQueryClient();
  const modemId = useActiveModemId();

  // Get modem health from query cache (set by WebSocket events)
  const modemHealth = queryClient.getQueryData<ModemHealth>(modemHealthQueryKey);
  const modemStatus = queryClient.getQueryData<ModemStatus>(modemStatusQueryKey);
  const isOnline = !modemHealth || modemHealth.state === 'ok';
  const healthState = modemHealth?.state ?? 'ok';

  // Fetch airplane mode state when dropdown opens
  useEffect(() => {
    if (!open || !isOnline || !modemId) return;
    let cancelled = false;
    getAirplaneMode(modemId)
      .then(result => { if (!cancelled) setAirplaneModeState(result.airplane_mode); })
      .catch(() => { if (!cancelled) setAirplaneModeState(false); });
    return () => { cancelled = true; };
  }, [open, isOnline, modemId]);

  // Close menu on outside click
  useEffect(() => {
    if (!open) return;
    const handler = (e: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        setOpen(false);
      }
    };
    document.addEventListener('mousedown', handler);
    return () => document.removeEventListener('mousedown', handler);
  }, [open]);

  // Clear loading state when modem health comes back online
  useEffect(() => {
    if (healthState === 'ok' && loading) {
      setLoading(null);
    }
  }, [healthState, loading]);

  // Clear reboot pending when modem status shows connected
  useEffect(() => {
    if (rebootPending && isOnline && modemStatus?.connected) {
      setRebootPending(false);
    }
  }, [rebootPending, isOnline, modemStatus?.connected]);

  // Clear radio warmup after 30s
  useEffect(() => {
    if (!radioWarmup) return;
    const timer = setTimeout(() => setRadioWarmup(false), 30000);
    return () => clearTimeout(timer);
  }, [radioWarmup]);

  const handleGentleReboot = useCallback(async () => {
    if (!modemId) return;
    if (!window.confirm(
      'Gentle reboot the modem (AT+QPOWD)?\n\n' +
      'The modem will power down gracefully and boot back up. ' +
      'This may functionally be the same as the AT+CFUN=1,1 reboot above. ' +
      'The CTRL-Modem interface and network registration may take ~30-60 seconds to fully restore.'
    )) return;
    setLoading('power-down');
    setRebootPending(true);
    try {
      await powerDownModem(modemId);
    } catch (e) {
      console.error('Gentle reboot failed:', e);
      setLoading(null);
      setRebootPending(false);
    }
    setOpen(false);
  }, [modemId]);

  const handleReboot = useCallback(async () => {
    if (!modemId) return;
    if (!window.confirm(
      'Reboot the modem?\n\nThe modem will restart and reconnect automatically. ' +
      'The CTRL-Modem interface and network registration may take ~30-60 seconds to fully restore.'
    )) return;
    setLoading('reboot');
    setRebootPending(true);
    try {
      await rebootModem(modemId);
    } catch (e) {
      console.error('Reboot failed:', e);
      setLoading(null);
      setRebootPending(false);
    }
    setOpen(false);
  }, [modemId]);

  const handleAirplaneToggle = useCallback(async () => {
    if (!modemId) return;
    const newState = !airplaneMode;
    setLoading('airplane');
    try {
      const result = await setAirplaneMode(modemId, newState);
      setAirplaneModeState(result.airplane_mode);
      // If radio was just turned back on, show warmup message
      if (!result.airplane_mode) {
        setRadioWarmup(true);
      } else {
        setRadioWarmup(false);
      }
    } catch (e) {
      console.error('Airplane mode toggle failed:', e);
    }
    setLoading(null);
  }, [modemId, airplaneMode]);

  // Show animation while rebooting/recovering OR while waiting for connection to re-establish
  const showRebootAnimation = !isOnline || rebootPending;
  const isPulsing = healthState === 'rebooting' || rebootPending;

  // Determine button label
  const buttonLabel = healthState === 'rebooting' ? 'Rebooting...'
    : rebootPending ? 'Reconnecting...'
    : healthState === 'unavailable' ? 'Modem Off'
    : healthState === 'error' ? 'Error'
    : null;

  return (
    <div className="relative shrink-0" ref={menuRef}>
      <button
        onClick={() => setOpen(!open)}
        className={clsx(
          'flex items-center gap-1.5 px-2.5 py-1.5 rounded-full text-xs font-medium',
          'transition-colors cursor-pointer',
          showRebootAnimation
            ? 'bg-theme-warning/15 text-theme-warning'
            : 'bg-theme-bg-tertiary text-theme-text-secondary hover:bg-theme-bg-hover'
        )}
        title="Modem control"
      >
        {showRebootAnimation ? (
          <>
            {isPulsing ? (
              <span className="relative flex h-3.5 w-3.5">
                <span className="animate-ping absolute inline-flex h-full w-full rounded-full bg-theme-warning opacity-75" />
                <span className="relative inline-flex items-center justify-center h-3.5 w-3.5">
                  <Power className="w-3 h-3" />
                </span>
              </span>
            ) : (
              <Power className="w-3.5 h-3.5" />
            )}
            <span className="hidden sm:inline">{buttonLabel}</span>
          </>
        ) : (
          <>
            <Power className="w-3.5 h-3.5" />
            <ChevronDown className="w-3 h-3" />
          </>
        )}
      </button>

      {open && (
        <div className="absolute right-0 mt-2 w-56 bg-theme-bg-popover border border-theme-border rounded-lg shadow-lg z-50">
          <div className="p-2 space-y-1">
            {/* Modem health status */}
            {showRebootAnimation && (
              <div className="px-3 py-2 text-xs text-theme-warning bg-theme-warning/10 rounded">
                <div className="flex items-center gap-2">
                  {isPulsing && (
                    <Loader2 className="w-3 h-3 animate-spin shrink-0" />
                  )}
                  <span>
                    {healthState === 'rebooting' ? 'Modem is rebooting — scanning for hardware...'
                      : rebootPending ? 'Modem online — waiting for network registration...'
                      : healthState === 'error' ? (modemHealth?.message ?? 'Modem error')
                      : modemHealth?.message ?? 'Modem unavailable'}
                  </span>
                </div>
              </div>
            )}

            {/* Airplane Mode Toggle */}
            <button
              onClick={handleAirplaneToggle}
              disabled={loading !== null || !isOnline}
              className={clsx(
                'w-full flex items-center gap-3 px-3 py-2 rounded text-sm text-left',
                'transition-colors',
                loading === 'airplane'
                  ? 'opacity-50'
                  : isOnline
                  ? 'hover:bg-theme-bg-hover text-theme-text-primary'
                  : 'opacity-50 cursor-not-allowed text-theme-text-muted'
              )}
            >
              {loading === 'airplane' ? (
                <Loader2 className="w-4 h-4 animate-spin shrink-0" />
              ) : (
                <Plane className={clsx('w-4 h-4 shrink-0', airplaneMode && 'text-theme-accent')} />
              )}
              <div className="flex-1">
                <div className="font-medium">Airplane Mode</div>
                <div className="text-xs text-theme-text-muted">
                  {airplaneMode ? 'Radio OFF' : 'Radio ON'}
                </div>
              </div>
              <div className={clsx(
                'w-8 h-4 rounded-full transition-colors relative',
                airplaneMode === null
                  ? 'bg-theme-bg-tertiary'
                  : !airplaneMode ? 'bg-theme-success' : 'bg-theme-bg-tertiary'
              )}>
                <div className={clsx(
                  'absolute top-0.5 w-3 h-3 rounded-full bg-white transition-transform',
                  airplaneMode ? 'translate-x-0.5' : 'translate-x-4'
                )} />
              </div>
            </button>

            {/* Radio warmup notice */}
            {radioWarmup && (
              <div className="px-3 py-1.5 text-xs text-theme-accent bg-theme-accent/10 rounded flex items-center gap-2">
                <Loader2 className="w-3 h-3 animate-spin shrink-0" />
                <span>CTRL-Modem interface along with network registration may take approximately 30s to show updates again</span>
              </div>
            )}

            <div className="border-t border-theme-border my-1" />

            {/* Reboot */}
            <button
              onClick={handleReboot}
              disabled={loading !== null || !isOnline}
              className={clsx(
                'w-full flex items-center gap-3 px-3 py-2 rounded text-sm text-left',
                'transition-colors',
                loading !== null || !isOnline
                  ? 'opacity-50 cursor-not-allowed text-theme-text-muted'
                  : 'hover:bg-theme-bg-hover text-theme-text-primary'
              )}
            >
              {loading === 'reboot' ? (
                <Loader2 className="w-4 h-4 animate-spin shrink-0" />
              ) : (
                <RotateCcw className="w-4 h-4 shrink-0" />
              )}
              <div>
                <div className="font-medium">Reboot Modem</div>
                <div className="text-xs text-theme-text-muted">AT+CFUN=1,1 — auto-reconnects</div>
              </div>
            </button>

            {/* Gentle Reboot */}
            <button
              onClick={handleGentleReboot}
              disabled={loading !== null || !isOnline}
              className={clsx(
                'w-full flex items-center gap-3 px-3 py-2 rounded text-sm text-left',
                'transition-colors',
                loading !== null || !isOnline
                  ? 'opacity-50 cursor-not-allowed text-theme-text-muted'
                  : 'hover:bg-theme-bg-hover text-theme-text-primary'
              )}
            >
              {loading === 'power-down' ? (
                <Loader2 className="w-4 h-4 animate-spin shrink-0" />
              ) : (
                <Power className="w-4 h-4 shrink-0" />
              )}
              <div>
                <div className="font-medium">Gentle Reboot</div>
                <div className="text-xs text-theme-text-muted">AT+QPOWD — graceful power cycle</div>
              </div>
            </button>
          </div>
        </div>
      )}
    </div>
  );
}

// ============================================================================
// Dashboard
// ============================================================================

/** Authenticated dashboard with sidebar, header, panels, and footer. */
function Dashboard({ onLogout, user, licenseInfo }: { onLogout: () => Promise<void>; user: UserInfo | null; licenseInfo?: LicenseStatus | null }) {
  const { status: wsStatus, reconnect } = useWebSocket({
    onError: (err) => console.error('WebSocket error:', err),
  });

  const { sidebarCollapsed, viewMode, setViewMode } = useUIStore();

  // Sync view presets with server
  usePresetSync();

  return (
    <UserContext.Provider value={user}>
      <div className="min-h-screen bg-theme-bg-primary">
        {/* Sidebar */}
        <Sidebar user={user} onLogout={onLogout} licenseInfo={licenseInfo} />

        {/* Main Content Area - shifts based on sidebar width, gap from floating sidebar */}
        <div
          className={clsx(
            'transition-all duration-300',
            sidebarCollapsed ? 'ml-[4.5rem]' : 'ml-[15.5rem]'
          )}
        >
          {/* Header */}
          <header className="sticky top-0 z-10 mx-2 sm:mx-3 mt-2 sm:mt-3">
            <div className="bg-theme-bg-secondary/80 backdrop-blur-sm border-2 border-theme-border rounded-2xl px-3 sm:px-5 py-1.5 sm:py-2.5">
              <div className="flex flex-wrap items-center gap-2">
                {/* Logo */}
                <div className="flex items-center gap-3 shrink-0">
                  <AgcccLogo size={24} />
                  <div className="hidden sm:block">
                    <h1 className="text-base font-bold text-theme-text-primary leading-tight">
                      CTRL-Modem
                    </h1>
                    <p className="text-caption text-theme-text-muted">
                      Cellular Modem Management
                    </p>
                  </div>
                </div>

                {/* Right-side controls — pushed right on mobile, right on desktop */}
                <div className="flex items-center gap-1.5 sm:gap-2 ml-auto shrink-0 order-1 sm:order-3">
                  {/* Active Modem */}
                  <ActiveModemLabel />

                  {/* Modem Control */}
                  <ModemControlMenu />

                  {/* WebSocket Status */}
                  <button
                    onClick={reconnect}
                    className={clsx(
                      'flex items-center gap-1.5 sm:gap-2 px-2 sm:px-3 py-1 sm:py-1.5 rounded-full text-xs font-medium',
                      'transition-colors cursor-pointer',
                      wsStatus === 'connected'
                        ? 'bg-theme-success/15 text-theme-success'
                        : wsStatus === 'connecting'
                        ? 'bg-theme-warning/15 text-theme-warning'
                        : 'bg-theme-error/15 text-theme-error hover:bg-theme-error/25'
                    )}
                    title={wsStatus !== 'connected' ? 'Click to reconnect' : 'Connected'}
                  >
                    {wsStatus === 'connected' ? (
                      <>
                        <span className="relative flex h-2 w-2">
                          <span className="animate-ping absolute inline-flex h-full w-full rounded-full bg-theme-success opacity-75" />
                          <span className="relative inline-flex rounded-full h-2 w-2 bg-theme-success" />
                        </span>
                        <span className="hidden sm:inline">Live</span>
                      </>
                    ) : wsStatus === 'connecting' ? (
                      <>
                        <div className="w-2 h-2 border border-theme-warning border-t-transparent rounded-full animate-spin" />
                        <span className="hidden sm:inline">Connecting</span>
                      </>
                    ) : (
                      <>
                        <WifiOff className="w-3 h-3" />
                        <span className="hidden sm:inline">Offline</span>
                      </>
                    )}
                  </button>
                </div>

                {/* Center area: presets on the left */}
                <div className="w-full sm:w-auto sm:flex-1 flex items-center gap-2 min-w-0 order-2">
                  {/* View Presets — only shown in dashboard mode */}
                  {viewMode === 'dashboard' && <ViewPresetBar />}

                  {/* Focused Mode label on mobile when in focus mode */}
                  {viewMode === 'focus' && (
                    <span className="sm:hidden text-xs font-medium text-theme-text-secondary uppercase tracking-wider">
                      Focused Mode
                    </span>
                  )}
                </div>

                {/* Dashboards / Focused Mode segmented control — right side, desktop only */}
                <div className="hidden sm:flex items-center bg-theme-bg-tertiary rounded-lg p-0.5 shrink-0 order-2">
                  <button
                    onClick={() => setViewMode('dashboard')}
                    className={clsx(
                      'flex items-center gap-1.5 px-3 py-1 rounded-lg text-xs font-medium transition-colors',
                      'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-theme-accent',
                      viewMode === 'dashboard'
                        ? 'bg-theme-accent text-white'
                        : 'text-theme-text-secondary hover:text-theme-text-primary'
                    )}
                  >
                    <LayoutGrid className="w-3.5 h-3.5" />
                    <span>Dashboards</span>
                  </button>
                  <button
                    onClick={() => setViewMode('focus')}
                    className={clsx(
                      'flex items-center gap-1.5 px-3 py-1 rounded-lg text-xs font-medium transition-colors',
                      'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-theme-accent',
                      viewMode === 'focus'
                        ? 'bg-theme-accent text-white'
                        : 'text-theme-text-secondary hover:text-theme-text-primary'
                    )}
                  >
                    <Maximize2 className="w-3.5 h-3.5" />
                    <span>Focused Mode</span>
                  </button>
                </div>
              </div>
            </div>
          </header>

          {/* Panel Grid */}
          <main className="mx-2 sm:mx-3 mt-2 sm:mt-3 mb-2 sm:mb-3 p-1 rounded-xl min-h-[calc(100vh-8rem)]">
            <PanelGrid />
          </main>

          {/* Footer */}
          <footer className="py-3 mx-2 sm:mx-3 mb-2 sm:mb-3">
            <div className="px-4 sm:px-5">
              <p className="text-center text-xs text-theme-text-muted">
                &copy; 2026 Net Solution Shop LLC &bull; {viewMode === 'dashboard' ? 'Drag panels to reorder' : 'Focused Mode — use sidebar to switch panels'} &bull; Use sidebar to {viewMode === 'dashboard' ? 'show/hide' : 'navigate'}
              </p>
            </div>
          </footer>
        </div>
      </div>
    </UserContext.Provider>
  );
}

export default App;
