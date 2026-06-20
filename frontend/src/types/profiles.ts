/**
 * Modem Profile Types
 *
 * Types for the modem profile system — identifies modem models,
 * their capabilities, and profile match status.
 */

export interface ModemIdentity {
  vendor_id: string;
  product_id: string;
  manufacturer: string;
  model: string;
}

export interface ModemCapabilities {
  supports_5g: boolean;
  supports_carrier_aggregation: boolean;
  supported_technologies: string[];
  max_supported_bands: string[];
  supported_protocols: string[];
  has_temperature_sensor: boolean;
  has_gps: boolean;
}

export interface ModemProfileSummary {
  profile_id: string;
  vendor_id: string;
  product_id: string;
  manufacturer: string;
  model: string;
  capabilities: ModemCapabilities;
  is_generic: boolean;
  notes?: string;
}

export interface ActiveModemInfo {
  modem_id: string;
  profile: ModemProfileSummary;
  detected: DetectedModemEnhanced | null;
}

export interface DetectedModemEnhanced {
  modem_id: string;
  device_path: string;
  protocol: string;
  description: string;
  vendor_id: string | null;
  product_id: string | null;
  profile_id: string | null;
  has_profile: boolean;
}

export interface RescanResponse {
  detected: DetectedModemEnhanced[];
  active_modem_index: number;
  active_profile: ModemProfileSummary;
}

export interface ProfileRequestPayload {
  vendor_id: string;
  product_id: string;
  device_info_response: string;
  user_notes: string;
}

export interface ProfileRequestResponse {
  mailto_link: string;
  request_text: string;
}

export interface ProbeResult {
  status: 'Success' | 'Error';
  data: string;
}

export interface DiscoveryProbe {
  timestamp: string;
  usb_id: string;
  standard_commands: Record<string, string>;
  vendor_probes: Record<string, ProbeResult>;
  device_summary: string | null;
}

export interface DiscoveryResponse {
  probe: DiscoveryProbe;
  saved_to: string;
}
