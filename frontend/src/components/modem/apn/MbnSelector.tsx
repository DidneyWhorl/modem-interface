/**
 * MbnSelector
 *
 * MBN carrier profile dropdown.
 * - When mbn_supported=false: disabled, shows "Not supported on this modem".
 * - Options: "Auto" + each mbn_profiles[].name.
 * - Maps to the request's mbn_profile field:
 *     "Auto" -> "__auto__"
 *     a profile name -> that string
 */

import type { MbnProfile } from '@/types/api';

export const MBN_AUTO_VALUE = '__auto__';

interface MbnSelectorProps {
  profiles: MbnProfile[];
  supported: boolean;
  value: string;
  onChange: (value: string) => void;
  disabled: boolean;
}

export function MbnSelector({
  profiles,
  supported,
  value,
  onChange,
  disabled,
}: MbnSelectorProps) {
  return (
    <div>
      <label
        htmlFor="apn-editor-mbn"
        className="block text-sm font-semibold text-theme-text-secondary mb-1"
      >
        Carrier Profile (MBN)
      </label>
      {!supported ? (
        <div
          id="apn-editor-mbn"
          className="input flex items-center text-theme-text-muted cursor-not-allowed opacity-60"
          aria-disabled="true"
          role="combobox"
          aria-expanded="false"
        >
          Not supported on this modem
        </div>
      ) : (
        <select
          id="apn-editor-mbn"
          value={value}
          onChange={(e) => onChange(e.target.value)}
          disabled={disabled}
          className="select w-full"
        >
          <option value={MBN_AUTO_VALUE}>Auto</option>
          {profiles.map((p) => (
            <option key={p.index} value={p.name}>
              {p.name}
            </option>
          ))}
        </select>
      )}
      <p className="mt-1 text-caption text-theme-text-muted">
        {supported
          ? 'Changing the carrier profile requires a modem reboot.'
          : 'This modem does not support carrier profile selection.'}
      </p>
    </div>
  );
}
