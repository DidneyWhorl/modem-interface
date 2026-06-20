/**
 * ApnFormFields
 *
 * Core APN form fields: APN string, IP type, Auth type, and CID selector.
 * Stateless — parent owns all values and change handlers.
 */

import type { AuthType, IpType } from '@/types/api';

export interface ApnFormValues {
  cid: number;
  apn: string;
  ip_type: IpType;
  auth_type: AuthType;
}

interface ApnFormFieldsProps {
  values: ApnFormValues;
  onChange: <K extends keyof ApnFormValues>(field: K, value: ApnFormValues[K]) => void;
  disabled: boolean;
  apnError?: string;
}

export function ApnFormFields({ values, onChange, disabled, apnError }: ApnFormFieldsProps) {
  return (
    <div className="space-y-4">
      {/* APN + Context ID (CID) row — APN grows, CID is a narrow control */}
      <div className="flex flex-col sm:flex-row sm:items-start gap-3">
        {/* APN */}
        <div className="flex-1 min-w-0">
          <label
            htmlFor="apn-editor-apn"
            className="block text-sm font-semibold text-theme-text-secondary mb-1"
          >
            APN
          </label>
          <input
            id="apn-editor-apn"
            type="text"
            value={values.apn}
            onChange={(e) => onChange('apn', e.target.value)}
            placeholder="e.g., internet, fast.t-mobile.com"
            disabled={disabled}
            maxLength={100}
            className="input w-full"
            aria-describedby={apnError ? 'apn-editor-apn-error' : undefined}
            aria-invalid={!!apnError}
          />
          {apnError && (
            <p id="apn-editor-apn-error" className="mt-1 text-xs text-theme-error">
              {apnError}
            </p>
          )}
        </div>

        {/* Context ID (CID) */}
        <div className="sm:w-32 shrink-0">
          <label
            htmlFor="apn-editor-cid"
            className="block text-sm font-semibold text-theme-text-secondary mb-1"
            title="PDP context ID — most carriers use 1, Verizon uses 3"
          >
            Context ID (CID)
          </label>
          <select
            id="apn-editor-cid"
            value={values.cid}
            onChange={(e) => onChange('cid', Number(e.target.value))}
            disabled={disabled}
            className="select w-full"
          >
            {[1, 2, 3, 4, 5, 6, 7, 8].map((n) => (
              <option key={n} value={n}>{n}</option>
            ))}
          </select>
          <p className="mt-1 text-caption text-theme-text-muted">
            Most carriers use CID 1. Verizon uses CID 3.
          </p>
        </div>
      </div>

      {/* IP Type + Auth Type row */}
      <div className="grid grid-cols-2 gap-3">
        <div>
          <label
            htmlFor="apn-editor-ip-type"
            className="block text-sm font-semibold text-theme-text-secondary mb-1"
          >
            IP Type
          </label>
          <select
            id="apn-editor-ip-type"
            value={values.ip_type}
            onChange={(e) => onChange('ip_type', e.target.value as IpType)}
            disabled={disabled}
            className="select w-full"
          >
            <option value="ipv4">IPv4</option>
            <option value="ipv6">IPv6</option>
            <option value="ipv4v6">IPv4v6</option>
          </select>
        </div>

        <div>
          <label
            htmlFor="apn-editor-auth"
            className="block text-sm font-semibold text-theme-text-secondary mb-1"
          >
            Auth Type
          </label>
          <select
            id="apn-editor-auth"
            value={values.auth_type}
            onChange={(e) => onChange('auth_type', e.target.value as AuthType)}
            disabled={disabled}
            className="select w-full"
          >
            <option value="none">None</option>
            <option value="pap">PAP</option>
            <option value="chap">CHAP</option>
          </select>
        </div>
      </div>
    </div>
  );
}
