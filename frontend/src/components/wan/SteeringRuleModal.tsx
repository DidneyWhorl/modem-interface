/**
 * SteeringRuleModal Component
 *
 * Modal dialog for creating or editing a traffic steering rule.
 * Progressive disclosure: match conditions start hidden and are added on demand.
 */

import { useState, useEffect, useMemo, useCallback } from 'react';
import { X, Plus } from 'lucide-react';
import { useCreateSteeringRule, useUpdateSteeringRule } from '@/hooks/mutations/useSteeringRules';
import type {
  SteeringRule,
  Protocol,
  PortMatch,
  FailoverMode,
  CreateSteeringRuleRequest,
  UpdateSteeringRuleRequest,
} from '@/types/steering';
import type { WanModemStatusEntry } from '@/types/api';

// ============================================================================
// Types
// ============================================================================

interface SteeringRuleModalProps {
  rule: SteeringRule | null; // null = add mode, SteeringRule = edit mode
  onClose: () => void;
  wanModems: WanModemStatusEntry[];
}

type ConditionType = 'source_ip' | 'destination_ip' | 'protocol' | 'destination_port' | 'source_port';

interface ConditionDef {
  key: ConditionType;
  label: string;
}

const ALL_CONDITIONS: ConditionDef[] = [
  { key: 'source_ip', label: 'From device/network' },
  { key: 'destination_ip', label: 'To destination' },
  { key: 'protocol', label: 'Protocol' },
  { key: 'destination_port', label: 'Port' },
  { key: 'source_port', label: 'Source port' },
];

const FAILOVER_OPTIONS: { value: FailoverMode; label: string; description: string }[] = [
  {
    value: 'automatic',
    label: 'Automatic',
    description: 'If this WAN goes down, traffic uses the next available WAN',
  },
  {
    value: 'preferred_fallback',
    label: 'Preferred Fallback',
    description: 'If this WAN goes down, traffic switches to a specific backup WAN',
  },
  {
    value: 'strict',
    label: 'Strict',
    description: 'If this WAN goes down, this traffic is blocked',
  },
];

// ============================================================================
// Helpers
// ============================================================================

function portMatchToString(pm: PortMatch | null | undefined): string {
  if (pm == null) return '';
  if (typeof pm === 'number') return String(pm);
  return `${pm[0]}-${pm[1]}`;
}

function parsePort(raw: string): { value: PortMatch | null; error: string | null } {
  const trimmed = raw.trim();
  if (!trimmed) return { value: null, error: null };

  if (trimmed.includes('-')) {
    const parts = trimmed.split('-');
    if (parts.length !== 2) return { value: null, error: 'Invalid range format' };
    const start = Number(parts[0]);
    const end = Number(parts[1]);
    if (!Number.isInteger(start) || !Number.isInteger(end))
      return { value: null, error: 'Ports must be integers' };
    if (start < 1 || start > 65535 || end < 1 || end > 65535)
      return { value: null, error: 'Ports must be 1-65535' };
    if (start > end)
      return { value: null, error: 'Range start must not exceed end' };
    return { value: [start, end], error: null };
  }

  const port = Number(trimmed);
  if (!Number.isInteger(port)) return { value: null, error: 'Port must be an integer' };
  if (port < 1 || port > 65535) return { value: null, error: 'Port must be 1-65535' };
  return { value: port, error: null };
}

function isValidCidr(value: string): boolean {
  const trimmed = value.trim();
  if (!trimmed) return true;
  // Simple validation: IPv4 address or CIDR
  const cidrPattern = /^(\d{1,3}\.){3}\d{1,3}(\/\d{1,2})?$/;
  // IPv6 basic check
  const ipv6Pattern = /^[0-9a-fA-F:.]+(\/\d{1,3})?$/;
  return cidrPattern.test(trimmed) || ipv6Pattern.test(trimmed);
}

function showCidrHint(value: string): boolean {
  const trimmed = value.trim();
  if (!trimmed) return false;
  // Show hint if user typed an IP without /
  return /^(\d{1,3}\.){3}\d{1,3}$/.test(trimmed);
}

