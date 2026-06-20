//! Per-device Ed25519 authentication keypair (Item #3 — signed device auth).
//!
//! Unlike the hardware fingerprint (`hardware/fingerprint.rs`), this key is
//! self-generated, so it is platform-independent and needs no mock variant.
//! Generate-once / persist-forever, mirroring the `device-token` / `license.key`
//! on-disk discipline (0600). Phase 1 only EXPOSES the public key + key-id and
//! can sign; no request path uses it yet.

use std::path::Path;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use ed25519_dalek::{Signer, SigningKey};
use rand::RngCore;
use sha2::{Digest, Sha256};

/// Default on-router path for the private key.
pub const DEVICE_AUTH_KEY_PATH: &str = "/etc/modem-interface/device-auth.key";

pub struct DeviceAuth {
    // Item #3 Phase 3: consumed by `sign()`, which the heartbeat + poll-config
    // senders now call to produce the signed envelope.
    signing_key: SigningKey,
    /// base64url(no-pad) of the 32-byte Ed25519 public key.
    pub public_key_b64: String,
    /// First 16 base64url chars of SHA-256(public_key_bytes).
    pub key_id: String,
}

impl DeviceAuth {
    /// Load the keypair from `path`, or generate + persist it (0600) if absent.
    pub fn load_or_create(path: &Path) -> std::io::Result<Self> {
        let signing_key = if path.exists() {
            let b64 = std::fs::read_to_string(path)?;
            let seed: [u8; 32] = URL_SAFE_NO_PAD
                .decode(b64.trim())
                .ok()
                .and_then(|v| v.try_into().ok())
                .ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "device-auth.key is not a valid base64url 32-byte seed",
                    )
                })?;
            SigningKey::from_bytes(&seed)
        } else {
            // SigningKey::generate requires ed25519-dalek's `rand_core` feature,
            // which this crate does not enable. A SigningKey IS its 32-byte seed,
            // so generate the seed directly with the OS CSPRNG (rand 0.8 OsRng)
            // and construct from it — cryptographically equivalent.
            let mut seed = [0u8; 32];
            rand::rngs::OsRng.fill_bytes(&mut seed);
            let sk = SigningKey::from_bytes(&seed);
            let b64 = URL_SAFE_NO_PAD.encode(sk.to_bytes());
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).ok();
            }
            // Create the key file already 0600 (no write-then-chmod TOCTOU
            // window where the private seed is world-readable). We are in the
            // `else` branch of `path.exists()`, so the file should not exist.
            write_new_secret(path, b64.as_bytes())?;
            sk
        };

        let pk_bytes = signing_key.verifying_key().to_bytes();
        let public_key_b64 = URL_SAFE_NO_PAD.encode(pk_bytes);
        let hash = Sha256::digest(pk_bytes);
        let key_id = URL_SAFE_NO_PAD.encode(hash)[..16].to_string();

        Ok(Self {
            signing_key,
            public_key_b64,
            key_id,
        })
    }

    /// Sign `message`; returns base64url(no-pad) of the 64-byte signature.
    pub fn sign(&self, message: &[u8]) -> String {
        URL_SAFE_NO_PAD.encode(self.signing_key.sign(message).to_bytes())
    }

    /// Test-only: build an ephemeral in-memory keypair (no disk I/O), for
    /// constructing `AppState` in unit tests.
    #[cfg(test)]
    pub fn ephemeral() -> Self {
        let mut seed = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut seed);
        let signing_key = SigningKey::from_bytes(&seed);
        let pk_bytes = signing_key.verifying_key().to_bytes();
        let public_key_b64 = URL_SAFE_NO_PAD.encode(pk_bytes);
        let hash = Sha256::digest(pk_bytes);
        let key_id = URL_SAFE_NO_PAD.encode(hash)[..16].to_string();
        Self {
            signing_key,
            public_key_b64,
            key_id,
        }
    }
}

