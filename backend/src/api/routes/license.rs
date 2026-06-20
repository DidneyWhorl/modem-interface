//! License management route handlers.
//!
//! Public endpoints for checking license status and activating a license key.
//! These endpoints do not require authentication so the activation screen
//! can function when the software is unlicensed.

use axum::{extract::State, Json};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::api::error::{ApiError, ApiResult};
use crate::security::license::{self, LicenseState};
use crate::state::AppState;

// === Response Types ===

#[derive(Serialize)]
pub struct LicenseStatusResponse {
    pub state: String,
    pub device_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
}

#[derive(Deserialize)]
pub struct ActivateRequest {
    pub license_key: String,
}

// === Handlers ===

/// GET /api/license/status — return current license state and device token.
pub async fn get_license_status(
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<LicenseStatusResponse>> {
    let license = state.license_state.read().await;
    Ok(Json(license_state_to_response(&license, &state.device_token)))
}

/// POST /api/license/activate — validate and store a license key.
pub async fn activate_license(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ActivateRequest>,
) -> ApiResult<Json<LicenseStatusResponse>> {
    let license_key = body.license_key.trim();
    if license_key.is_empty() {
        return Err(ApiError::bad_request("License key is required"));
    }

    // Resolve env once for verify + store.
    let env_name = {
        let cfg = state.config.read().await;
        crate::config::current_env(&cfg.portal.base_url)
    };

    // Verify the license key
    let new_state = license::verify_license(license_key, &state.device_token, env_name);

    match &new_state {
        LicenseState::Valid { .. } => {
            // Store to disk + env sidecar
            if let Err(e) = license::store_license(license_key, env_name).await {
                tracing::error!("Failed to write license file: {}", e);
                return Err(ApiError::internal("Failed to store license key"));
            }

            // Update in-memory state
            let mut ls = state.license_state.write().await;
            *ls = new_state.clone();
            drop(ls);

            tracing::info!("License activated successfully");
            Ok(Json(license_state_to_response(&new_state, &state.device_token)))
        }
        LicenseState::InvalidSignature => {
            Err(ApiError::bad_request("Invalid license key: signature verification failed"))
        }
        LicenseState::DeviceMismatch => {
            Err(ApiError::bad_request("License key is for a different device"))
        }
        LicenseState::Expired { .. } => {
            Err(ApiError::bad_request("License key has expired"))
        }
        LicenseState::Unlicensed => {
            Err(ApiError::bad_request("Invalid license key format"))
        }
    }
}

/// Convert LicenseState to API response.
fn license_state_to_response(state: &LicenseState, device_token: &str) -> LicenseStatusResponse {
    match state {
        LicenseState::Unlicensed => LicenseStatusResponse {
            state: "unlicensed".to_string(),
            device_token: device_token.to_string(),
            tier: None,
            expires_at: None,
            user_id: None,
        },
        LicenseState::Valid { user_id, tier, expires_at, .. } => LicenseStatusResponse {
            state: "valid".to_string(),
            device_token: device_token.to_string(),
            tier: Some(tier.clone()),
            expires_at: Some(*expires_at),
            user_id: Some(user_id.clone()),
        },
        LicenseState::Expired { user_id, tier } => LicenseStatusResponse {
            state: "expired".to_string(),
            device_token: device_token.to_string(),
            tier: Some(tier.clone()),
            expires_at: None,
            user_id: Some(user_id.clone()),
        },
        LicenseState::InvalidSignature => LicenseStatusResponse {
            state: "invalid_signature".to_string(),
            device_token: device_token.to_string(),
            tier: None,
            expires_at: None,
            user_id: None,
        },
        LicenseState::DeviceMismatch => LicenseStatusResponse {
            state: "device_mismatch".to_string(),
            device_token: device_token.to_string(),
            tier: None,
            expires_at: None,
            user_id: None,
        },
    }
}