// ============================================================================
// Component
// ============================================================================

export default function SteeringRuleModal({ rule, onClose, wanModems }: SteeringRuleModalProps) {
  const isEdit = rule !== null;

  const createMut = useCreateSteeringRule();
  const updateMut = useUpdateSteeringRule();

  // Form state
  const [name, setName] = useState(rule?.name ?? '');
  const [targetWan, setTargetWan] = useState(rule?.target_wan ?? '');
  const [failoverMode, setFailoverMode] = useState<FailoverMode>(rule?.failover_mode ?? 'automatic');
  const [fallbackWan, setFallbackWan] = useState(rule?.fallback_wan ?? '');

  // Match conditions — multi-value IP arrays
  const [sourceIps, setSourceIps] = useState<string[]>(rule?.source_ip ?? []);
  const [sourceIpInput, setSourceIpInput] = useState('');
  const [destinationIps, setDestinationIps] = useState<string[]>(rule?.destination_ip ?? []);
  const [destinationIpInput, setDestinationIpInput] = useState('');
  const [protocol, setProtocol] = useState<Protocol | ''>(rule?.protocol ?? '');
  const [destinationPort, setDestinationPort] = useState(portMatchToString(rule?.destination_port));
  const [sourcePort, setSourcePort] = useState(portMatchToString(rule?.source_port));

  // Track which conditions are visible
  const [activeConditions, setActiveConditions] = useState<Set<ConditionType>>(() => {
    const initial = new Set<ConditionType>();
    if (rule?.source_ip && rule.source_ip.length > 0) initial.add('source_ip');
    if (rule?.destination_ip && rule.destination_ip.length > 0) initial.add('destination_ip');
    if (rule?.protocol) initial.add('protocol');
    if (rule?.destination_port != null) initial.add('destination_port');
    if (rule?.source_port != null) initial.add('source_port');
    return initial;
  });

  // Condition add menu
  const [showConditionMenu, setShowConditionMenu] = useState(false);

  // Validation errors
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [attempted, setAttempted] = useState(false);

  // Default target WAN to first available if none set
  useEffect(() => {
    if (!targetWan && wanModems.length > 0) {
      setTargetWan(wanModems[0]!.modem_id);
    }
  }, [targetWan, wanModems]);

  const availableConditions = useMemo(
    () => ALL_CONDITIONS.filter(c => !activeConditions.has(c.key)),
    [activeConditions],
  );

  const fallbackOptions = useMemo(
    () => wanModems.filter(m => m.modem_id !== targetWan),
    [wanModems, targetWan],
  );

  // Clear fallback when target changes and it conflicts
  useEffect(() => {
    if (fallbackWan === targetWan) {
      setFallbackWan('');
    }
  }, [targetWan, fallbackWan]);

  const addCondition = useCallback((key: ConditionType) => {
    setActiveConditions(prev => new Set(prev).add(key));
    setShowConditionMenu(false);
  }, []);

  const removeCondition = useCallback((key: ConditionType) => {
    setActiveConditions(prev => {
      const next = new Set(prev);
      next.delete(key);
      return next;
    });
    // Clear the value
    switch (key) {
      case 'source_ip': setSourceIps([]); setSourceIpInput(''); break;
      case 'destination_ip': setDestinationIps([]); setDestinationIpInput(''); break;
      case 'protocol': setProtocol(''); break;
      case 'destination_port': setDestinationPort(''); break;
      case 'source_port': setSourcePort(''); break;
    }
  }, []);

  const validate = useCallback((): Record<string, string> => {
    const errs: Record<string, string> = {};

    if (!name.trim()) errs.name = 'Name is required';
    if (!targetWan) errs.targetWan = 'Target WAN is required';

    if (activeConditions.has('source_ip')) {
      for (const ip of sourceIps) {
        if (!isValidCidr(ip)) {
          errs.sourceIp = `Invalid IP or CIDR notation: ${ip}`;
          break;
        }
      }
    }
    if (activeConditions.has('destination_ip')) {
      for (const ip of destinationIps) {
        if (!isValidCidr(ip)) {
          errs.destinationIp = `Invalid IP or CIDR notation: ${ip}`;
          break;
        }
      }
    }

    if (activeConditions.has('destination_port')) {
      const parsed = parsePort(destinationPort);
      if (parsed.error) errs.destinationPort = parsed.error;
      if (destinationPort.trim() && !protocol) {
        errs.protocol = 'Protocol is required when a port is specified';
        if (!activeConditions.has('protocol')) {
          // Auto-show protocol condition
          setActiveConditions(prev => new Set(prev).add('protocol'));
        }
      }
    }
    if (activeConditions.has('source_port')) {
      const parsed = parsePort(sourcePort);
      if (parsed.error) errs.sourcePort = parsed.error;
      if (sourcePort.trim() && !protocol) {
        errs.protocol = 'Protocol is required when a port is specified';
        if (!activeConditions.has('protocol')) {
          setActiveConditions(prev => new Set(prev).add('protocol'));
        }
      }
    }

    if (failoverMode === 'preferred_fallback' && !fallbackWan) {
      errs.fallbackWan = 'Fallback WAN is required';
    }

    return errs;
  }, [
    name, targetWan, sourceIps, destinationIps, protocol,
    destinationPort, sourcePort, failoverMode, fallbackWan, activeConditions,
  ]);

  const handleSave = useCallback(() => {
    setAttempted(true);
    const errs = validate();
    setErrors(errs);
    if (Object.keys(errs).length > 0) return;

    const dpParsed = parsePort(destinationPort);
    const spParsed = parsePort(sourcePort);

    const srcIpValue = activeConditions.has('source_ip') && sourceIps.length > 0 ? sourceIps : null;
    const dstIpValue = activeConditions.has('destination_ip') && destinationIps.length > 0 ? destinationIps : null;

    if (isEdit && rule) {
      const req: UpdateSteeringRuleRequest = {
        name: name.trim(),
        source_ip: srcIpValue,
        destination_ip: dstIpValue,
        protocol: activeConditions.has('protocol') && protocol ? protocol as Protocol : null,
        destination_port: activeConditions.has('destination_port') ? dpParsed.value : null,
        source_port: activeConditions.has('source_port') ? spParsed.value : null,
        target_wan: targetWan,
        failover_mode: failoverMode,
        fallback_wan: failoverMode === 'preferred_fallback' ? fallbackWan : null,
      };
      updateMut.mutate({ id: rule.id, req }, { onSuccess: () => onClose() });
    } else {
      const req: CreateSteeringRuleRequest = {
        name: name.trim(),
        source_ip: srcIpValue,
        destination_ip: dstIpValue,
        protocol: activeConditions.has('protocol') && protocol ? protocol as Protocol : null,
        destination_port: activeConditions.has('destination_port') ? dpParsed.value : null,
        source_port: activeConditions.has('source_port') ? spParsed.value : null,
        target_wan: targetWan,
        failover_mode: failoverMode,
        fallback_wan: failoverMode === 'preferred_fallback' ? fallbackWan : null,
      };
      createMut.mutate(req, { onSuccess: () => onClose() });
    }
  }, [
    validate, isEdit, rule, name, sourceIps, destinationIps, protocol,
    destinationPort, sourcePort, targetWan, failoverMode, fallbackWan,
    activeConditions, createMut, updateMut, onClose,
  ]);

  // Re-validate on change when user has already attempted save
  useEffect(() => {
    if (attempted) {
      setErrors(validate());
    }
  }, [attempted, validate]);

  const isSaving = createMut.isPending || updateMut.isPending;
  const mutError = createMut.error || updateMut.error;

  const wanLabel = (modem: WanModemStatusEntry) =>
    modem.operator ? `${modem.label} (${modem.operator})` : modem.label;

  // ---- Render ----

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50">
      <div className="bg-theme-bg-primary border border-theme-border rounded-lg shadow-xl w-full max-w-lg mx-4 max-h-[90vh] overflow-y-auto">
        {/* Header */}
        <div className="flex items-center justify-between px-5 py-3 border-b border-theme-border sticky top-0 bg-theme-bg-primary z-10">
          <h3 className="text-lg font-medium text-theme-text-primary">
            {isEdit ? 'Edit Steering Rule' : 'Add Steering Rule'}
          </h3>
          <button
            onClick={onClose}
            className="p-1 text-theme-text-muted hover:text-theme-text-primary"
          >
            <X className="w-5 h-5" />
          </button>
        </div>

        {/* Content */}
        <div className="px-5 py-4 space-y-4">
          {/* Name */}
          <FieldGroup label="Name" error={errors.name}>
            <input
              type="text"
              value={name}
              onChange={e => setName(e.target.value)}
              placeholder="e.g., VoIP to AT&T"
              className={fieldClass(errors.name)}
            />
          </FieldGroup>

          {/* Target WAN */}
          <FieldGroup label="Target WAN" error={errors.targetWan}>
            <select
              value={targetWan}
              onChange={e => setTargetWan(e.target.value)}
              className={fieldClass(errors.targetWan)}
            >
              <option value="" disabled>Select a WAN</option>
              {wanModems.map(m => (
                <option key={m.modem_id} value={m.modem_id}>
                  {wanLabel(m)}
                </option>
              ))}
            </select>
          </FieldGroup>

          {/* Match Conditions */}
          <div>
            <label className="text-sm font-medium text-theme-text-secondary block mb-1.5">
              Match Conditions
            </label>

            {activeConditions.size === 0 && (
              <p className="text-xs text-theme-text-muted mb-2">Matches all traffic</p>
            )}

            <div className="space-y-2">
              {activeConditions.has('source_ip') && (
                <ConditionRow
                  label="From device/network"
                  error={errors.sourceIp}
                  onRemove={() => removeCondition('source_ip')}
                >
                  <IpChipInput
                    values={sourceIps}
                    inputValue={sourceIpInput}
                    onValuesChange={setSourceIps}
                    onInputChange={setSourceIpInput}
                    placeholder="192.168.1.100 or 192.168.1.0/24"
                    error={errors.sourceIp}
                  />
                </ConditionRow>
              )}

              {activeConditions.has('destination_ip') && (
                <ConditionRow
                  label="To destination"
                  error={errors.destinationIp}
                  onRemove={() => removeCondition('destination_ip')}
                >
                  <IpChipInput
                    values={destinationIps}
                    inputValue={destinationIpInput}
                    onValuesChange={setDestinationIps}
                    onInputChange={setDestinationIpInput}
                    placeholder="10.0.0.0/8 or 203.0.113.50"
                    error={errors.destinationIp}
                  />
                </ConditionRow>
              )}

              {activeConditions.has('protocol') && (
                <ConditionRow
                  label="Protocol"
                  error={errors.protocol}
                  onRemove={() => removeCondition('protocol')}
                >
                  <select
                    value={protocol}
                    onChange={e => setProtocol(e.target.value as Protocol | '')}
                    className={fieldClass(errors.protocol, true)}
                  >
                    <option value="">Any</option>
                    <option value="tcp">TCP</option>
                    <option value="udp">UDP</option>
                    <option value="icmp">ICMP</option>
                  </select>
                </ConditionRow>
              )}

              {activeConditions.has('destination_port') && (
                <ConditionRow
                  label="Port"
                  error={errors.destinationPort}
                  onRemove={() => removeCondition('destination_port')}
                >
                  <input
                    type="text"
                    value={destinationPort}
                    onChange={e => setDestinationPort(e.target.value)}
                    placeholder="443 or 5060-5080"
                    className={fieldClass(errors.destinationPort, true)}
                  />
                </ConditionRow>
              )}

              {activeConditions.has('source_port') && (
                <ConditionRow
                  label="Source port"
                  error={errors.sourcePort}
                  onRemove={() => removeCondition('source_port')}
                >
                  <input
                    type="text"
                    value={sourcePort}
                    onChange={e => setSourcePort(e.target.value)}
                    placeholder="1024-65535"
                    className={fieldClass(errors.sourcePort, true)}
                  />
                </ConditionRow>
              )}
            </div>

            {/* Add condition button */}
            {availableConditions.length > 0 && (
              <div className="relative mt-2">
                <button
                  type="button"
                  onClick={() => setShowConditionMenu(prev => !prev)}
                  className="flex items-center gap-1 text-xs text-theme-accent hover:text-theme-accent/80"
                >
                  <Plus className="w-3.5 h-3.5" />
                  Add condition
                </button>

                {showConditionMenu && (
                  <div className="absolute left-0 top-full mt-1 bg-theme-bg-secondary border border-theme-border rounded shadow-lg py-1 z-20 min-w-[180px]">
                    {availableConditions.map(c => (
                      <button
                        key={c.key}
                        onClick={() => addCondition(c.key)}
                        className="block w-full text-left px-3 py-1.5 text-xs text-theme-text-primary hover:bg-theme-bg-tertiary"
                      >
                        {c.label}
                      </button>
                    ))}
                  </div>
                )}
              </div>
            )}
          </div>

          {/* Failover Mode */}
          <div>
            <label className="text-sm font-medium text-theme-text-secondary block mb-1.5">
              Failover Mode
            </label>
            <div className="space-y-2">
              {FAILOVER_OPTIONS.map(opt => (
                <label
                  key={opt.value}
                  className={`flex items-start gap-2.5 p-2.5 rounded border cursor-pointer transition-colors ${
                    failoverMode === opt.value
                      ? 'border-theme-accent/50 bg-theme-accent/5'
                      : 'border-theme-border hover:border-theme-border-hover'
                  }`}
                >
                  <input
                    type="radio"
                    name="failover_mode"
                    value={opt.value}
                    checked={failoverMode === opt.value}
                    onChange={() => setFailoverMode(opt.value)}
                    className="mt-0.5 accent-theme-accent"
                  />
                  <div>
                    <span className="text-sm font-medium text-theme-text-primary">
                      {opt.label}
                    </span>
                    <p className="text-xs text-theme-text-muted mt-0.5">
                      {opt.description}
                    </p>
                  </div>
                </label>
              ))}
            </div>

            {/* Fallback WAN selector */}
            {failoverMode === 'preferred_fallback' && (
              <div className="mt-2 pl-6">
                <FieldGroup label="Fallback WAN" error={errors.fallbackWan}>
                  <select
                    value={fallbackWan}
                    onChange={e => setFallbackWan(e.target.value)}
                    className={fieldClass(errors.fallbackWan)}
                  >
                    <option value="" disabled>Select fallback WAN</option>
                    {fallbackOptions.map(m => (
                      <option key={m.modem_id} value={m.modem_id}>
                        {wanLabel(m)}
                      </option>
                    ))}
                  </select>
                </FieldGroup>
              </div>
            )}
          </div>

          {/* Mutation error */}
          {mutError && (
            <p className="text-sm text-theme-error">
              {mutError.message}
            </p>
          )}
        </div>

        {/* Footer */}
        <div className="flex justify-end gap-2 px-5 py-3 border-t border-theme-border">
          <button
            type="button"
            onClick={onClose}
            disabled={isSaving}
            className="px-4 py-2 text-sm font-medium text-theme-text-secondary border border-theme-border rounded hover:bg-theme-bg-secondary transition-colors disabled:opacity-50"
          >
            Cancel
          </button>
          <button
            type="button"
            onClick={handleSave}
            disabled={isSaving}
            className="px-4 py-2 text-sm font-medium text-white bg-theme-accent rounded hover:opacity-90 transition-opacity disabled:opacity-50"
          >
            {isSaving ? 'Saving...' : 'Save'}
          </button>
        </div>
      </div>
    </div>
  );
}