/// Write `contents` to a freshly-created secret file restricted to 0600.
///
/// On unix the file is created already at mode 0600 (`create_new` + `mode`) so
/// the private seed is never world-readable, even momentarily — this closes the
/// write-then-chmod TOCTOU window. If the file unexpectedly already exists, we
/// fall back to overwrite + chmod (the caller only reaches this in the
/// generate path, where the file is not supposed to exist). On non-unix this is
/// a plain write.
#[cfg(unix)]
fn write_new_secret(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
    {
        Ok(mut f) => f.write_all(contents),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            std::fs::write(path, contents)?;
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        }
        Err(e) => Err(e),
    }
}

#[cfg(not(unix))]
fn write_new_secret(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, contents)
}

/// Detect the portal's recoverable nonce-desync signal (Item #3 Phase 3, spec §13).
///
/// The portal returns HTTP **200** with body `{"status":"nonce_stale",
/// "next_nonce":"<fresh>"}` when a signed request's nonce no longer matches the
/// stored one (post-reboot empty nonce, single-use replay, or TTL expiry). The
/// busybox HTTP clients on the target hardware cannot read a 4xx body, so the
/// recoverable signal is carried as a 200 body. Returns `Some(next_nonce)` only
/// when the body is JSON with `status == "nonce_stale"` and a string
/// `next_nonce`; `None` otherwise (success body, non-JSON, missing field) —
/// never panics. The caller re-signs with the fresh nonce and retries once.
pub fn parse_nonce_stale(body: &str) -> Option<String> {
    let v = serde_json::from_str::<serde_json::Value>(body).ok()?;
    if v.get("status").and_then(|s| s.as_str()) != Some("nonce_stale") {
        return None;
    }
    v.get("next_nonce")
        .and_then(|n| n.as_str())
        .map(|s| s.to_string())
}

/// Canonical bytes for a tunnel-auth signature (Item #3 Phase 4). MUST match the
/// portal builder. Layout: `"v1\ntunnel\n" || device_token || "\n" || nonce`
/// (no trailing newline). The router signs these bytes over the server-issued
/// `TunnelChallenge` nonce in its `TunnelAuth` reply.
pub fn canonical_tunnel(device_token: &str, nonce: &str) -> Vec<u8> {
    format!("v1\ntunnel\n{device_token}\n{nonce}").into_bytes()
}

