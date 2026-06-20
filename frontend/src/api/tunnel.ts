import { apiGet, apiPut } from './client';

export interface TunnelConfigResponse {
  enabled: boolean;
  ports: number[];
  url: string;
  feature_available: boolean;
}

export interface UpdateTunnelConfigRequest {
  enabled?: boolean;
  ports?: number[];
}

export function getTunnelConfig(): Promise<TunnelConfigResponse> {
  return apiGet<TunnelConfigResponse>('/tunnel/config');
}

export function updateTunnelConfig(update: UpdateTunnelConfigRequest): Promise<TunnelConfigResponse> {
  return apiPut<TunnelConfigResponse>('/tunnel/config', update);
}