// ============================================================================
// Sub-components
// ============================================================================

function FieldGroup({
  label,
  error,
  children,
}: {
  label: string;
  error?: string;
  children: React.ReactNode;
}) {
  return (
    <div>
      <label className="text-sm font-medium text-theme-text-secondary block mb-1">
        {label}
      </label>
      {children}
      {error && <p className="text-xs text-theme-error mt-0.5">{error}</p>}
    </div>
  );
}

function ConditionRow({
  label,
  error,
  onRemove,
  children,
}: {
  label: string;
  error?: string;
  onRemove: () => void;
  children: React.ReactNode;
}) {
  return (
    <div className="flex items-start gap-2">
      <div className="flex-1">
        <div className="flex items-center justify-between mb-0.5">
          <span className="text-xs text-theme-text-muted">{label}</span>
          <button
            type="button"
            onClick={onRemove}
            className="p-0.5 text-theme-text-muted hover:text-theme-error"
          >
            <X className="w-3.5 h-3.5" />
          </button>
        </div>
        {children}
        {error && <p className="text-xs text-theme-error mt-0.5">{error}</p>}
      </div>
    </div>
  );
}

function fieldClass(error?: string, compact = false): string {
  const base = `w-full rounded border bg-theme-bg-secondary text-theme-text-primary placeholder:text-theme-text-muted text-sm ${
    compact ? 'px-2 py-1' : 'px-3 py-1.5'
  }`;
  return error
    ? `${base} border-theme-error focus:outline-none focus:ring-1 focus:ring-theme-error`
    : `${base} border-theme-border focus:outline-none focus:ring-1 focus:ring-theme-accent`;
}

