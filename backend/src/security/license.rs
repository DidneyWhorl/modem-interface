//! License verification for CTRL-Modem.
//!
//! Verifies Ed25519-signed license keys against the device's hardware token.
//! License format: `BASE64URL(payload_json).BASE64URL(ed25519_signature)`

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{DateTime, Utc};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

/// Compile-time embedded production public key (PEM).
const EMBEDDED_PROD_PUB_KEY_PEM: &str = include_str!("signing.pub");

/// Compile-time embedded staging public key (PEM), used to verify
/// staging-environment license signatures. Rotating the key requires
/// updating the embedded PEM file in this repo and rebuilding the binary.
const EMBEDDED_STAGING_PUB_KEY_PEM: &str = include_str!("signing-staging.pub");

/// Runtime override path for the public key.
const RUNTIME_PUB_KEY_PATH: &str = "/etc/modem-interface/license.pub";

/// Directory holding the active license slot + per-env sidecars.
const LICENSE_DIR: &str = "/etc/modem-interface";

/// Path where the license key file is stored.
const LICENSE_KEY_PATH: &str = "/etc/modem-interface/license.key";

/// Returns the sidecar path for known envs (production|staging), or None
/// for "(custom)" / unknown envs.
///
/// Used by `store_license` to write the per-env sidecar alongside the
/// active slot, and by `cli/env_cmd.rs` tests to attest the same path
/// schema is in use across both modules.
pub(crate) fn license_sidecar_path(env: &str) -> Option<std::path::PathBuf> {
    if env == "production" || env == "staging" {
        Some(std::path::PathBuf::from(format!(
            "{LICENSE_DIR}/license.{env}.key"
        )))
    } else {
        None
    }
}

/// Short, non-reversible SHA-256 hex prefix of a token, for safe logging.
///
/// Device tokens are portal credentials and must never be logged verbatim
/// (finding #3b). Returns the first 8 hex chars (4 bytes) of `SHA-256(token)`
/// — enough for support correlation, not enough to recover the token.
fn token_sha256_prefix(token: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(token.as_bytes())
        .iter()
        .take(4)
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// License state enumeration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "state")]
pub enum LicenseState {
    #[serde(rename = "unlicensed")]
    Unlicensed,
    #[serde(rename = "valid")]
    Valid {
        user_id: String,
        tier: String,
        expires_at: DateTime<Utc>,
        #[serde(default)]
        features: Vec<String>,
    },
    #[serde(rename = "expired")]
    Expired {
        user_id: String,
        tier: String,
    },
    #[serde(rename = "invalid_signature")]
    InvalidSignature,
    #[serde(rename = "device_mismatch")]
    DeviceMismatch,
}

impl LicenseState {
    /// Check if the license includes a specific feature flag.
    #[allow(dead_code)]
    pub fn has_feature(&self, feature: &str) -> bool {
        match self {
            LicenseState::Valid { features, .. } => features.iter().any(|f| f == feature),
            _ => false,
        }
    }

    /// Returns the state name as a simple string for API responses.
    #[allow(dead_code)]
    pub fn state_name(&self) -> &'static str {
        match self {
            LicenseState::Unlicensed => "unlicensed",
            LicenseState::Valid { .. } => "valid",
            LicenseState::Expired { .. } => "expired",
            LicenseState::InvalidSignature => "invalid_signature",
            LicenseState::DeviceMismatch => "device_mismatch",
        }
    }
}

/// License payload JSON structure (inside the signed envelope).
#[derive(Debug, Deserialize)]
struct LicensePayload {
    #[allow(dead_code)]
    v: u32,
    device_token: String,
    user_id: String,
    #[allow(dead_code)]
    issued_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    tier: String,
    features: Vec<String>,
}

