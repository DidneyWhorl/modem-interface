//! License management route handlers.
//!
//! The status/activate endpoints are public (no auth) so the activation screen
//! can function when the software is unlicensed. The *full* license detail
//! (tier / expiry / account id) is only served on an authenticated route.

use axum::{extract::State, Json};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::api::error::{ApiError, ApiResult};
use crate::security::license::{self, LicenseState};
use crate::state::AppState;

// === Response Types ===

/// Full license detail — `state` + `device_token` plus the sensitive
/// `tier` / `expires_at` / `user_id` fields.
///
/// Served ONLY to authenticated callers (`GET /api/license/detail`) and echoed
/// back to the caller of `POST /api/license/activate` (the activator supplied
/// the key, so returning its own tier/user_id is not a new disclosure). The
/// public `GET /api/license/status` route returns the reduced
/// [`PublicLicenseStatus`] instead.
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

/// Reduced, unauthenticated license shape for the public `/license/status`
/// route (L-01, 2026-06-21).
///
/// Deliberately carries ONLY `state` + `device_token`. The `/license/status`
/// route is reachable without auth so the activation screen can render on an
/// unlicensed device — but an unauthenticated caller must NOT be able to read
/// the account's `tier`, `expires_at`, or `user_id` (an account identifier).
/// Those sensitive fields move behind authentication on
/// [`get_license_detail`] (`GET /api/license/detail`). Mirrors the
/// `PublicSignalInfo` (cell_id) / `PublicModemStatus` (ip_address) treatment in
/// `modem.rs`.
#[derive(Serialize)]
pub struct PublicLicenseStatus {
    pub state: String,
    pub device_token: String,
    // tier / expires_at / user_id intentionally omitted pre-auth.
}

impl From<&LicenseStatusResponse> for PublicLicenseStatus {
    fn from(full: &LicenseStatusResponse) -> Self {
        Self {
            state: full.state.clone(),
            device_token: full.device_token.clone(),
        }
    }
}

#[derive(Deserialize)]
pub struct ActivateRequest {
    pub license_key: String,
}

// === Handlers ===

/// GET /api/license/status — PUBLIC (no auth). Returns ONLY the reduced
/// `state` + `device_token` shape so the activation screen can function on an
/// unlicensed device without leaking the account's tier/expiry/user_id (L-01).
pub async fn get_license_status(
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<PublicLicenseStatus>> {
    let license = state.license_state.read().await;
    let full = license_state_to_response(&license, &state.device_token);
    Ok(Json(PublicLicenseStatus::from(&full)))
}

/// GET /api/license/detail — AUTHENTICATED. Returns the full license shape
/// (state, device_token, tier, expires_at, user_id) for the dashboard's user
/// profile display. Registered in the PROTECTED route group; auth is enforced
/// by the router middleware, so no extra check is needed here.
pub async fn get_license_detail(
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

// =============================================================================
// Public (pre-auth) license shape — L-01 disclosure split (2026-06-21)
//
// The public/unauthenticated `/license/status` route returns the reduced
// `PublicLicenseStatus` shape, which must NOT expose `tier`, `expires_at`, or
// `user_id`. The authenticated `/license/detail` route (and the activate echo)
// still returns the full `LicenseStatusResponse` INCLUDING those fields. These
// tests lock the split in both directions — mirroring
// `public_signal_json_omits_cell_id` / `authenticated_signal_json_includes_cell_id`
// in `modem.rs`.
// =============================================================================

#[cfg(test)]
mod license_disclosure_tests {
    use super::*;

    /// Build the full response from a Valid license carrying secret-marker
    /// values so a leak anywhere in the JSON string is detectable.
    fn sample_valid_full() -> LicenseStatusResponse {
        let state = LicenseState::Valid {
            user_id: "SECRET_USER_42".to_string(),
            tier: "SECRET_TIER_ENTERPRISE".to_string(),
            expires_at: "2099-01-02T03:04:05Z".parse::<DateTime<Utc>>().unwrap(),
            features: vec!["remote_access".to_string()],
        };
        license_state_to_response(&state, "device-token-abc")
    }

    #[test]
    fn public_license_json_omits_tier_expiry_user_id() {
        // Reduced public shape must drop tier / expires_at / user_id entirely.
        let full = sample_valid_full();
        let public = PublicLicenseStatus::from(&full);
        let json = serde_json::to_value(&public).expect("PublicLicenseStatus must serialize");
        let obj = json.as_object().expect("must serialize to a JSON object");

        for forbidden in ["tier", "expires_at", "user_id"] {
            assert!(
                !obj.contains_key(forbidden),
                "public license JSON must NOT contain {forbidden}; got keys: {:?}",
                obj.keys().collect::<Vec<_>>()
            );
        }
        // The allowed fields must still be present.
        assert_eq!(obj.get("state"), Some(&serde_json::json!("valid")));
        assert_eq!(obj.get("device_token"), Some(&serde_json::json!("device-token-abc")));

        // The secret values must not leak under ANY key.
        let raw = json.to_string();
        for secret in ["SECRET_USER_42", "SECRET_TIER_ENTERPRISE", "2099-01-02"] {
            assert!(
                !raw.contains(secret),
                "public license JSON must not leak {secret} anywhere; got: {raw}"
            );
        }
    }

    #[test]
    fn authenticated_license_json_includes_tier_expiry_user_id() {
        // The full LicenseStatusResponse (authenticated /license/detail + the
        // activate echo) keeps tier / expires_at / user_id.
        let full = sample_valid_full();
        let json = serde_json::to_value(&full).expect("LicenseStatusResponse must serialize");
        let obj = json.as_object().expect("must serialize to a JSON object");

        assert_eq!(
            obj.get("tier"),
            Some(&serde_json::json!("SECRET_TIER_ENTERPRISE")),
            "authenticated license JSON MUST contain tier"
        );
        assert_eq!(
            obj.get("user_id"),
            Some(&serde_json::json!("SECRET_USER_42")),
            "authenticated license JSON MUST contain user_id"
        );
        assert!(
            obj.contains_key("expires_at"),
            "authenticated license JSON MUST contain expires_at"
        );
    }
}
