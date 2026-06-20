import { useCallback, useEffect, useState } from 'react';
import {
  getTunnelConfig,
  updateTunnelConfig,
  type TunnelConfigResponse,
} from '@/api/tunnel';

interface TunnelConfigProps {
  licenseInfo?: { state: string; features?: string[] } | null;
}

export function TunnelConfig({ licenseInfo }: TunnelConfigProps) {
  const [config, setConfig] = useState<TunnelConfigResponse | null>(null);
  const [portInput, setPortInput] = useState('');
  const [loading, setLoading] = useState(false);

  const hasFeature = config?.feature_available ?? false;

  useEffect(() => {
    let cancelled = false;
    getTunnelConfig()
      .then((c) => { if (!cancelled) { setConfig(c); setPortInput(c.ports.join(', ')); } })
      .catch(() => {});
    return () => { cancelled = true; };
  }, []);

  const handleToggle = useCallback(async () => {
    if (!config || loading) return;
    setLoading(true);
    try {
      const updated = await updateTunnelConfig({ enabled: !config.enabled });
      setConfig(updated);
    } catch { /* silent */ }
    setLoading(false);
  }, [config, loading]);

  const handleSavePorts = useCallback(async () => {
    if (!config || loading) return;
    const ports = portInput
      .split(',')
      .map((s) => parseInt(s.trim(), 10))
      .filter((n) => !isNaN(n) && n > 0 && n <= 65535);
    if (ports.length === 0) return;

    setLoading(true);
    try {
      const updated = await updateTunnelConfig({ ports });
      setConfig(updated);
      setPortInput(updated.ports.join(', '));
    } catch { /* silent */ }
    setLoading(false);
  }, [config, portInput, loading]);

  // licenseInfo is used by the parent to decide whether to render this component,
  // but we keep the prop for future feature-flag gating at this level if needed.
  void licenseInfo;

  if (!config) return null;

  const hasNonDefault = config.ports.some((p) => p !== 443 && p !== 8443);

  return (
    <div className="space-y-3">
      <div className="flex items-center justify-between">
        <div>
          <div className="text-sm font-medium text-theme-text-primary">Remote Access</div>
          <div className="text-xs text-theme-text-secondary">
            {hasFeature
              ? 'Allow remote access via portal'
              : 'Requires remote_access license'}
          </div>
        </div>
        <button
          className={`btn-secondary text-xs ${!hasFeature ? 'opacity-50 cursor-not-allowed' : ''}`}
          onClick={handleToggle}
          disabled={!hasFeature || loading}
        >
          {config.enabled ? 'Enabled' : 'Disabled'}
        </button>
      </div>

      {hasFeature && config.enabled && (
        <div className="space-y-2 pl-2 border-l-2 border-theme-border">
          <div>
            <label className="text-xs text-theme-text-secondary">Allowed Ports</label>
            <div className="flex gap-2 mt-1">
              <input
                className="input-compact flex-1"
                value={portInput}
                onChange={(e) => setPortInput(e.target.value)}
                placeholder="443, 8443"
              />
              <button className="btn-secondary text-xs" onClick={handleSavePorts} disabled={loading}>
                Save
              </button>
            </div>
          </div>
          {hasNonDefault && (
            <div className="text-xs text-amber-500 bg-amber-500/10 rounded px-2 py-1">
              Non-default ports are exposed. Only expose services you trust.
            </div>
          )}
        </div>
      )}
    </div>
  );
}