/// Canonical bytes for an HTTP request signature. MUST match the portal builder.
pub fn canonical_http(
    device_token: &str,
    nonce: &str,
    method: &str,
    path: &str,
    body: &[u8],
) -> Vec<u8> {
    let body_hash = Sha256::digest(body);
    let body_hex: String = body_hash.iter().map(|b| format!("{b:02x}")).collect();
    format!("v1\n{device_token}\n{nonce}\n{method}\n{path}\n{body_hex}").into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_temp_path(tag: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        p.push(format!("ctrl-device-auth-{tag}-{n}.key"));
        p
    }

    #[test]
    fn creates_then_loads_same_key() {
        let path = unique_temp_path("loadcreate");
        let a = DeviceAuth::load_or_create(&path).unwrap();
        let b = DeviceAuth::load_or_create(&path).unwrap(); // second call must READ, not regen
        assert_eq!(a.public_key_b64, b.public_key_b64);
        assert_eq!(a.key_id, b.key_id);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn key_id_is_16_base64url_chars() {
        let path = unique_temp_path("kid");
        let da = DeviceAuth::load_or_create(&path).unwrap();
        assert_eq!(da.key_id.len(), 16);
        assert!(da
            .key_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn sign_then_verify_roundtrip() {
        let path = unique_temp_path("roundtrip");
        let da = DeviceAuth::load_or_create(&path).unwrap();
        let msg = b"hello-canonical";
        let sig_b64 = da.sign(msg);
        // verify with the public key, as the portal will
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
        use ed25519_dalek::{Signature, Verifier, VerifyingKey};
        let pk_bytes: [u8; 32] = URL_SAFE_NO_PAD
            .decode(&da.public_key_b64)
            .unwrap()
            .try_into()
            .unwrap();
        let vk = VerifyingKey::from_bytes(&pk_bytes).unwrap();
        let sig_bytes: [u8; 64] = URL_SAFE_NO_PAD
            .decode(&sig_b64)
            .unwrap()
            .try_into()
            .unwrap();
        assert!(vk
            .verify_strict(msg, &Signature::from_bytes(&sig_bytes))
            .is_ok());
        // tampered message must fail
        assert!(vk
            .verify_strict(b"tampered", &Signature::from_bytes(&sig_bytes))
            .is_err());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn canonical_http_matches_shared_vector() {
        let got = canonical_http("TOKEN123", "NONCE456", "GET", "/api/v1/device/poll-config", b"");
        let want = b"v1\nTOKEN123\nNONCE456\nGET\n/api/v1/device/poll-config\ne3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        assert_eq!(got, want);
    }

    // Item #3 Phase 3 parity vectors — byte-identical to the portal's
    // `crypto::device_auth::canonical_http` test assertions so the two builders
    // are proven to agree on the new Phase-3 surfaces.

    #[test]
    fn canonical_http_heartbeat_post_parity_vector() {
        // POST /api/v1/heartbeat over body b"{}" (SHA-256("{}") =
        // 44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a).
        let got = canonical_http("TOKEN123", "NONCE456", "POST", "/api/v1/heartbeat", b"{}");
        let want = b"v1\nTOKEN123\nNONCE456\nPOST\n/api/v1/heartbeat\n44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a";
        assert_eq!(got, want);
    }

    #[test]
    fn canonical_http_empty_nonce_get_parity_vector() {
        // Empty-nonce GET (the post-reboot recovery case): the nonce line is
        // empty; body hash is SHA-256("") =
        // e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855.
        let got = canonical_http("TOKEN123", "", "GET", "/api/v1/device/poll-config", b"");
        let want = b"v1\nTOKEN123\n\nGET\n/api/v1/device/poll-config\ne3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        assert_eq!(got, want);
    }

    #[test]
    fn canonical_tunnel_matches_shared_vector() {
        // Byte-identical to the portal's `crypto::device_auth::canonical_tunnel`
        // assertion so the two builders are proven to agree (Item #3 Phase 4).
        let got = canonical_tunnel("TOKEN123", "NONCE456");
        let want = b"v1\ntunnel\nTOKEN123\nNONCE456";
        assert_eq!(got, want);
    }

    #[test]
    fn parse_nonce_stale_present() {
        let body = r#"{"status":"nonce_stale","next_nonce":"FRESH123"}"#;
        assert_eq!(parse_nonce_stale(body), Some("FRESH123".to_string()));
    }

    #[test]
    fn parse_nonce_stale_success_body_is_none() {
        // A normal success body (status ok, even with a next_nonce) is NOT a
        // nonce_stale signal — the success path stores next_nonce separately.
        let body = r#"{"status":"ok","next_nonce":"FRESH123"}"#;
        assert_eq!(parse_nonce_stale(body), None);
    }

    #[test]
    fn parse_nonce_stale_missing_next_nonce_is_none() {
        let body = r#"{"status":"nonce_stale"}"#;
        assert_eq!(parse_nonce_stale(body), None);
    }

    #[test]
    fn parse_nonce_stale_non_json_does_not_panic() {
        assert_eq!(parse_nonce_stale("not json"), None);
        assert_eq!(parse_nonce_stale(""), None);
    }

    #[cfg(unix)]
    #[test]
    fn key_file_is_0600() {
        use std::os::unix::fs::PermissionsExt;
        let path = unique_temp_path("perms");
        let _ = DeviceAuth::load_or_create(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        std::fs::remove_file(&path).ok();
    }
}
