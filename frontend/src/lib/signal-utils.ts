/**
 * Signal Strength Utilities
 * 
 * Converts raw signal values to human-readable formats.
 * Based on 3GPP specifications for LTE signal quality.
 */

export type SignalQuality = 'excellent' | 'good' | 'fair' | 'poor' | 'none' | 'null';

/**
 * Convert RSSI (dBm) to signal bars (0-5).
 * RSSI ranges typically from -113 dBm (poor) to -51 dBm (excellent).
 */
export function rssiToBars(rssi: number): number {
  if (rssi >= -65) return 5;
  if (rssi >= -75) return 4;
  if (rssi >= -85) return 3;
  if (rssi >= -95) return 2;
  if (rssi >= -105) return 1;
  return 0;
}

/**
 * Convert RSRP (dBm) to signal bars (0-5).
 * RSRP is the primary LTE signal strength indicator.
 * Ranges from -140 dBm (very weak) to -44 dBm (very strong).
 */
export function rsrpToBars(rsrp: number): number {
  if (rsrp >= -80) return 5;
  if (rsrp >= -90) return 4;
  if (rsrp >= -100) return 3;
  if (rsrp >= -110) return 2;
  if (rsrp >= -120) return 1;
  return 0;
}

/**
 * Get quality label from RSRP value.
 */
export function rsrpToQuality(rsrp: number): SignalQuality {
  if (rsrp <= -140) return 'null'; // Invalid/unavailable
  if (rsrp >= -80) return 'excellent';
  if (rsrp >= -90) return 'good';
  if (rsrp >= -100) return 'fair';
  if (rsrp >= -110) return 'poor';
  return 'none';
}

/**
 * Convert RSRQ (dB) to quality assessment.
 * RSRQ indicates signal quality relative to interference.
 * Ranges from -20 dB (poor) to -3 dB (excellent).
 */
export function rsrqToQuality(rsrq: number): SignalQuality {
  if (rsrq <= -140) return 'null'; // Invalid/unavailable
  if (rsrq >= -5) return 'excellent';
  if (rsrq >= -9) return 'good';
  if (rsrq >= -12) return 'fair';
  if (rsrq >= -15) return 'poor';
  return 'none';
}

/**
 * Convert SINR (dB) to quality assessment.
 * SINR indicates signal quality vs noise.
 * Higher is better; ranges from -20 dB to 30 dB typically.
 */
export function sinrToQuality(sinr: number): SignalQuality {
  if (sinr === 0 || sinr <= -140) return 'null'; // Invalid/unavailable
  if (sinr >= 20) return 'excellent';
  if (sinr >= 13) return 'good';
  if (sinr > 0) return 'fair';
  if (sinr >= -5) return 'poor';
  return 'none';
}

/**
 * Get overall signal quality from all metrics.
 * Prioritizes RSRP as the primary indicator for LTE.
 */
export function getOverallQuality(
  rsrp: number,
  rsrq: number,
  sinr: number
): SignalQuality {
  const rsrpQ = rsrpToQuality(rsrp);
  const rsrqQ = rsrqToQuality(rsrq);
  const sinrQ = sinrToQuality(sinr);

  // Weight RSRP most heavily
  const qualities: SignalQuality[] = [rsrpQ, rsrpQ, rsrqQ, sinrQ];
  const scores: number[] = qualities.map(q => {
    switch (q) {
      case 'excellent': return 4;
      case 'good': return 3;
      case 'fair': return 2;
      case 'poor': return 1;
      default: return 0;
    }
  });

  const avg = scores.reduce((a, b) => a + b, 0) / scores.length;

  if (avg >= 3.5) return 'excellent';
  if (avg >= 2.5) return 'good';
  if (avg >= 1.5) return 'fair';
  if (avg >= 0.5) return 'poor';
  return 'none';
}

/**
 * Get Tailwind color class for signal quality.
 */
export function qualityToColor(quality: SignalQuality): string {
  switch (quality) {
    case 'excellent': return 'text-signal-excellent';
    case 'good': return 'text-signal-good';
    case 'fair': return 'text-signal-fair';
    case 'poor': return 'text-signal-poor';
    case 'null': return 'text-theme-text-muted';
    default: return 'text-signal-none';
  }
}

/**
 * Get background color class for signal quality.
 */
export function qualityToBgColor(quality: SignalQuality): string {
  switch (quality) {
    case 'excellent': return 'bg-signal-excellent';
    case 'good': return 'bg-signal-good';
    case 'fair': return 'bg-signal-fair';
    case 'poor': return 'bg-signal-poor';
    case 'null': return 'bg-theme-bg-tertiary';
    default: return 'bg-signal-none';
  }
}

/**
 * Format dBm value for display.
 */
export function formatDbm(value: number): string {
  return `${value.toFixed(0)} dBm`;
}

/**
 * Format dB value for display.
 */
export function formatDb(value: number): string {
  return `${value.toFixed(1)} dB`;
}

/**
 * Check if a signal metric value is a sentinel (invalid/unavailable).
 * Sentinel values: -999 (explicit unknown from AT+CSQ) or <= -140 (out of valid range).
 */
export function isSentinel(value: number): boolean {
  return value === -999 || value <= -140;
}

/**
 * Format a signal metric for display, returning "N/A" for sentinel values.
 * Sentinel values: -999 (explicit unknown from AT+CSQ) or <= -140 (out of valid range).
 */
export function formatSignalValue(value: number, unit: 'dbm' | 'db'): string {
  if (isSentinel(value)) return 'N/A';
  return unit === 'dbm' ? formatDbm(value) : formatDb(value);
}

/**
 * Convert CSQ (0-31) to RSSI (dBm).
 * CSQ is the older signal indicator used with AT+CSQ.
 */
export function csqToRssi(csq: number): number {
  if (csq === 99) return -999; // Unknown
  return -113 + csq * 2;
}

/**
 * Get human-readable technology name.
 */
export function technologyLabel(tech: string | null): string {
  if (!tech) return 'No Signal';
  switch (tech) {
    case '2G': return '2G (GSM)';
    case '3G': return '3G (UMTS)';
    case '4G': return '4G LTE';
    case '5G': return '5G NR';
    default: return tech;
  }
}
