/**
 * Traffic Steering Types (Level 2)
 *
 * TypeScript equivalents of backend steering rule types.
 * See docs/API-CONTRACT.md for endpoint details.
 */

export type Protocol = 'tcp' | 'udp' | 'icmp';
export type PortMatch = number | [number, number]; // single port or [start, end] range
export type FailoverMode = 'automatic' | 'preferred_fallback' | 'strict';
export type RuleStatus = 'active' | 'dormant' | 'blocked';

export interface SteeringRule {
  id: string;
  name: string;
  enabled: boolean;
  priority: number;
  source_ip: string[] | null;
  destination_ip: string[] | null;
  protocol: Protocol | null;
  destination_port: PortMatch | null;
  source_port: PortMatch | null;
  target_wan: string;
  target_wan_label: string | null;
  failover_mode: FailoverMode;
  fallback_wan: string | null;
  status: RuleStatus;
  fwmark: number;
}

export interface SteeringListResponse {
  rules: SteeringRule[];
  firewall_backend: string;
}

export interface CreateSteeringRuleRequest {
  name: string;
  enabled?: boolean;
  source_ip?: string[] | null;
  destination_ip?: string[] | null;
  protocol?: Protocol | null;
  destination_port?: PortMatch | null;
  source_port?: PortMatch | null;
  target_wan: string;
  failover_mode?: FailoverMode;
  fallback_wan?: string | null;
}

export interface UpdateSteeringRuleRequest {
  name?: string;
  enabled?: boolean;
  source_ip?: string[] | null;
  destination_ip?: string[] | null;
  protocol?: Protocol | null;
  destination_port?: PortMatch | null;
  source_port?: PortMatch | null;
  target_wan?: string;
  failover_mode?: FailoverMode;
  fallback_wan?: string | null;
}

export interface ReorderSteeringRequest {
  order: string[];
}