/// Load the Ed25519 verifying key for the given env, preferring runtime override.
///
/// `env` is one of "production", "staging", or "(custom)" (the latter falls
/// through to production). Runtime override at `RUNTIME_PUB_KEY_PATH` wins
/// for both envs (emergency hot-fix path).
fn load_verifying_key(env: &str) -> Result<VerifyingKey, String> {
    // Runtime override wins for both envs (emergency hot-fix path).
    if let Ok(runtime_pem) = std::fs::read_to_string(RUNTIME_PUB_KEY_PATH) {
        tracing::debug!("Using runtime public key from {}", RUNTIME_PUB_KEY_PATH);
        return parse_ed25519_pub_pem(&runtime_pem);
    }
    let pem = if env == "staging" {
        EMBEDDED_STAGING_PUB_KEY_PEM
    } else {
        EMBEDDED_PROD_PUB_KEY_PEM
    };
    parse_ed25519_pub_pem(pem)
}

/// Parse an Ed25519 public key from PEM format.
fn parse_ed25519_pub_pem(pem: &str) -> Result<VerifyingKey, String> {
    // Extract base64 content between PEM headers
    let b64: String = pem
        .lines()
        .filter(|line| !line.starts_with("-----"))
        .collect::<Vec<_>>()
        .join("");

    let der = base64::engine::general_purpose::STANDARD
        .decode(&b64)
        .map_err(|e| format!("PEM base64 decode failed: {e}"))?;

    // Ed25519 SPKI DER: 12-byte header + 32-byte key
    // OID 1.3.101.112 = Ed25519
    if der.len() == 44 {
        // Standard SPKI wrapping
        let key_bytes: [u8; 32] = der[12..]
            .try_into()
            .map_err(|_| "Invalid key length in SPKI".to_string())?;
        VerifyingKey::from_bytes(&key_bytes)
            .map_err(|e| format!("Invalid Ed25519 public key: {e}"))
    } else if der.len() == 32 {
        // Raw 32-byte key
        let key_bytes: [u8; 32] = der
            .try_into()
            .map_err(|_| "Invalid raw key length".to_string())?;
        VerifyingKey::from_bytes(&key_bytes)
            .map_err(|e| format!("Invalid Ed25519 public key: {e}"))
    } else {
        Err(format!("Unexpected DER length: {} (expected 44 for SPKI or 32 for raw)", der.len()))
    }
}

/// Verify a license key string against the given device token.
fn verify_license_key(license_key: &str, device_token: &str, env: &str) -> LicenseState {
    let key = match load_verifying_key(env) {
        Ok(k) => k,
        Err(e) => {
            tracing::error!("Failed to load license public key: {}", e);
            return LicenseState::InvalidSignature;
        }
    };

    // Split on "."
    let parts: Vec<&str> = license_key.trim().splitn(2, '.').collect();
    if parts.len() != 2 {
        tracing::warn!("License key missing '.' separator");
        return LicenseState::InvalidSignature;
    }

    let payload_b64 = parts[0];
    let sig_b64 = parts[1];

    // Decode signature
    let sig_bytes = match URL_SAFE_NO_PAD.decode(sig_b64) {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!("License signature base64 decode failed: {}", e);
            return LicenseState::InvalidSignature;
        }
    };

    // Verify Ed25519 signature against the base64-encoded payload string,
    // matching the portal which signs payload_b64.as_bytes()
    let signature = match Signature::from_slice(&sig_bytes) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("Invalid signature format: {}", e);
            return LicenseState::InvalidSignature;
        }
    };

    if key.verify(payload_b64.as_bytes(), &signature).is_err() {
        return LicenseState::InvalidSignature;
    }

    // Signature verified — now decode and deserialize the payload
    let payload_bytes = match URL_SAFE_NO_PAD.decode(payload_b64) {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!("License payload base64 decode failed: {}", e);
            return LicenseState::InvalidSignature;
        }
    };

    let payload: LicensePayload = match serde_json::from_slice(&payload_bytes) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("License payload JSON parse failed: {}", e);
            return LicenseState::InvalidSignature;
        }
    };

    // Check device token match
    if payload.device_token != device_token {
        // Never log raw device tokens — they are portal credentials. Log only a
        // short, non-reversible SHA-256 prefix of each (regression guard for
        // prior finding #3b; mirrors the sha256-prefix pattern in main.rs).
        tracing::warn!(
            "License device token mismatch: expected=sha256:{}…, got=sha256:{}…",
            token_sha256_prefix(device_token),
            token_sha256_prefix(&payload.device_token)
        );
        return LicenseState::DeviceMismatch;
    }

    // Check expiry (skip if system time looks wrong, e.g., before 2025)
    let now = Utc::now();
    if now.timestamp() > 1735689600 {
        // 2025-01-01 epoch
        if payload.expires_at < now {
            return LicenseState::Expired {
                user_id: payload.user_id,
                tier: payload.tier,
            };
        }
    }

    LicenseState::Valid {
        user_id: payload.user_id,
        tier: payload.tier,
        expires_at: payload.expires_at,
        features: payload.features,
    }
}

