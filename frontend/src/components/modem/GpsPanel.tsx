/**
 * GpsPanel Component
 *
 * Standalone panel for GPS position data.
 * Polls /api/modem/gps at 10s intervals when active.
 * User can start/stop the GPS engine via AT+QGPS/AT+QGPSEND.
 * Shows position, altitude, satellites, speed, HDOP, and fix status.
 */

import { useState } from 'react';
import { useGps, useActiveModemId } from '@/hooks';
import { stopGps } from '@/api/modem';
import { Navigation, Satellite, Play, Square } from 'lucide-react';

export function GpsPanel() {
  const modemId = useActiveModemId();
  const [gpsActive, setGpsActive] = useState(false);
  const [stopping, setStopping] = useState(false);

  const { data: gpsData, isLoading } = useGps({
    enabled: gpsActive,
    refreshInterval: 10000,
  });

  const handleStart = () => {
    setGpsActive(true);
  };

  const handleStop = async () => {
    if (!modemId) return;
    setStopping(true);
    try {
      await stopGps(modemId);
    } catch {
      // Stop polling regardless
    }
    setGpsActive(false);
    setStopping(false);
  };

  const hasFix = gpsData?.fix_type != null && gpsData.fix_type !== 'none';

  return (
    <div className="space-y-3">
      {/* Controls */}
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-2">
          {gpsActive && (
            <>
              <div className={`w-2.5 h-2.5 rounded-full ${hasFix ? 'bg-theme-success' : 'bg-theme-warning animate-pulse'}`} />
              <span className="text-sm text-theme-text-secondary">
                {hasFix ? 'GPS Fix Acquired' : 'Acquiring fix...'}
              </span>
              {hasFix && gpsData?.fix_type && (
                <span className="text-xs text-theme-text-muted">
                  ({gpsData.fix_type})
                </span>
              )}
            </>
          )}
          {!gpsActive && (
            <span className="text-sm text-theme-text-muted">GPS stopped</span>
          )}
        </div>
        <button
          onClick={gpsActive ? handleStop : handleStart}
          disabled={stopping}
          className={`flex items-center gap-1.5 text-xs ${
            gpsActive
              ? 'btn-danger px-3 py-1.5'
              : 'btn-success px-3 py-1.5'
          }`}
        >
          {gpsActive ? (
            <>
              <Square className="w-3 h-3" />
              {stopping ? 'Stopping...' : 'Stop'}
            </>
          ) : (
            <>
              <Play className="w-3 h-3" />
              Start
            </>
          )}
        </button>
      </div>

      {/* Content only shown when active */}
      {gpsActive && (
        <>
          {isLoading && !gpsData ? (
            <div className="loading-state py-4">
              <div className="loading-spinner-sm" />
              <span>Starting GPS engine...</span>
            </div>
          ) : (
            <>
              {/* Position */}
              <div className="bg-theme-bg-primary rounded-lg p-3">
                <div className="flex items-center gap-1 text-xs text-theme-text-secondary mb-1">
                  <Navigation className="w-3 h-3" />
                  Position
                </div>
                <div className="text-sm font-mono text-theme-text-primary">
                  {hasFix
                    ? `${gpsData!.latitude.toFixed(6)}, ${gpsData!.longitude.toFixed(6)}`
                    : 'Waiting for satellites...'}
                </div>
              </div>

              {/* Metrics Grid */}
              <div className="grid grid-cols-2 gap-2">
                <div className="bg-theme-bg-primary rounded-lg p-3">
                  <div className="text-xs text-theme-text-secondary">Altitude</div>
                  <div className="text-sm text-theme-text-primary">
                    {gpsData?.altitude != null ? `${gpsData.altitude.toFixed(1)} m` : '--'}
                  </div>
                </div>
                <div className="bg-theme-bg-primary rounded-lg p-3">
                  <div className="flex items-center gap-1 text-xs text-theme-text-secondary">
                    <Satellite className="w-3 h-3" />
                    Satellites
                  </div>
                  <div className="text-sm text-theme-text-primary">
                    {gpsData?.satellites ?? '--'}
                  </div>
                </div>
                <div className="bg-theme-bg-primary rounded-lg p-3">
                  <div className="text-xs text-theme-text-secondary">Speed</div>
                  <div className="text-sm text-theme-text-primary">
                    {gpsData?.speed != null ? `${gpsData.speed.toFixed(1)} km/h` : '--'}
                  </div>
                </div>
              </div>

              {/* Timestamp */}
              {gpsData?.timestamp && (
                <div className="text-xs text-theme-text-muted text-center">
                  {new Date(gpsData.timestamp).toLocaleString()}
                </div>
              )}
            </>
          )}
        </>
      )}
    </div>
  );
}