function IpChipInput({
  values,
  inputValue,
  onValuesChange,
  onInputChange,
  placeholder,
  error,
}: {
  values: string[];
  inputValue: string;
  onValuesChange: (v: string[]) => void;
  onInputChange: (v: string) => void;
  placeholder: string;
  error?: string;
}) {
  const addValue = (raw: string) => {
    const trimmed = raw.trim();
    if (!trimmed) return;
    if (!isValidCidr(trimmed)) return;
    if (values.includes(trimmed)) return;
    onValuesChange([...values, trimmed]);
    onInputChange('');
  };

  const removeValue = (index: number) => {
    onValuesChange(values.filter((_, i) => i !== index));
  };

  const handleKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'Enter' || e.key === ',') {
      e.preventDefault();
      addValue(inputValue);
    } else if (e.key === 'Backspace' && !inputValue && values.length > 0) {
      removeValue(values.length - 1);
    }
  };

  const borderClass = error
    ? 'border-theme-error focus-within:ring-1 focus-within:ring-theme-error'
    : 'border-theme-border focus-within:ring-1 focus-within:ring-theme-accent';

  return (
    <div>
      <div className={`flex flex-wrap items-center gap-1 rounded border bg-theme-bg-secondary px-2 py-1 ${borderClass}`}>
        {values.map((ip, i) => (
          <span
            key={i}
            className="inline-flex items-center gap-0.5 rounded bg-theme-accent/15 text-theme-accent px-1.5 py-0.5 text-caption"
          >
            {ip}
            <button
              type="button"
              onClick={() => removeValue(i)}
              className="ml-0.5 hover:text-theme-error"
            >
              <X className="w-3 h-3" />
            </button>
          </span>
        ))}
        <input
          type="text"
          value={inputValue}
          onChange={e => onInputChange(e.target.value)}
          onKeyDown={handleKeyDown}
          onBlur={() => addValue(inputValue)}
          placeholder={values.length === 0 ? placeholder : ''}
          className="flex-1 min-w-[120px] bg-transparent text-sm text-theme-text-primary placeholder:text-theme-text-muted outline-none border-none py-0"
        />
      </div>
      {showCidrHint(inputValue) && (
        <p className="text-[10px] text-theme-text-muted mt-0.5">
          To match a network, use CIDR notation (e.g., /24). Press Enter to add.
        </p>
      )}
    </div>
  );
}