/// Check for an existing license on disk and verify it.
pub async fn check_license(device_token: &str, env: &str) -> LicenseState {
    let license_key = match tokio::fs::read_to_string(LICENSE_KEY_PATH).await {
        Ok(key) => key,
        Err(_) => return LicenseState::Unlicensed,
    };

    let trimmed = license_key.trim();
    if trimmed.is_empty() {
        return LicenseState::Unlicensed;
    }

    verify_license_key(trimmed, device_token, env)
}

/// Verify a license key string without reading from disk.
pub fn verify_license(license_key: &str, device_token: &str, env: &str) -> LicenseState {
    verify_license_key(license_key.trim(), device_token, env)
}

/// Write a license key to the env-specific sidecar (when env is known) and
/// to the active slot. For "(custom)" / unknown envs, only the active slot
/// is written.
///
/// **Write order: sidecar FIRST, active second.** This preserves the env
/// round-trip byte-perfect guarantee (Item #35 sub-task 1b spec) on partial
/// failure:
///
/// - sidecar OK + active fails: caller sees Err, sidecar holds the freshest
///   per-env state, the next `env set` round-trip back to this env restores
///   active from the sidecar consistently. No desync.
/// - sidecar fails: caller sees Err, active is unchanged from its previous
///   contents. No desync — the previous env still owns the active mirror.
///
/// The reverse order (active first) would corrupt the round-trip on partial
/// failure: active would carry the new license while the sidecar still held
/// the previous license, so a later `env set` cycle back to this env would
/// silently restore the stale sidecar to active and lose the recent grant.
///
/// Sidecar write is best-effort *for unknown envs only* — "(custom)" envs
/// have no canonical sidecar name, so operators using a custom portal manage
/// license placement themselves. Callers (heartbeat) that want full best-
/// effort semantics can `let _ = store_license(...)` and continue.
pub async fn store_license(license_key: &str, env: &str) -> std::io::Result<()> {
    tokio::fs::create_dir_all(std::path::Path::new(LICENSE_DIR)).await?;
    let trimmed = license_key.trim();

    // Sidecar first (when the env is canonical) so partial failure leaves
    // the per-env source of truth ahead of the active mirror.
    if let Some(sidecar) = license_sidecar_path(env) {
        crate::config::write_secret_file(&sidecar, trimmed).await?;
    }

    // Then mirror to the active slot. License keys are secrets — 0600.
    crate::config::write_secret_file(LICENSE_KEY_PATH, trimmed).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_embedded_keys_both_envs() {
        assert!(parse_ed25519_pub_pem(EMBEDDED_PROD_PUB_KEY_PEM).is_ok(),
            "production pubkey must parse");
        assert!(parse_ed25519_pub_pem(EMBEDDED_STAGING_PUB_KEY_PEM).is_ok(),
            "staging pubkey must parse");
    }

    #[test]
    fn load_verifying_key_picks_staging_pem_for_staging_env() {
        // Verifies the env routing in load_verifying_key.
        // Compares parsed bytes with parse_ed25519_pub_pem to confirm the
        // staging const is what gets returned for env="staging".
        let staging_key = load_verifying_key("staging").expect("staging must load");
        let expected = parse_ed25519_pub_pem(EMBEDDED_STAGING_PUB_KEY_PEM).unwrap();
        assert_eq!(staging_key.as_bytes(), expected.as_bytes());
    }

    #[test]
    fn load_verifying_key_picks_prod_pem_for_production_env() {
        let prod_key = load_verifying_key("production").expect("production must load");
        let expected = parse_ed25519_pub_pem(EMBEDDED_PROD_PUB_KEY_PEM).unwrap();
        assert_eq!(prod_key.as_bytes(), expected.as_bytes());
    }

    #[test]
    fn load_verifying_key_falls_through_to_prod_for_custom_env() {
        let custom_key = load_verifying_key("(custom)").expect("custom must load");
        let prod_expected = parse_ed25519_pub_pem(EMBEDDED_PROD_PUB_KEY_PEM).unwrap();
        assert_eq!(custom_key.as_bytes(), prod_expected.as_bytes());
    }

    #[test]
    fn store_license_sidecar_path_predicate_pins_known_envs() {
        // Pins the env predicate logic that gates sidecar writes inside
        // store_license. Bytes-on-disk attestation lives in bench predicates
        // P4+P5 (file I/O against /etc/modem-interface/ cannot be unit-tested
        // without exposing the path as a parameter).
        assert_eq!(
            license_sidecar_path("staging")
                .as_deref()
                .and_then(std::path::Path::to_str),
            Some("/etc/modem-interface/license.staging.key")
        );
        assert_eq!(
            license_sidecar_path("production")
                .as_deref()
                .and_then(std::path::Path::to_str),
            Some("/etc/modem-interface/license.production.key")
        );
        assert!(license_sidecar_path("(custom)").is_none());
        assert!(license_sidecar_path("anything-else").is_none());
    }

    #[test]
    fn token_sha256_prefix_does_not_leak_raw_token() {
        // The device-token-mismatch warn! must log only a sha256 prefix, never
        // the raw token (finding #3b regression guard). Build the same message
        // the handler builds and assert neither raw token appears.
        let expected = "EXPECTED-SECRET-DEVICE-TOKEN-123";
        let got = "ATTACKER-SUPPLIED-TOKEN-456";

        let exp_prefix = token_sha256_prefix(expected);
        let got_prefix = token_sha256_prefix(got);

        let message = format!(
            "License device token mismatch: expected=sha256:{exp_prefix}…, got=sha256:{got_prefix}…"
        );

        assert!(
            !message.contains(expected),
            "raw expected token leaked into log message: {message}"
        );
        assert!(
            !message.contains(got),
            "raw got token leaked into log message: {message}"
        );

        // Prefix is a short hex string (4 bytes => 8 hex chars), deterministic.
        assert_eq!(exp_prefix.len(), 8);
        assert!(exp_prefix.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(exp_prefix, got_prefix);
        assert_eq!(token_sha256_prefix(expected), exp_prefix, "must be deterministic");
    }

    #[test]
    fn test_invalid_license_format() {
        let state = verify_license_key("not-a-valid-license", "SOME-TOKEN", "production");
        assert!(matches!(state, LicenseState::InvalidSignature));
    }

    #[test]
    fn test_unlicensed_state_name() {
        assert_eq!(LicenseState::Unlicensed.state_name(), "unlicensed");
    }

    #[test]
    fn test_has_feature_with_features() {
        let state = LicenseState::Valid {
            user_id: "test".into(),
            tier: "pro".into(),
            expires_at: Utc::now() + chrono::Duration::days(365),
            features: vec!["remote_access".into(), "telemetry".into()],
        };
        assert!(state.has_feature("remote_access"));
        assert!(state.has_feature("telemetry"));
        assert!(!state.has_feature("nonexistent"));
    }

    #[test]
    fn test_has_feature_empty() {
        let state = LicenseState::Valid {
            user_id: "test".into(),
            tier: "standard".into(),
            expires_at: Utc::now() + chrono::Duration::days(365),
            features: vec![],
        };
        assert!(!state.has_feature("remote_access"));
    }

    #[test]
    fn test_has_feature_non_valid_states() {
        assert!(!LicenseState::Unlicensed.has_feature("remote_access"));
        assert!(!LicenseState::InvalidSignature.has_feature("remote_access"));
        assert!(!LicenseState::DeviceMismatch.has_feature("remote_access"));
        assert!(!LicenseState::Expired {
            user_id: "test".into(),
            tier: "pro".into(),
        }.has_feature("remote_access"));
    }
}
