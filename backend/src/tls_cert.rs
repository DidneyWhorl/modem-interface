//! In-binary self-signed TLS certificate generation.
//!
//! Replaces the first-boot `openssl req -x509` call that the OpenWrt init script
//! used to make (which dragged the ~1.3 MB `openssl-util` / `libopenssl` packages
//! into the overlay). The binary already links pure-Rust rustls (`ring`); `rcgen`
//! reuses that same `ring` backend, so generating the cert here adds no
//! aws-lc-rs / aws-lc-sys / system-OpenSSL dependency.
//!
//! The `ring` backend cannot produce RSA keys, so the cert is **ECDSA P-256**
//! (`PKCS_ECDSA_P256_SHA256`) — smaller than the old RSA-2048 cert and accepted
//! by the frontend (which already trusts the self-signed LAN cert).

use std::path::Path;

use anyhow::Context;
use tracing::info;

/// Generate a self-signed ECDSA P-256 certificate + private key as PEM strings.
///
/// Validity is anchored to a **fixed wide window** (2020-01-01 .. 2045-01-01),
/// NOT to "now": routers boot with the clock at the 1970 epoch until NTP syncs,
/// so a `now`-relative window would produce a not-yet-valid (or 1970-anchored)
/// cert. Validity is not enforced anyway (self-signed + accept-invalid on the
/// frontend), but a sane fixed window avoids epoch weirdness.
fn generate_self_signed_pem() -> anyhow::Result<(String, String)> {
    use rcgen::{
        date_time_ymd, CertificateParams, DistinguishedName, DnType, KeyPair,
        PKCS_ECDSA_P256_SHA256,
    };

    let mut params = CertificateParams::new(vec![
        "modem-interface".to_string(),
        "localhost".to_string(),
    ])
    .context("building certificate params")?;

    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, "modem-interface");
    params.distinguished_name = dn;

    // Clock-independent validity window (D4).
    params.not_before = date_time_ymd(2020, 1, 1);
    params.not_after = date_time_ymd(2045, 1, 1);

    // ring backend → ECDSA P-256 (RSA is unavailable on this backend).
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)
        .context("generating ECDSA P-256 key pair")?;
    let cert = params
        .self_signed(&key)
        .context("self-signing certificate")?;

    Ok((cert.pem(), key.serialize_pem()))
}

/// Ensure a TLS cert/key pair exists at `cert_path`/`key_path`.
///
/// Idempotent and upgrade-safe: if **both** files already exist this is a no-op,
/// so existing installs keep their current (possibly openssl-generated) cert
/// untouched. Otherwise a fresh self-signed ECDSA P-256 pair is generated and
/// written, the parent directory is created if missing (`mkdir -p` equivalent),
/// and unix permissions are set to key=0600 / cert=0644 (matching the old
/// init-script `chmod`).
pub fn ensure_self_signed_cert(cert_path: &Path, key_path: &Path) -> anyhow::Result<()> {
    if cert_path.exists() && key_path.exists() {
        return Ok(());
    }

    info!(
        "TLS cert/key missing at {} / {}; generating self-signed ECDSA P-256 certificate",
        cert_path.display(),
        key_path.display()
    );

    let (cert_pem, key_pem) = generate_self_signed_pem()?;

    // mkdir -p the parent TLS directory for both paths.
    for p in [cert_path, key_path] {
        if let Some(parent) = p.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).with_context(|| {
                    format!("creating TLS directory {}", parent.display())
                })?;
            }
        }
    }

    std::fs::write(key_path, key_pem.as_bytes())
        .with_context(|| format!("writing private key to {}", key_path.display()))?;
    std::fs::write(cert_path, cert_pem.as_bytes())
        .with_context(|| format!("writing certificate to {}", cert_path.display()))?;

    set_perms(key_path, 0o600)?;
    set_perms(cert_path, 0o644)?;

    info!(
        "Generated self-signed TLS certificate at {} (key {})",
        cert_path.display(),
        key_path.display()
    );

    Ok(())
}

/// Set file permissions on unix; no-op elsewhere (Windows preflight builds).
#[cfg(unix)]
fn set_perms(path: &Path, mode: u32) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let perms = std::fs::Permissions::from_mode(mode);
    std::fs::set_permissions(path, perms)
        .with_context(|| format!("setting permissions {:o} on {}", mode, path.display()))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_perms(_path: &Path, _mode: u32) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_loadable_ecdsa_pem_pair() {
        let dir = tempfile::tempdir().unwrap();
        // Nest under a not-yet-existing subdir to exercise the mkdir -p path.
        let cert_path = dir.path().join("tls").join("cert.pem");
        let key_path = dir.path().join("tls").join("key.pem");

        ensure_self_signed_cert(&cert_path, &key_path).unwrap();

        assert!(cert_path.exists(), "cert.pem should have been written");
        assert!(key_path.exists(), "key.pem should have been written");

        let cert_pem = std::fs::read_to_string(&cert_path).unwrap();
        let key_pem = std::fs::read_to_string(&key_path).unwrap();
        assert!(cert_pem.contains("BEGIN CERTIFICATE"));
        assert!(key_pem.contains("BEGIN PRIVATE KEY"));

        // rustls round-trip: the generated PEM pair must parse as a usable
        // cert chain + private key.
        use rustls_pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer};
        let certs: Vec<CertificateDer<'static>> =
            CertificateDer::pem_slice_iter(cert_pem.as_bytes())
                .collect::<Result<_, _>>()
                .expect("cert PEM should parse");
        assert_eq!(certs.len(), 1, "expected exactly one certificate");

        let key = PrivateKeyDer::from_pem_slice(key_pem.as_bytes())
            .expect("key PEM should parse");
        // ring backend emits PKCS#8 (SEC1 would be Pkcs1/Sec1); ECDSA P-256
        // keys serialize as PKCS#8.
        assert!(
            matches!(key, PrivateKeyDer::Pkcs8(_)),
            "ECDSA P-256 key should be PKCS#8, got {key:?}"
        );

        // The cert must build a valid rustls server config (full crypto load).
        let provider = std::sync::Arc::new(rustls::crypto::ring::default_provider());
        rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .expect("rustls should accept the generated cert+key");
    }

    #[test]
    fn is_idempotent_and_leaves_existing_cert_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let cert_path = dir.path().join("cert.pem");
        let key_path = dir.path().join("key.pem");

        // Pre-seed sentinel content as if an existing install already had a cert.
        std::fs::write(&cert_path, b"EXISTING-CERT").unwrap();
        std::fs::write(&key_path, b"EXISTING-KEY").unwrap();

        ensure_self_signed_cert(&cert_path, &key_path).unwrap();

        assert_eq!(std::fs::read(&cert_path).unwrap(), b"EXISTING-CERT");
        assert_eq!(std::fs::read(&key_path).unwrap(), b"EXISTING-KEY");
    }

    #[cfg(unix)]
    #[test]
    fn sets_unix_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let cert_path = dir.path().join("cert.pem");
        let key_path = dir.path().join("key.pem");

        ensure_self_signed_cert(&cert_path, &key_path).unwrap();

        let key_mode = std::fs::metadata(&key_path).unwrap().permissions().mode() & 0o777;
        let cert_mode = std::fs::metadata(&cert_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(key_mode, 0o600, "key.pem should be 0600");
        assert_eq!(cert_mode, 0o644, "cert.pem should be 0644");
    }
}
