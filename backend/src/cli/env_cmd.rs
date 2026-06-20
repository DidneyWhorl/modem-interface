use super::{cli_eprintln, cli_println};
use std::ffi::OsString;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Local};

const CONFIG_PATH: &str = "/etc/modem-interface/config.toml";
const APK_ARCH_PATH: &str = "/etc/apk/arch";
const APK_REPO_PATH: &str = "/etc/apk/repositories.d/ctrl-modem.list";
const OPKG_REPO_PATH: &str = "/etc/opkg/customfeeds.conf";
const LICENSE_KEY_PATH: &str = "/etc/modem-interface/license.key";
const LICENSE_DIR: &str = "/etc/modem-interface";
const SERVICE_INIT: &str = "/etc/init.d/modem-interface";
// Both daemon shapes are probed in order (see wait_for_health):
//  - TLS active: app router serves HTTPS on :8443 (axum_server::bind_rustls)
//  - TLS off:    app router (incl. /health) serves plain HTTP on :8080
// In TLS+redirect mode the :8080 listener answers 307, which the probe
// deliberately rejects, so the HTTP fallback can never vacuously pass.
const HEALTH_URL_HTTPS: &str = "https://127.0.0.1:8443/health";
const HEALTH_URL_HTTP: &str = "http://127.0.0.1:8080/health";
const HEALTH_POLL_BUDGET: Duration = Duration::from_secs(30);

pub async fn run(args: &[OsString]) -> u8 {
    let Some(action) = args.first().and_then(|s| s.to_str()) else {
        cli_eprintln!("usage: modem-interface env {{show|set <name>}}");
        return 2;
    };
    match action {
        "show" => show().await,
        "set" => {
            let Some(name) = args.get(1).and_then(|s| s.to_str()) else {
                cli_eprintln!("usage: modem-interface env set <name>");
                return 2;
            };
            set(name).await
        }
        _ => {
            cli_eprintln!("unknown env subcommand: {action}");
            2
        }
    }
}

async fn print_license_slot_row(label: &str, filename: &str, path: &str) {
    let row = match tokio::fs::metadata(path).await {
        Ok(meta) => {
            let mtime = meta
                .modified()
                .ok()
                .map(|t| DateTime::<Local>::from(t).format("%Y-%m-%d %H:%M:%S").to_string())
                .unwrap_or_else(|| "?".to_string());
            format!("present (mtime {mtime})")
        }
        Err(_) => "absent".to_string(),
    };
    cli_println!("  {label:<11}   {filename:<23}  {row}");
}

async fn show() -> u8 {
    let config_path = "/etc/modem-interface/config.toml";
    let contents = match tokio::fs::read_to_string(config_path).await {
        Ok(s) => s,
        Err(e) => {
            cli_eprintln!("cannot read {config_path}: {e}");
            return 1;
        }
    };
    #[derive(serde::Deserialize)]
    struct Shim {
        #[serde(default)]
        portal: crate::config::PortalConfig,
    }
    let parsed: Shim = match toml::from_str(&contents) {
        Ok(p) => p,
        Err(e) => {
            cli_eprintln!("config parse error: {e}");
            return 1;
        }
    };
    let env_name = crate::config::EnvConfig::from_portal_base(&parsed.portal.base_url)
        .unwrap_or("(custom)");
    let arch = tokio::fs::read_to_string("/etc/apk/arch")
        .await
        .ok()
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "<unknown-arch>".to_string());

    cli_println!("Environment:  {env_name}");
    cli_println!("Portal:       {}", parsed.portal.base_url.trim_end_matches('/'));
    cli_println!("Tunnel:       {}", parsed.portal.resolved_tunnel_url());
    if let Some(e) = crate::config::EnvConfig::lookup(env_name) {
        cli_println!("APK feed:     {}/{}", e.apk_feed, arch);
        cli_println!("Opkg feed:    {}/{}", e.opkg_feed, arch);
    } else {
        cli_println!("APK feed:     (custom — not resolvable)");
        cli_println!("Opkg feed:    (custom — not resolvable)");
    }
    cli_println!("Config file:  {config_path}");
    cli_println!();
    cli_println!("License slots:");
    print_license_slot_row("active", "license.key", "/etc/modem-interface/license.key").await;
    print_license_slot_row("production", "license.production.key", "/etc/modem-interface/license.production.key").await;
    print_license_slot_row("staging", "license.staging.key", "/etc/modem-interface/license.staging.key").await;
    print_license_slot_row("override", "license.pub", "/etc/modem-interface/license.pub").await;
    0
}

async fn set(name: &str) -> u8 {
    set_with_hooks_and_paths(
        name,
        Path::new(CONFIG_PATH),
        Path::new(APK_ARCH_PATH),
        Path::new(APK_REPO_PATH),
        Path::new(OPKG_REPO_PATH),
        Path::new(LICENSE_KEY_PATH),
        Path::new(LICENSE_DIR),
        Path::new(SERVICE_INIT),
        &[HEALTH_URL_HTTPS.to_string(), HEALTH_URL_HTTP.to_string()],
        HEALTH_POLL_BUDGET,
        |init: PathBuf| async move { restart_service(&init).await },
        |urls: Vec<String>, budget| async move { wait_for_health(&urls, budget).await },
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn set_with_hooks_and_paths<R, RF, W, WF>(
    name: &str,
    config_path: &Path,
    apk_arch_path: &Path,
    apk_repo_path: &Path,
    opkg_repo_path: &Path,
    license_key_path: &Path,
    license_dir: &Path,
    service_init: &Path,
    health_urls: &[String],
    health_budget: Duration,
    restart: R,
    wait_health: W,
) -> u8
where
    R: Fn(PathBuf) -> RF,
    RF: Future<Output = std::io::Result<()>>,
    W: Fn(Vec<String>, Duration) -> WF,
    WF: Future<Output = std::io::Result<()>>,
{
    let Some(env) = crate::config::EnvConfig::lookup(name) else {
        let known: Vec<&str> = crate::config::EnvConfig::all().iter().map(|(n, _)| *n).collect();
        cli_eprintln!("unknown environment: '{name}'. Known: {}", known.join(", "));
        return 2;
    };

    cli_println!("Switching to environment: {name}");

    let snapshot = match FileSnapshot::new(&[config_path, apk_repo_path, opkg_repo_path, license_key_path]).await {
        Ok(s) => s,
        Err(e) => {
            cli_eprintln!("env set: failed to snapshot config files: {e}");
            return 1;
        }
    };

    let result = apply_env(
        &env,
        name,
        config_path,
        apk_arch_path,
        apk_repo_path,
        opkg_repo_path,
        license_key_path,
        license_dir,
        service_init,
        health_urls,
        health_budget,
        &restart,
        &wait_health,
    )
    .await;

    match result {
        Ok(()) => {
            snapshot.commit();
            cli_println!("Environment switched to {name}. Run 'apk update && apk upgrade modem-interface' to pull matching packages.");
            0
        }
        Err(e) => {
            cli_eprintln!("env set failed: {e}. Rolling back...");
            snapshot.restore().await;
            let _ = restart(service_init.to_path_buf()).await;
            cli_eprintln!("Rollback complete. Router remains on previous environment.");
            1
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn apply_env<R, RF, W, WF>(
    env: &crate::config::EnvConfig,
    new_env_name: &str,
    config_path: &Path,
    apk_arch_path: &Path,
    apk_repo_path: &Path,
    opkg_repo_path: &Path,
    license_key_path: &Path,
    license_dir: &Path,
    service_init: &Path,
    health_urls: &[String],
    health_budget: Duration,
    restart: R,
    wait_health: W,
) -> std::io::Result<()>
where
    R: Fn(PathBuf) -> RF,
    RF: Future<Output = std::io::Result<()>>,
    W: Fn(Vec<String>, Duration) -> WF,
    WF: Future<Output = std::io::Result<()>>,
{
    // 1. Detect old env from existing config.toml BEFORE we rewrite it.
    let old_env_name = detect_current_env_name(config_path).await;

    // 2. Migration claim — copy active license to OLD env's sidecar IFF
    //    sidecar is absent and active is present. No-op for "(custom)".
    if old_env_name == "production" || old_env_name == "staging" {
        let old_sidecar = license_dir.join(format!("license.{old_env_name}.key"));
        claim_existing_license_as_sidecar(license_key_path, &old_sidecar).await?;
    }

    // 3. Rewrite repo + portal files (existing).
    rewrite_portal_section(env, config_path).await?;
    rewrite_apk_repo(env, apk_repo_path).await?;
    rewrite_opkg_repo(env, apk_arch_path, opkg_repo_path).await?;

    // 4. Slot swap — copy NEW env's sidecar to active, or remove active.
    let new_sidecar = license_dir.join(format!("license.{new_env_name}.key"));
    copy_active_from_env_sidecar(license_key_path, &new_sidecar).await?;

    // 5. Restart + health (existing).
    restart(service_init.to_path_buf()).await?;
    wait_health(health_urls.to_vec(), health_budget).await?;
    Ok(())
}

/// Read the current portal base_url from config.toml (best-effort) and
/// resolve to a canonical env name. Returns "(custom)" on parse error,
/// missing file, or unknown URL — matches the behavior of
/// `current_env(...)` for non-canonical portals.
async fn detect_current_env_name(config_path: &Path) -> &'static str {
    let contents = match tokio::fs::read_to_string(config_path).await {
        Ok(s) => s,
        Err(_) => return "(custom)",
    };
    #[derive(serde::Deserialize)]
    struct Shim {
        #[serde(default)]
        portal: crate::config::PortalConfig,
    }
    let parsed: Shim = match toml::from_str(&contents) {
        Ok(p) => p,
        Err(_) => return "(custom)",
    };
    crate::config::current_env(&parsed.portal.base_url)
}

// --- File snapshot + rollback ---

struct FileSnapshot {
    files: Vec<(PathBuf, Option<Vec<u8>>)>,
    committed: bool,
}

impl FileSnapshot {
    async fn new(paths: &[&Path]) -> std::io::Result<Self> {
        let mut files = Vec::with_capacity(paths.len());
        for &p in paths {
            let contents = match tokio::fs::read(p).await {
                Ok(b) => Some(b),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => return Err(e),
            };
            files.push((p.to_path_buf(), contents));
        }
        Ok(Self { files, committed: false })
    }

    async fn restore(mut self) {
        for (path, contents) in &self.files {
            match contents {
                Some(b) => {
                    if let Err(e) = atomic_write(path, b).await {
                        cli_eprintln!("rollback: failed to restore {}: {e}", path.display());
                    }
                }
                None => {
                    if let Err(e) = tokio::fs::remove_file(path).await {
                        if e.kind() != std::io::ErrorKind::NotFound {
                            cli_eprintln!("rollback: failed to remove {}: {e}", path.display());
                        }
                    }
                }
            }
        }
        self.committed = true;
    }

    fn commit(mut self) {
        self.committed = true;
    }
}

impl Drop for FileSnapshot {
    // Panic-safety net: sync-restore originals if we exit without committing.
    // The real rollback path uses restore().await above.
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        for (path, contents) in &self.files {
            match contents {
                Some(b) => {
                    if let Some(parent) = path.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    if let Some(name) = path.file_name() {
                        let tmp = path.with_file_name(format!(".{}.tmp", name.to_string_lossy()));
                        let _ = std::fs::write(&tmp, b).and_then(|_| std::fs::rename(&tmp, path));
                    }
                }
                None => {
                    let _ = std::fs::remove_file(path);
                }
            }
        }
    }
}

// --- Helpers ---

/// Copy the active license to the env-specific sidecar IFF the sidecar
/// is absent and the active is present. Spec §3 step 3.
///
/// This fires on the first `env set <known>` after a 1b upgrade so the
/// existing license is preserved as a sidecar before the slot swap. After
/// the first claim, this is a no-op (sidecar already exists).
async fn claim_existing_license_as_sidecar(
    active: &Path,
    sidecar: &Path,
) -> std::io::Result<()> {
    let active_bytes = match tokio::fs::read(active).await {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    if tokio::fs::metadata(sidecar).await.is_ok() {
        // Sidecar already exists — claim has already been done. Idempotent no-op.
        return Ok(());
    }
    atomic_write(sidecar, &active_bytes).await
}

/// Set the active license slot from the env-specific sidecar. If the
/// sidecar exists, copy it to the active path (atomic_write). If the
/// sidecar is absent, remove the active path so the bench enters the
/// Unlicensed state. Spec §3 step 7.
///
/// Absent active path during removal is not an error.
async fn copy_active_from_env_sidecar(
    active: &Path,
    sidecar: &Path,
) -> std::io::Result<()> {
    match tokio::fs::read(sidecar).await {
        Ok(bytes) => atomic_write(active, &bytes).await,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Sidecar absent — remove active so we land in Unlicensed state.
            match tokio::fs::remove_file(active).await {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e),
            }
        }
        Err(e) => Err(e),
    }
}

async fn atomic_write(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let tmp = match path.file_name() {
        Some(name) => path.with_file_name(format!(".{}.tmp", name.to_string_lossy())),
        None => return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "path has no filename")),
    };
    // Write the temp file with 0600 perms BEFORE the rename so the target
    // never exists world-readable, even momentarily (H4/#3a hardening — these
    // files carry auth/TLS/portal config and license keys on the root-run
    // router). On non-unix, fall back to a plain write so the crate still
    // builds/tests on Windows.
    #[cfg(unix)]
    {
        use tokio::io::AsyncWriteExt;
        // A temp left over from a crashed prior run would make create_new fail;
        // clear it first. (Absent file is fine.)
        match tokio::fs::remove_file(&tmp).await {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        let mut f = tokio::fs::OpenOptions::new()
            .mode(0o600)
            .create_new(true)
            .write(true)
            .open(&tmp)
            .await?;
        // On any write failure, clean up the restricted temp file so a stale
        // temp doesn't linger; propagate the original error.
        if let Err(e) = async {
            f.write_all(contents).await?;
            f.flush().await
        }
        .await
        {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(e);
        }
        drop(f);
    }
    #[cfg(not(unix))]
    {
        tokio::fs::write(&tmp, contents).await?;
    }

    tokio::fs::rename(&tmp, path).await?;
    Ok(())
}

/// Returns true if `line` (already trimmed) is a `[table]` section header
/// (single brackets only — `[[array]]` headers return false, matching the
/// semantics used throughout this module for section boundary detection).
fn is_table_header(trimmed: &str) -> bool {
    trimmed.starts_with('[') && !trimmed.starts_with("[[")
}

/// Returns true if `line` is a `base_url = "..."` assignment within a [portal]
/// section.  Only the line whose trimmed form starts with `base_url` followed
/// by optional whitespace and `=` is matched; comment lines are never matched.
fn is_base_url_line(line: &str) -> bool {
    let t = line.trim_start();
    if t.starts_with('#') {
        return false;
    }
    t.starts_with("base_url") && t[8..].trim_start().starts_with('=')
}

/// Same matching logic for `tunnel_url`.
fn is_tunnel_url_line(line: &str) -> bool {
    let t = line.trim_start();
    if t.starts_with('#') {
        return false;
    }
    t.starts_with("tunnel_url") && t[10..].trim_start().starts_with('=')
}

async fn rewrite_portal_section(
    env: &crate::config::EnvConfig,
    config_path: &Path,
) -> std::io::Result<()> {
    let existing = match tokio::fs::read_to_string(config_path).await {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };

    // Detect whether a [portal] section is present.
    let has_portal = existing
        .lines()
        .any(|l| l.trim() == "[portal]");

    if !has_portal {
        // Case A: [portal] absent — append a minimal block (v1.1.0 upgrade path).
        let mut body = existing.clone();
        let trimmed_len = body.trim_end().len();
        body.truncate(trimmed_len);
        if !body.is_empty() {
            body.push('\n');
        }
        body.push_str(&format!(
            "\n[portal]\nbase_url = \"{}\"\ntunnel_url = \"{}\"\n",
            env.portal_base, env.tunnel_url,
        ));
        return atomic_write(config_path, body.as_bytes()).await;
    }

    // Case B: [portal] present — walk lines in-place, replacing only the two
    // value lines.  All other bytes (comments, blank lines, extra keys) are
    // preserved verbatim.
    let new_base = format!("base_url = \"{}\"", env.portal_base);
    let new_tunnel = format!("tunnel_url = \"{}\"", env.tunnel_url);

    let mut out = String::with_capacity(existing.len() + 64);
    let mut in_portal = false;
    let mut found_base = false;
    let mut found_tunnel = false;

    let lines: Vec<&str> = existing.lines().collect();
    let total = lines.len();
    let mut i = 0;

    while i < total {
        let line = lines[i];
        let trimmed = line.trim();

        if trimmed == "[portal]" {
            in_portal = true;
            out.push_str(line);
            out.push('\n');
            i += 1;
            continue;
        }

        if in_portal && is_table_header(trimmed) {
            // Leaving the [portal] section.  Append any missing keys before
            // the next section header.
            if !found_base {
                out.push_str(&new_base);
                out.push('\n');
            }
            if !found_tunnel {
                out.push_str(&new_tunnel);
                out.push('\n');
            }
            in_portal = false;
        }

        if in_portal {
            if is_base_url_line(line) {
                out.push_str(&new_base);
                out.push('\n');
                found_base = true;
            } else if is_tunnel_url_line(line) {
                out.push_str(&new_tunnel);
                out.push('\n');
                found_tunnel = true;
            } else {
                out.push_str(line);
                out.push('\n');
            }
        } else {
            out.push_str(line);
            out.push('\n');
        }

        i += 1;
    }

    // If [portal] was the last section (no trailing table header), append any
    // missing keys now.
    if in_portal {
        if !found_base {
            out.push_str(&new_base);
            out.push('\n');
        }
        if !found_tunnel {
            out.push_str(&new_tunnel);
            out.push('\n');
        }
    }

    atomic_write(config_path, out.as_bytes()).await
}

async fn rewrite_apk_repo(
    env: &crate::config::EnvConfig,
    apk_repo_path: &Path,
) -> std::io::Result<()> {
    // apk-tools auto-appends `/<arch>/APKINDEX.tar.gz` to each repo line when
    // fetching the index, so we write the arch-less base URL here. Writing
    // `<feed>/<arch>` would produce a doubled path like
    // `/stable/apk/<arch>/<arch>/APKINDEX.tar.gz` → nginx 404.
    let line = format!("{}\n", env.apk_feed);
    atomic_write(apk_repo_path, line.as_bytes()).await
}

async fn rewrite_opkg_repo(
    env: &crate::config::EnvConfig,
    apk_arch_path: &Path,
    opkg_repo_path: &Path,
) -> std::io::Result<()> {
    // Pure-apk routers have no opkg file; if either the feed file or arch
    // sentinel is missing, there's nothing to rewrite.
    let existing = match tokio::fs::read_to_string(opkg_repo_path).await {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    let arch = match tokio::fs::read_to_string(apk_arch_path).await {
        Ok(s) => s.trim().to_string(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    let new_line = format!("src/gz ctrl_modem {}/{arch}", env.opkg_feed);
    let mut out = String::with_capacity(existing.len() + new_line.len() + 1);
    let mut replaced = false;
    for line in existing.lines() {
        if line.contains("packages.ctrl-modem.com") {
            if !replaced {
                out.push_str(&new_line);
                out.push('\n');
                replaced = true;
            }
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    if !replaced {
        out.push_str(&new_line);
        out.push('\n');
    }
    atomic_write(opkg_repo_path, out.as_bytes()).await
}

async fn restart_service(service_init: &Path) -> std::io::Result<()> {
    let status = tokio::process::Command::new(service_init)
        .arg("restart")
        .status()
        .await?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!("init.d restart exited with {status}")))
    }
}

// --- Native health probe (env-set wolfssl fix) ---
//
// Previously this shelled out to `uclient-fetch --no-check-certificate`. On
// OpenWrt 22.03 / libustream-wolfssl (hardware repro 2026-06-10: ZBT-WG3526,
// 22.03.7) uclient-fetch cannot complete a TLS handshake against the daemon's
// rustls listener (rc=4 every attempt), so `env set` always timed out and
// rolled back a healthy daemon. The CLI is the same binary as the daemon and
// already links tokio + rustls, so the probe is now performed natively — no
// dependency on the platform TLS stack or any external fetcher.

/// One probe target parsed from a health URL. Only the http/https loopback
/// forms used by `env set` are supported (no IPv6 literals, no userinfo).
#[derive(Debug, PartialEq, Eq)]
struct HealthTarget {
    tls: bool,
    host: String,
    port: u16,
    path: String,
}

fn parse_health_url(url: &str) -> Option<HealthTarget> {
    let (tls, rest) = if let Some(r) = url.strip_prefix("https://") {
        (true, r)
    } else if let Some(r) = url.strip_prefix("http://") {
        (false, r)
    } else {
        return None;
    };
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], rest[i..].to_string()),
        None => (rest, "/".to_string()),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (h, p.parse().ok()?),
        None => (authority, if tls { 443 } else { 80 }),
    };
    if host.is_empty() {
        return None;
    }
    Some(HealthTarget { tls, host: host.to_string(), port, path })
}

/// Trust-anything certificate verifier for the localhost health probe —
/// parity with the removed `uclient-fetch --no-check-certificate`. The probe
/// only ever talks to 127.0.0.1, where the daemon serves a self-signed cert;
/// what the health gate proves is liveness, not certificate trust. Handshake
/// signatures are still cryptographically verified.
mod danger {
    use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
    use rustls::crypto::WebPkiSupportedAlgorithms;
    use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
    use rustls::{DigitallySignedStruct, Error, SignatureScheme};

    #[derive(Debug)]
    pub(super) struct AcceptAnyServerCert(WebPkiSupportedAlgorithms);

    impl AcceptAnyServerCert {
        pub(super) fn new(algs: WebPkiSupportedAlgorithms) -> Self {
            Self(algs)
        }
    }

    impl ServerCertVerifier for AcceptAnyServerCert {
        fn verify_server_cert(
            &self,
            _end_entity: &CertificateDer<'_>,
            _intermediates: &[CertificateDer<'_>],
            _server_name: &ServerName<'_>,
            _ocsp_response: &[u8],
            _now: UnixTime,
        ) -> Result<ServerCertVerified, Error> {
            Ok(ServerCertVerified::assertion())
        }

        fn verify_tls12_signature(
            &self,
            message: &[u8],
            cert: &CertificateDer<'_>,
            dss: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, Error> {
            rustls::crypto::verify_tls12_signature(message, cert, dss, &self.0)
        }

        fn verify_tls13_signature(
            &self,
            message: &[u8],
            cert: &CertificateDer<'_>,
            dss: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, Error> {
            rustls::crypto::verify_tls13_signature(message, cert, dss, &self.0)
        }

        fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
            self.0.supported_schemes()
        }
    }
}

fn no_verify_tls_connector() -> std::io::Result<tokio_rustls::TlsConnector> {
    let provider = std::sync::Arc::new(rustls::crypto::ring::default_provider());
    let algs = provider.signature_verification_algorithms;
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(std::io::Error::other)?
        .dangerous()
        .with_custom_certificate_verifier(std::sync::Arc::new(danger::AcceptAnyServerCert::new(algs)))
        .with_no_client_auth();
    Ok(tokio_rustls::TlsConnector::from(std::sync::Arc::new(config)))
}

/// Send `GET {path}` over an established stream and succeed iff the response
/// status is 2xx. A 307 from the TLS-redirect listener on :8080 or any error
/// status is a failure — only the real app router answering /health counts.
async fn check_http_2xx<S>(stream: &mut S, target: &HealthTarget) -> std::io::Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}:{}\r\nUser-Agent: modem-interface-env-set\r\nAccept: */*\r\nConnection: close\r\n\r\n",
        target.path, target.host, target.port
    );
    stream.write_all(request.as_bytes()).await?;
    // Read until the status line is complete (first CRLF), the server closes,
    // or 1 KiB — far more than any status line needs.
    let mut buf: Vec<u8> = Vec::with_capacity(256);
    let mut chunk = [0u8; 256];
    while !buf.windows(2).any(|w| w == b"\r\n") && buf.len() < 1024 {
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let head = String::from_utf8_lossy(&buf);
    let status_line = head.lines().next().unwrap_or("");
    // Anchor on "HTTP/" so a non-HTTP service whose banner happens to carry
    // a 2xx-looking token (e.g. "ICY 200 OK") cannot count as healthy.
    let status: u16 = status_line
        .strip_prefix("HTTP/")
        .and_then(|rest| rest.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("malformed HTTP status line: {status_line:?}"),
            )
        })?;
    if (200..300).contains(&status) {
        Ok(())
    } else {
        Err(std::io::Error::other(format!("health endpoint returned HTTP {status}")))
    }
}

/// One probe attempt against a single target (TCP connect, optional TLS
/// handshake, HTTP GET, 2xx check).
async fn probe_health_once(
    target: &HealthTarget,
    tls_connector: &tokio_rustls::TlsConnector,
) -> std::io::Result<()> {
    let stream = tokio::net::TcpStream::connect((target.host.as_str(), target.port)).await?;
    if target.tls {
        let server_name = rustls::pki_types::ServerName::try_from(target.host.clone())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
        let mut tls = tls_connector.connect(server_name, stream).await?;
        check_http_2xx(&mut tls, target).await
    } else {
        let mut stream = stream;
        check_http_2xx(&mut stream, target).await
    }
}

/// Poll the daemon's /health until a 2xx response or budget exhaustion.
/// `urls` are tried in order on each 500 ms tick; the first 2xx wins. The
/// default list (HTTPS :8443, then HTTP :8080) covers both the TLS-active
/// and TLS-off daemon shapes without weakening the gate: in TLS mode the
/// :8080 redirect listener answers 307, which is rejected above.
async fn wait_for_health(urls: &[String], budget: Duration) -> std::io::Result<()> {
    const PER_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(3);
    let targets: Vec<(&String, HealthTarget)> = urls
        .iter()
        .filter_map(|u| parse_health_url(u).map(|t| (u, t)))
        .collect();
    if targets.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "no valid health URLs to probe",
        ));
    }
    let tls_connector = no_verify_tls_connector()?;
    let deadline = tokio::time::Instant::now() + budget;
    // Carried into the TimedOut message so one log line shows WHY the gate
    // failed (the wolfssl bug needed hardware repro because the old probe's
    // failure mode was opaque).
    let mut last_error: Option<String> = None;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            let secs = budget.as_secs();
            let detail = last_error.map(|e| format!(" (last: {e})")).unwrap_or_default();
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("service did not become healthy within {secs}s{detail}"),
            ));
        }
        for (url, target) in &targets {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            let per_attempt = PER_ATTEMPT_TIMEOUT.min(remaining);
            match tokio::time::timeout(per_attempt, probe_health_once(target, &tls_connector))
                .await
            {
                Ok(Ok(())) => return Ok(()),
                // Not healthy yet — record why, try next target/tick.
                Ok(Err(e)) => last_error = Some(format!("{url}: {e}")),
                Err(_) => {
                    last_error =
                        Some(format!("{url}: no response within {}ms", per_attempt.as_millis()));
                }
            }
        }
        // Bound the inter-tick sleep to the remaining budget so wall-clock
        // cannot overshoot the budget before TimedOut is reported.
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        tokio::time::sleep(Duration::from_millis(500).min(remaining)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn production_env() -> crate::config::EnvConfig {
        crate::config::EnvConfig::lookup("production").unwrap()
    }

    fn staging_env() -> crate::config::EnvConfig {
        crate::config::EnvConfig::lookup("staging").unwrap()
    }

    // --- atomic_write tests ---

    #[tokio::test]
    async fn atomic_write_creates_new_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("new.txt");
        atomic_write(&path, b"hello").await.unwrap();
        let got = tokio::fs::read(&path).await.unwrap();
        assert_eq!(got, b"hello");
    }

    #[tokio::test]
    async fn atomic_write_replaces_existing_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("existing.txt");
        tokio::fs::write(&path, b"old content").await.unwrap();
        atomic_write(&path, b"new content").await.unwrap();
        let got = tokio::fs::read(&path).await.unwrap();
        assert_eq!(got, b"new content");
    }

    /// Regression guard for H4/#3a: env-set writes sensitive files
    /// (config.toml, per-env license keys). They must land 0600, not at
    /// the process umask (0644, world-readable) on the root-run router.
    #[cfg(unix)]
    #[tokio::test]
    async fn atomic_write_creates_file_with_0600_perms() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let path = dir.path().join("secret.key");
        atomic_write(&path, b"sensitive").await.unwrap();
        let mode = tokio::fs::metadata(&path).await.unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "file must be owner-only (0600)");
    }

    /// A stale temp file from a crashed prior run must not break the write
    /// (create_new would otherwise error), and the result must still be 0600.
    #[cfg(unix)]
    #[tokio::test]
    async fn atomic_write_recovers_from_stale_temp_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let stale = path.with_file_name(".config.toml.tmp");
        tokio::fs::write(&stale, b"garbage from crash").await.unwrap();
        atomic_write(&path, b"fresh").await.unwrap();
        let got = tokio::fs::read(&path).await.unwrap();
        assert_eq!(got, b"fresh");
        let mode = tokio::fs::metadata(&path).await.unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    // --- rewrite_portal_section tests ---

    #[tokio::test]
    async fn rewrite_portal_section_replaces_existing_portal_block_with_new_env() {
        let dir = tempdir().unwrap();
        let config = dir.path().join("config.toml");
        let initial = "[other]\nkey = \"val\"\n\n[portal]\nbase_url = \"https://old.example.com\"\ntunnel_url = \"wss://old.example.com/api/v1/tunnel\"\n";
        tokio::fs::write(&config, initial).await.unwrap();

        let env = staging_env();
        rewrite_portal_section(&env, &config).await.unwrap();

        let out = tokio::fs::read_to_string(&config).await.unwrap();
        assert!(out.contains("[other]"));
        assert!(out.contains("[portal]"));
        assert!(out.contains("https://staging.ctrl-modem.com"));
        assert!(out.contains("wss://tunnel-staging.ctrl-modem.com"));
        assert!(!out.contains("old.example.com"));
    }

    #[tokio::test]
    async fn rewrite_portal_section_appends_when_portal_section_absent() {
        let dir = tempdir().unwrap();
        let config = dir.path().join("config.toml");
        tokio::fs::write(&config, "[other]\nkey = \"val\"\n").await.unwrap();

        let env = production_env();
        rewrite_portal_section(&env, &config).await.unwrap();

        let out = tokio::fs::read_to_string(&config).await.unwrap();
        assert!(out.contains("[other]"));
        assert!(out.contains("[portal]"));
        assert!(out.contains("https://portal.ctrl-modem.com"));
    }

    // --- rewrite_apk_repo tests ---

    #[tokio::test]
    async fn rewrite_apk_repo_writes_arch_less_base_url() {
        // apk-tools auto-appends `/<arch>/APKINDEX.tar.gz`, so the written line
        // must be the arch-less base URL. Writing `<feed>/<arch>` here produces
        // a doubled `<arch>/<arch>` path at fetch time (the defect this test guards).
        let dir = tempdir().unwrap();
        let repo_file = dir.path().join("ctrl-modem.list");

        let env = staging_env();
        rewrite_apk_repo(&env, &repo_file).await.unwrap();

        let out = tokio::fs::read_to_string(&repo_file).await.unwrap();
        assert_eq!(out.trim(), "https://packages.ctrl-modem.com/testing/apk");
    }

    // --- rewrite_opkg_repo tests ---

    #[tokio::test]
    async fn rewrite_opkg_repo_noop_when_opkg_file_missing() {
        let dir = tempdir().unwrap();
        let arch_file = dir.path().join("arch");
        let opkg_file = dir.path().join("nonexistent-customfeeds.conf");
        tokio::fs::write(&arch_file, "aarch64\n").await.unwrap();

        let env = production_env();
        let result = rewrite_opkg_repo(&env, &arch_file, &opkg_file).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn rewrite_opkg_repo_noop_when_arch_missing() {
        let dir = tempdir().unwrap();
        let arch_file = dir.path().join("nonexistent-arch");
        let opkg_file = dir.path().join("customfeeds.conf");
        tokio::fs::write(&opkg_file, "src/gz ctrl-modem https://old.example.com/feed/aarch64\n").await.unwrap();

        let env = production_env();
        let result = rewrite_opkg_repo(&env, &arch_file, &opkg_file).await;
        assert!(result.is_ok());
        // File must be unchanged since arch was missing
        let out = tokio::fs::read_to_string(&opkg_file).await.unwrap();
        assert!(out.contains("old.example.com"));
    }

    #[tokio::test]
    async fn rewrite_opkg_repo_strips_old_ctrl_modem_line_and_appends_new() {
        let dir = tempdir().unwrap();
        let arch_file = dir.path().join("arch");
        let opkg_file = dir.path().join("customfeeds.conf");
        tokio::fs::write(&arch_file, "aarch64_cortex-a53\n").await.unwrap();
        let initial = "src/gz core https://downloads.openwrt.org/feed/aarch64\nsrc/gz ctrl-modem https://old.packages.ctrl-modem.com/stable/feed/aarch64\nsrc/gz extra https://example.com/feed\n";
        tokio::fs::write(&opkg_file, initial).await.unwrap();

        let env = staging_env();
        rewrite_opkg_repo(&env, &arch_file, &opkg_file).await.unwrap();

        let out = tokio::fs::read_to_string(&opkg_file).await.unwrap();
        assert!(out.contains("src/gz ctrl_modem https://packages.ctrl-modem.com/testing/feed/aarch64_cortex-a53"));
        assert!(!out.contains("old.packages.ctrl-modem.com"));
        assert!(out.contains("src/gz core"));
        assert!(out.contains("src/gz extra"));
    }

    // --- set_with_hooks_and_paths round-trip + rollback tests ---

    struct SetFixture {
        _dir: tempfile::TempDir,
        config_path: PathBuf,
        apk_arch_path: PathBuf,
        apk_repo_path: PathBuf,
        opkg_repo_path: PathBuf,
        service_init: PathBuf,
        initial_config_bytes: Vec<u8>,
        initial_apk_repo_bytes: Vec<u8>,
        initial_opkg_repo_bytes: Vec<u8>,
    }

    async fn build_fixture() -> SetFixture {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        let apk_arch_path = dir.path().join("arch");
        let apk_repo_path = dir.path().join("ctrl-modem.list");
        let opkg_repo_path = dir.path().join("customfeeds.conf");
        let service_init = dir.path().join("init-stub");

        let initial_config = "[other]\nkey = \"val\"\n\n[portal]\nbase_url = \"https://old.example.com\"\ntunnel_url = \"wss://old.example.com/api/v1/tunnel\"\n";
        let initial_apk_repo = "https://old.packages.ctrl-modem.com/stable/apk/aarch64_cortex-a53\n";
        let initial_opkg_repo = "src/gz core https://downloads.openwrt.org/feed/aarch64\nsrc/gz ctrl-modem https://old.packages.ctrl-modem.com/stable/feed/aarch64\nsrc/gz extra https://example.com/feed\n";

        tokio::fs::write(&config_path, initial_config).await.unwrap();
        tokio::fs::write(&apk_arch_path, "aarch64_cortex-a53\n").await.unwrap();
        tokio::fs::write(&apk_repo_path, initial_apk_repo).await.unwrap();
        tokio::fs::write(&opkg_repo_path, initial_opkg_repo).await.unwrap();

        let initial_config_bytes = tokio::fs::read(&config_path).await.unwrap();
        let initial_apk_repo_bytes = tokio::fs::read(&apk_repo_path).await.unwrap();
        let initial_opkg_repo_bytes = tokio::fs::read(&opkg_repo_path).await.unwrap();

        SetFixture {
            _dir: dir,
            config_path,
            apk_arch_path,
            apk_repo_path,
            opkg_repo_path,
            service_init,
            initial_config_bytes,
            initial_apk_repo_bytes,
            initial_opkg_repo_bytes,
        }
    }

    #[tokio::test]
    async fn set_with_hooks_happy_path_rewrites_all_three_files() {
        let fx = build_fixture().await;

        let code = set_with_hooks_and_paths(
            "staging",
            &fx.config_path,
            &fx.apk_arch_path,
            &fx.apk_repo_path,
            &fx.opkg_repo_path,
            &fx._dir.path().join("license.key"),
            fx._dir.path(),
            &fx.service_init,
            &["https://127.0.0.1:0/health".to_string()],
            Duration::from_secs(1),
            |_init: PathBuf| async { Ok(()) },
            |_urls: Vec<String>, _budget| async { Ok(()) },
        )
        .await;

        assert_eq!(code, 0);

        let config_out = tokio::fs::read_to_string(&fx.config_path).await.unwrap();
        assert!(config_out.contains("[other]"));
        assert!(config_out.contains("key = \"val\""));
        assert!(config_out.contains("[portal]"));
        assert!(config_out.contains("https://staging.ctrl-modem.com"));
        assert!(config_out.contains("wss://tunnel-staging.ctrl-modem.com"));
        assert!(!config_out.contains("old.example.com"));

        let apk_out = tokio::fs::read_to_string(&fx.apk_repo_path).await.unwrap();
        assert_eq!(
            apk_out.trim(),
            "https://packages.ctrl-modem.com/testing/apk"
        );

        let opkg_out = tokio::fs::read_to_string(&fx.opkg_repo_path).await.unwrap();
        assert!(opkg_out.contains("src/gz ctrl_modem https://packages.ctrl-modem.com/testing/feed/aarch64_cortex-a53"));
        assert!(!opkg_out.contains("old.packages.ctrl-modem.com"));
        assert!(opkg_out.contains("src/gz core"));
        assert!(opkg_out.contains("src/gz extra"));
    }

    async fn assert_all_files_restored(fx: &SetFixture) {
        let config_after = tokio::fs::read(&fx.config_path).await.unwrap();
        let apk_after = tokio::fs::read(&fx.apk_repo_path).await.unwrap();
        let opkg_after = tokio::fs::read(&fx.opkg_repo_path).await.unwrap();
        assert_eq!(config_after, fx.initial_config_bytes);
        assert_eq!(apk_after, fx.initial_apk_repo_bytes);
        assert_eq!(opkg_after, fx.initial_opkg_repo_bytes);
    }

    #[tokio::test]
    async fn set_rolls_back_when_restart_fails() {
        let fx = build_fixture().await;

        let code = set_with_hooks_and_paths(
            "staging",
            &fx.config_path,
            &fx.apk_arch_path,
            &fx.apk_repo_path,
            &fx.opkg_repo_path,
            &fx._dir.path().join("license.key"),
            fx._dir.path(),
            &fx.service_init,
            &["https://127.0.0.1:0/health".to_string()],
            Duration::from_secs(1),
            |_init: PathBuf| async { Err(std::io::Error::other("simulated restart failure")) },
            |_urls: Vec<String>, _budget| async { Ok(()) },
        )
        .await;

        assert_eq!(code, 1);
        assert_all_files_restored(&fx).await;
    }

    #[tokio::test]
    async fn set_rolls_back_when_health_times_out() {
        let fx = build_fixture().await;

        let code = set_with_hooks_and_paths(
            "staging",
            &fx.config_path,
            &fx.apk_arch_path,
            &fx.apk_repo_path,
            &fx.opkg_repo_path,
            &fx._dir.path().join("license.key"),
            fx._dir.path(),
            &fx.service_init,
            &["https://127.0.0.1:0/health".to_string()],
            Duration::from_secs(1),
            |_init: PathBuf| async { Ok(()) },
            |_urls: Vec<String>, _budget| async {
                Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "simulated health timeout",
                ))
            },
        )
        .await;

        assert_eq!(code, 1);
        assert_all_files_restored(&fx).await;
    }

    #[tokio::test]
    async fn set_rolls_back_when_health_errors() {
        let fx = build_fixture().await;

        let code = set_with_hooks_and_paths(
            "staging",
            &fx.config_path,
            &fx.apk_arch_path,
            &fx.apk_repo_path,
            &fx.opkg_repo_path,
            &fx._dir.path().join("license.key"),
            fx._dir.path(),
            &fx.service_init,
            &["https://127.0.0.1:0/health".to_string()],
            Duration::from_secs(1),
            |_init: PathBuf| async { Ok(()) },
            |_urls: Vec<String>, _budget| async { Err(std::io::Error::other("simulated health error")) },
        )
        .await;

        assert_eq!(code, 1);
        assert_all_files_restored(&fx).await;
    }

    // --- rewrite_portal_section: comment-preservation tests (Defect 2) ---

    #[tokio::test]
    async fn rewrite_portal_section_preserves_comments_when_rewriting_values() {
        let dir = tempdir().unwrap();
        let config = dir.path().join("config.toml");
        // A [portal] section with comment lines interspersed with value lines.
        let initial = "[other]\nkey = \"val\"\n\n[portal]\n# This is a comment above base_url\nbase_url = \"https://portal.ctrl-modem.com\"\n# This is a comment above tunnel_url\ntunnel_url = \"wss://portal.ctrl-modem.com/api/v1/tunnel\"\n";
        tokio::fs::write(&config, initial).await.unwrap();

        let env = staging_env();
        rewrite_portal_section(&env, &config).await.unwrap();

        let out = tokio::fs::read_to_string(&config).await.unwrap();
        // Comments must be preserved verbatim.
        assert!(out.contains("# This is a comment above base_url"));
        assert!(out.contains("# This is a comment above tunnel_url"));
        // Values must be updated.
        assert!(out.contains("base_url = \"https://staging.ctrl-modem.com\""));
        assert!(out.contains("tunnel_url = \"wss://tunnel-staging.ctrl-modem.com/api/v1/tunnel\""));
        // Old URLs must be gone.
        assert!(!out.contains("portal.ctrl-modem.com"));
        // Other sections intact.
        assert!(out.contains("[other]"));
    }

    #[tokio::test]
    async fn rewrite_portal_section_round_trip_with_full_template_is_byte_identical() {
        // This is the PRIMARY regression test for Defect 2.
        // The input is the exact shape of the shipped openwrt/files/etc/modem-interface/config.toml
        // [portal] section (production URLs + 10 comment lines + blank line separator).
        // A P→S→P round trip MUST produce bytes identical to the original input.
        let dir = tempdir().unwrap();
        let config = dir.path().join("config.toml");

        // Exact text from the shipped template (openwrt/files/etc/modem-interface/config.toml)
        // including the preceding [tls] section to simulate the real file shape.
        let original = "\
[tls]\n\
enabled = true\n\
redirect_http = true\n\
\n\
[portal]\n\
# Base URL for all portal HTTPS calls (heartbeat, telemetry poll-config,\n\
# license activation). Switching this URL redirects every portal call in\n\
# one step. Use `modem-interface env set {production|staging}` instead of\n\
# editing by hand — the CLI also rewrites /etc/apk/repositories.d/ctrl-modem.list\n\
# atomically and verifies the service comes back healthy.\n\
base_url = \"https://portal.ctrl-modem.com\"\n\
\n\
# Tunnel WSS endpoint. If omitted, derived from base_url (scheme swapped\n\
# to wss://, path /api/v1/tunnel). Staging uses a separate hostname, so\n\
# this is made explicit there.\n\
tunnel_url = \"wss://portal.ctrl-modem.com/api/v1/tunnel\"\n";

        tokio::fs::write(&config, original).await.unwrap();

        // Production → Staging
        let staging = staging_env();
        rewrite_portal_section(&staging, &config).await.unwrap();

        // Staging → Production (round trip back)
        let production = production_env();
        rewrite_portal_section(&production, &config).await.unwrap();

        let after_round_trip = tokio::fs::read_to_string(&config).await.unwrap();
        assert_eq!(
            after_round_trip, original,
            "P→S→P round trip must produce byte-identical output to original"
        );
    }

    #[tokio::test]
    async fn rewrite_portal_section_preserves_unrelated_portal_keys_if_present() {
        let dir = tempdir().unwrap();
        let config = dir.path().join("config.toml");
        let initial = "[portal]\nbase_url = \"https://portal.ctrl-modem.com\"\ntunnel_url = \"wss://portal.ctrl-modem.com/api/v1/tunnel\"\ntimeout_secs = 30\n";
        tokio::fs::write(&config, initial).await.unwrap();

        let env = staging_env();
        rewrite_portal_section(&env, &config).await.unwrap();

        let out = tokio::fs::read_to_string(&config).await.unwrap();
        // The extra key must be preserved verbatim.
        assert!(out.contains("timeout_secs = 30"));
        // Values must be updated.
        assert!(out.contains("base_url = \"https://staging.ctrl-modem.com\""));
        assert!(out.contains("tunnel_url = \"wss://tunnel-staging.ctrl-modem.com/api/v1/tunnel\""));
    }

    #[tokio::test]
    async fn rewrite_portal_section_adds_missing_tunnel_url_to_present_section() {
        let dir = tempdir().unwrap();
        let config = dir.path().join("config.toml");
        // [portal] present with base_url but NO tunnel_url, followed by another [table].
        let initial = "[portal]\nbase_url = \"https://portal.ctrl-modem.com\"\n\n[other]\nkey = 1\n";
        tokio::fs::write(&config, initial).await.unwrap();

        let env = staging_env();
        rewrite_portal_section(&env, &config).await.unwrap();

        let out = tokio::fs::read_to_string(&config).await.unwrap();
        // base_url updated.
        assert!(out.contains("base_url = \"https://staging.ctrl-modem.com\""));
        // tunnel_url appended within the [portal] section (before [other]).
        assert!(out.contains("tunnel_url = \"wss://tunnel-staging.ctrl-modem.com/api/v1/tunnel\""));
        // [other] section still present.
        assert!(out.contains("[other]"));
        // tunnel_url must appear BEFORE [other] in the output.
        let tunnel_pos = out.find("tunnel_url").unwrap();
        let other_pos = out.find("[other]").unwrap();
        assert!(
            tunnel_pos < other_pos,
            "tunnel_url must appear inside [portal], before [other]"
        );
    }

    #[tokio::test]
    async fn rewrite_portal_section_adds_missing_base_url_to_present_section() {
        let dir = tempdir().unwrap();
        let config = dir.path().join("config.toml");
        // [portal] present with tunnel_url but NO base_url, followed by another [table].
        let initial = "[portal]\ntunnel_url = \"wss://portal.ctrl-modem.com/api/v1/tunnel\"\n\n[other]\nkey = 1\n";
        tokio::fs::write(&config, initial).await.unwrap();

        let env = staging_env();
        rewrite_portal_section(&env, &config).await.unwrap();

        let out = tokio::fs::read_to_string(&config).await.unwrap();
        // tunnel_url updated.
        assert!(out.contains("tunnel_url = \"wss://tunnel-staging.ctrl-modem.com/api/v1/tunnel\""));
        // base_url appended within the [portal] section (before [other]).
        assert!(out.contains("base_url = \"https://staging.ctrl-modem.com\""));
        // [other] section still present.
        assert!(out.contains("[other]"));
        // base_url must appear BEFORE [other] in the output.
        let base_pos = out.find("base_url").unwrap();
        let other_pos = out.find("[other]").unwrap();
        assert!(
            base_pos < other_pos,
            "base_url must appear inside [portal], before [other]"
        );
    }

    #[tokio::test]
    async fn rewrite_portal_section_portal_absent_still_appends_minimal_block() {
        // Codifies Case A: v1.1.0 upgrade path where config.toml predated [portal].
        // Only base_url and tunnel_url are appended; no comments added.
        let dir = tempdir().unwrap();
        let config = dir.path().join("config.toml");
        tokio::fs::write(&config, "[tls]\nenabled = true\n").await.unwrap();

        let env = production_env();
        rewrite_portal_section(&env, &config).await.unwrap();

        let out = tokio::fs::read_to_string(&config).await.unwrap();
        assert!(out.contains("[tls]"));
        assert!(out.contains("[portal]"));
        assert!(out.contains("base_url = \"https://portal.ctrl-modem.com\""));
        assert!(out.contains("tunnel_url = \"wss://portal.ctrl-modem.com/api/v1/tunnel\""));
        // Minimal block only — no comment lines added for the absent-section path.
        let portal_idx = out.find("[portal]").unwrap();
        let portal_section = &out[portal_idx..];
        assert!(!portal_section.contains('#'), "absent-section path must not inject comments");
    }

    #[tokio::test]
    async fn rewrite_portal_section_preserves_double_bracket_array_inside_portal() {
        // `[[array]]` headers inside [portal] must NOT be treated as a section boundary;
        // they must survive verbatim in the output, along with any trailing [other] section.
        let dir = tempdir().unwrap();
        let config = dir.path().join("config.toml");
        let initial = concat!(
            "[portal]\n",
            "base_url = \"https://old.example.com\"\n",
            "tunnel_url = \"wss://old.example.com/api/v1/tunnel\"\n",
            "\n",
            "[[something]]\n",
            "item = \"preserved\"\n",
            "\n",
            "[other]\n",
            "key = \"val\"\n",
        );
        tokio::fs::write(&config, initial).await.unwrap();

        let env = staging_env();
        rewrite_portal_section(&env, &config).await.unwrap();

        let out = tokio::fs::read_to_string(&config).await.unwrap();

        // URLs updated to staging values.
        assert!(out.contains("https://staging.ctrl-modem.com"), "base_url must be staging");
        assert!(out.contains("wss://tunnel-staging.ctrl-modem.com"), "tunnel_url must be staging");
        assert!(!out.contains("old.example.com"), "old URLs must be gone");

        // [[something]] block and its content are preserved verbatim.
        assert!(out.contains("[[something]]"), "[[something]] must survive rewrite");
        assert!(out.contains("item = \"preserved\""), "[[something]] body must survive rewrite");

        // [[something]] appears before [other] — original order maintained.
        let array_pos = out.find("[[something]]").unwrap();
        let other_pos = out.find("[other]").unwrap();
        assert!(array_pos < other_pos, "[[something]] must precede [other]");

        // [other] section is also present.
        assert!(out.contains("[other]"), "[other] section must be preserved");
    }

    // --- license slot helpers (Item #35 1b) ---

    #[tokio::test]
    async fn claim_copies_active_to_sidecar_when_sidecar_absent() {
        let dir = tempdir().unwrap();
        let active = dir.path().join("license.key");
        let sidecar = dir.path().join("license.production.key");
        tokio::fs::write(&active, b"prod-license-bytes").await.unwrap();

        claim_existing_license_as_sidecar(&active, &sidecar).await.unwrap();

        let got = tokio::fs::read(&sidecar).await.unwrap();
        assert_eq!(got, b"prod-license-bytes");
        // Active is preserved (claim is a copy, not a move).
        let active_after = tokio::fs::read(&active).await.unwrap();
        assert_eq!(active_after, b"prod-license-bytes");
    }

    #[tokio::test]
    async fn claim_is_noop_when_sidecar_already_exists() {
        let dir = tempdir().unwrap();
        let active = dir.path().join("license.key");
        let sidecar = dir.path().join("license.production.key");
        tokio::fs::write(&active, b"new-license-bytes").await.unwrap();
        tokio::fs::write(&sidecar, b"old-sidecar-bytes").await.unwrap();

        claim_existing_license_as_sidecar(&active, &sidecar).await.unwrap();

        // Sidecar must NOT be overwritten — claim only fires on first migration.
        let got = tokio::fs::read(&sidecar).await.unwrap();
        assert_eq!(got, b"old-sidecar-bytes");
    }

    #[tokio::test]
    async fn claim_is_noop_when_active_absent() {
        let dir = tempdir().unwrap();
        let active = dir.path().join("license.key");
        let sidecar = dir.path().join("license.production.key");
        // Neither file exists.

        claim_existing_license_as_sidecar(&active, &sidecar).await.unwrap();

        assert!(tokio::fs::metadata(&sidecar).await.is_err(),
            "sidecar must remain absent when active is absent");
    }

    #[tokio::test]
    async fn slot_swap_copies_sidecar_to_active_when_sidecar_present() {
        let dir = tempdir().unwrap();
        let active = dir.path().join("license.key");
        let sidecar = dir.path().join("license.staging.key");
        tokio::fs::write(&active, b"old-prod-bytes").await.unwrap();
        tokio::fs::write(&sidecar, b"new-staging-bytes").await.unwrap();

        copy_active_from_env_sidecar(&active, &sidecar).await.unwrap();

        let got = tokio::fs::read(&active).await.unwrap();
        assert_eq!(got, b"new-staging-bytes");
        // Sidecar is preserved (swap is a copy, not a move).
        let sidecar_after = tokio::fs::read(&sidecar).await.unwrap();
        assert_eq!(sidecar_after, b"new-staging-bytes");
    }

    #[tokio::test]
    async fn slot_swap_removes_active_when_sidecar_absent() {
        let dir = tempdir().unwrap();
        let active = dir.path().join("license.key");
        let sidecar = dir.path().join("license.staging.key");
        tokio::fs::write(&active, b"old-prod-bytes").await.unwrap();
        // Sidecar absent.

        copy_active_from_env_sidecar(&active, &sidecar).await.unwrap();

        assert!(tokio::fs::metadata(&active).await.is_err(),
            "active must be removed when sidecar is absent");
    }

    #[tokio::test]
    async fn slot_swap_idempotent_when_active_already_matches_sidecar() {
        let dir = tempdir().unwrap();
        let active = dir.path().join("license.key");
        let sidecar = dir.path().join("license.staging.key");
        tokio::fs::write(&active, b"staging-bytes").await.unwrap();
        tokio::fs::write(&sidecar, b"staging-bytes").await.unwrap();

        copy_active_from_env_sidecar(&active, &sidecar).await.unwrap();
        copy_active_from_env_sidecar(&active, &sidecar).await.unwrap(); // re-run

        let got = tokio::fs::read(&active).await.unwrap();
        assert_eq!(got, b"staging-bytes");
    }

    #[tokio::test]
    async fn slot_swap_noop_when_active_already_absent_and_sidecar_absent() {
        let dir = tempdir().unwrap();
        let active = dir.path().join("license.key");
        let sidecar = dir.path().join("license.staging.key");
        // Neither present.

        copy_active_from_env_sidecar(&active, &sidecar).await.unwrap();

        assert!(tokio::fs::metadata(&active).await.is_err());
    }

    // --- apply_env wiring (Task 7) ---

    #[tokio::test]
    async fn set_with_custom_portal_skips_claim_and_unlicenses_active_when_no_sidecar() {
        // Fixture's config.toml uses base_url = "https://old.example.com", so
        // detect_current_env_name returns "(custom)" and the migration claim
        // path is skipped. The slot swap to staging still runs: with no
        // staging sidecar present, the active license.key is removed and the
        // bench lands in the Unlicensed state. (The claim path itself is
        // covered by set_round_trip_restores_active_from_sidecar below, which
        // uses build_fixture_with_prod_portal.)
        let fx = build_fixture().await;
        let license_key_path = fx._dir.path().join("license.key");
        let license_dir = fx._dir.path().to_path_buf();
        tokio::fs::write(&license_key_path, b"existing-prod-license").await.unwrap();
        // No sidecars present yet.

        let code = set_with_hooks_and_paths(
            "staging",
            &fx.config_path,
            &fx.apk_arch_path,
            &fx.apk_repo_path,
            &fx.opkg_repo_path,
            &license_key_path,
            &license_dir,
            &fx.service_init,
            &["https://127.0.0.1:0/health".to_string()],
            Duration::from_secs(1),
            |_init: PathBuf| async { Ok(()) },
            |_urls: Vec<String>, _budget| async { Ok(()) },
        ).await;

        assert_eq!(code, 0);
        assert!(tokio::fs::metadata(&license_key_path).await.is_err(),
            "active license.key must be absent (Unlicensed) when staging sidecar absent");
    }

    /// Helper: build a fixture whose initial config.toml uses the production
    /// portal base_url so detect_current_env_name returns "production" and
    /// the migration claim path actually exercises.
    async fn build_fixture_with_prod_portal() -> SetFixture {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        let apk_arch_path = dir.path().join("arch");
        let apk_repo_path = dir.path().join("ctrl-modem.list");
        let opkg_repo_path = dir.path().join("customfeeds.conf");
        let service_init = dir.path().join("init-stub");

        let initial_config = "[other]\nkey = \"val\"\n\n[portal]\nbase_url = \"https://portal.ctrl-modem.com\"\ntunnel_url = \"wss://portal.ctrl-modem.com/api/v1/tunnel\"\n";
        let initial_apk_repo = "https://packages.ctrl-modem.com/stable/apk\n";
        let initial_opkg_repo = "src/gz core https://downloads.openwrt.org/feed/aarch64\nsrc/gz ctrl-modem https://packages.ctrl-modem.com/stable/feed/aarch64\nsrc/gz extra https://example.com/feed\n";

        tokio::fs::write(&config_path, initial_config).await.unwrap();
        tokio::fs::write(&apk_arch_path, "aarch64_cortex-a53\n").await.unwrap();
        tokio::fs::write(&apk_repo_path, initial_apk_repo).await.unwrap();
        tokio::fs::write(&opkg_repo_path, initial_opkg_repo).await.unwrap();

        let initial_config_bytes = tokio::fs::read(&config_path).await.unwrap();
        let initial_apk_repo_bytes = tokio::fs::read(&apk_repo_path).await.unwrap();
        let initial_opkg_repo_bytes = tokio::fs::read(&opkg_repo_path).await.unwrap();

        SetFixture {
            _dir: dir,
            config_path,
            apk_arch_path,
            apk_repo_path,
            opkg_repo_path,
            service_init,
            initial_config_bytes,
            initial_apk_repo_bytes,
            initial_opkg_repo_bytes,
        }
    }

    #[tokio::test]
    async fn set_round_trip_restores_active_from_sidecar() {
        let fx = build_fixture_with_prod_portal().await;
        let license_key_path = fx._dir.path().join("license.key");
        let license_dir = fx._dir.path().to_path_buf();
        tokio::fs::write(&license_key_path, b"existing-prod-license").await.unwrap();

        // First swap to staging → claim runs (initial config has production portal).
        let code = set_with_hooks_and_paths(
            "staging",
            &fx.config_path,
            &fx.apk_arch_path,
            &fx.apk_repo_path,
            &fx.opkg_repo_path,
            &license_key_path,
            &license_dir,
            &fx.service_init,
            &["https://127.0.0.1:0/health".to_string()],
            Duration::from_secs(1),
            |_init: PathBuf| async { Ok(()) },
            |_urls: Vec<String>, _budget| async { Ok(()) },
        ).await;
        assert_eq!(code, 0);

        // Verify the claim ran: license.production.key exists with the original bytes.
        let prod_sidecar = license_dir.join("license.production.key");
        let prod_bytes = tokio::fs::read(&prod_sidecar).await.unwrap();
        assert_eq!(prod_bytes, b"existing-prod-license");

        // Second swap back to production → license.key restored from license.production.key.
        let code2 = set_with_hooks_and_paths(
            "production",
            &fx.config_path,
            &fx.apk_arch_path,
            &fx.apk_repo_path,
            &fx.opkg_repo_path,
            &license_key_path,
            &license_dir,
            &fx.service_init,
            &["https://127.0.0.1:0/health".to_string()],
            Duration::from_secs(1),
            |_init: PathBuf| async { Ok(()) },
            |_urls: Vec<String>, _budget| async { Ok(()) },
        ).await;
        assert_eq!(code2, 0);
        let restored = tokio::fs::read(&license_key_path).await.unwrap();
        assert_eq!(restored, b"existing-prod-license",
            "round-trip must restore the original prod license byte-perfect");
    }

    // --- wait_for_health native probe tests (env-set wolfssl fix) ---
    //
    // Hardware-verified bug (2026-06-10, ZBT-WG3526 / OpenWrt 22.03.7):
    // uclient-fetch + libustream-wolfssl cannot complete a TLS handshake
    // against the daemon's rustls listener, so the old shell-out probe always
    // failed (rc=4) and `env set` rolled back a perfectly healthy daemon.
    // wait_for_health now probes natively from Rust (same rustls stack the
    // daemon itself uses) and must not shell out to any external fetcher.

    /// Minimal HTTP/1.1 responder: accepts connections in a loop, consumes the
    /// request, writes `response` verbatim, and closes. Returns the bound addr.
    async fn spawn_responder(response: &'static [u8]) -> std::net::SocketAddr {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else { break };
                tokio::spawn(async move {
                    let mut buf = [0u8; 1024];
                    let _ = sock.read(&mut buf).await;
                    let _ = sock.write_all(response).await;
                    let _ = sock.shutdown().await;
                });
            }
        });
        addr
    }

    /// Self-signed ECDSA P-256 cert for 127.0.0.1/localhost, valid to 2126.
    /// The probe client does NOT verify trust (parity with the old
    /// `--no-check-certificate`), so expiry/SAN are irrelevant; the handshake
    /// itself is what's under test.
    const TEST_CERT_PEM: &str = "-----BEGIN CERTIFICATE-----\n\
MIIBmzCCAUGgAwIBAgIULnkPG3DWA1d8uzzvX75jLeliX3kwCgYIKoZIzj0EAwIw\n\
FDESMBAGA1UEAwwJbG9jYWxob3N0MCAXDTI2MDYxMTE2MTYyMFoYDzIxMjYwNTE4\n\
MTYxNjIwWjAUMRIwEAYDVQQDDAlsb2NhbGhvc3QwWTATBgcqhkjOPQIBBggqhkjO\n\
PQMBBwNCAATbbGnTyOSJOWLj+qbMqj+cQolwevcPLV2+yjxZgOeErFttGsWY5Jiu\n\
6Rp+/B4nMusHGrgSCL07Jiazk5sZRffGo28wbTAdBgNVHQ4EFgQU0A17axmX7fcW\n\
aihcxXtEgMbr9a8wHwYDVR0jBBgwFoAU0A17axmX7fcWaihcxXtEgMbr9a8wDwYD\n\
VR0TAQH/BAUwAwEB/zAaBgNVHREEEzARhwR/AAABgglsb2NhbGhvc3QwCgYIKoZI\n\
zj0EAwIDSAAwRQIhAK1zKSJ+82ejSSRfRgzdNlw8yTY39DpcaqWybqkDM0zGAiBs\n\
j0NtvDN1BzulDGfq6kX5/wq89F9xySNG3vCLuS44/g==\n\
-----END CERTIFICATE-----\n";

    const TEST_KEY_PEM: &str = "-----BEGIN PRIVATE KEY-----\n\
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgVRItf94xlxL/wYG0\n\
ES8RNuI1RRQIlhho0Yw3KQzTg3OhRANCAATbbGnTyOSJOWLj+qbMqj+cQolwevcP\n\
LV2+yjxZgOeErFttGsWY5Jiu6Rp+/B4nMusHGrgSCL07Jiazk5sZRffG\n\
-----END PRIVATE KEY-----\n";

    const HEALTH_OK_RESPONSE: &[u8] =
        b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok";

    /// HTTPS responder serving a 200 "ok" /health behind a rustls listener —
    /// the same TLS stack shape as the real daemon (axum_server::bind_rustls).
    async fn spawn_tls_responder() -> std::net::SocketAddr {
        use rustls_pki_types::pem::PemObject;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let cert = rustls_pki_types::CertificateDer::from_pem_slice(TEST_CERT_PEM.as_bytes())
            .expect("test cert PEM parses");
        let key = rustls_pki_types::PrivateKeyDer::from_pem_slice(TEST_KEY_PEM.as_bytes())
            .expect("test key PEM parses");
        let config = rustls::ServerConfig::builder_with_provider(std::sync::Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(std::sync::Arc::new(config));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((sock, _)) = listener.accept().await else { break };
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    let Ok(mut tls) = acceptor.accept(sock).await else { return };
                    let mut buf = [0u8; 1024];
                    let _ = tls.read(&mut buf).await;
                    let _ = tls.write_all(HEALTH_OK_RESPONSE).await;
                    let _ = tls.shutdown().await;
                });
            }
        });
        addr
    }

    #[tokio::test]
    async fn wait_for_health_succeeds_against_plain_http_listener() {
        // No-TLS daemon shape: the app router (incl. /health) serves plain
        // HTTP on 8080. The probe must succeed without any external fetcher.
        let addr = spawn_responder(HEALTH_OK_RESPONSE).await;
        let urls = vec![format!("http://127.0.0.1:{}/health", addr.port())];
        let result = wait_for_health(&urls, Duration::from_secs(5)).await;
        assert!(result.is_ok(), "expected healthy, got {result:?}");
    }

    #[tokio::test]
    async fn wait_for_health_succeeds_against_rustls_https_listener() {
        // THE wolfssl regression test: a self-signed rustls HTTPS listener
        // (exactly what the daemon runs) must be probeable natively, with no
        // dependency on the platform's uclient-fetch/ustream TLS stack.
        let addr = spawn_tls_responder().await;
        let urls = vec![format!("https://127.0.0.1:{}/health", addr.port())];
        let result = wait_for_health(&urls, Duration::from_secs(5)).await;
        assert!(result.is_ok(), "expected healthy over rustls TLS, got {result:?}");
    }

    #[tokio::test]
    async fn wait_for_health_does_not_accept_redirect_as_healthy() {
        // TLS-active daemon: port 8080 runs the 307-redirect fallback. A 307
        // from the redirect server proves nothing about the HTTPS app — it
        // must NOT count as healthy (would weaken the gate to vacuity).
        let addr = spawn_responder(
            b"HTTP/1.1 307 Temporary Redirect\r\nLocation: https://127.0.0.1:8443/health\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        )
        .await;
        let urls = vec![format!("http://127.0.0.1:{}/health", addr.port())];
        let result = wait_for_health(&urls, Duration::from_millis(1500)).await;
        let err = result.expect_err("307 redirect must not count as healthy");
        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
    }

    #[tokio::test]
    async fn wait_for_health_does_not_accept_server_error_as_healthy() {
        let addr = spawn_responder(
            b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        )
        .await;
        let urls = vec![format!("http://127.0.0.1:{}/health", addr.port())];
        let result = wait_for_health(&urls, Duration::from_millis(1500)).await;
        let err = result.expect_err("500 must not count as healthy");
        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
    }

    #[tokio::test]
    async fn wait_for_health_times_out_when_daemon_never_responds() {
        // Down-daemon shape, race-free: hold a bound listener that never
        // accepts for the whole test, so no parallel test's 127.0.0.1:0 bind
        // can reuse the port and serve a 200 (the old bind-then-drop version
        // was racy). The OS backlog completes the TCP connect but no HTTP
        // response ever arrives, so this also exercises the per-attempt
        // timeout path; the budget must still exhaust and report TimedOut
        // (rollback trigger).
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let urls = vec![
            format!("https://127.0.0.1:{port}/health"),
            format!("http://127.0.0.1:{port}/health"),
        ];
        let result = wait_for_health(&urls, Duration::from_millis(1200)).await;
        let err = result.expect_err("down daemon must fail health");
        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
        drop(listener); // keep the port occupied until the probe has finished
    }

    #[tokio::test]
    async fn wait_for_health_timeout_reports_last_probe_error() {
        // Budget exhaustion must say WHY the last attempt failed — the
        // wolfssl bug needed hardware repro precisely because the old probe's
        // failure mode was opaque. One log line should now suffice.
        let addr = spawn_responder(
            b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        )
        .await;
        let url = format!("http://127.0.0.1:{}/health", addr.port());
        let result = wait_for_health(std::slice::from_ref(&url), Duration::from_millis(1200)).await;
        let err = result.expect_err("500 must not count as healthy");
        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
        let msg = err.to_string();
        assert!(msg.contains(&format!("last: {url}")), "timeout message must name the failing URL, got: {msg}");
        assert!(msg.contains("HTTP 500"), "timeout message must carry the last probe error, got: {msg}");
    }

    #[tokio::test]
    async fn wait_for_health_rejects_non_http_response() {
        // A non-HTTP service whose banner has a 2xx-looking second token
        // ("ICY 200 OK") must NOT count as healthy — the status-line parse
        // must anchor on "HTTP/" and route to the malformed-line error path.
        let addr = spawn_responder(b"ICY 200 OK\r\n\r\n").await;
        let urls = vec![format!("http://127.0.0.1:{}/health", addr.port())];
        let result = wait_for_health(&urls, Duration::from_millis(1200)).await;
        let err = result.expect_err("non-HTTP response must not count as healthy");
        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
        assert!(
            err.to_string().contains("malformed HTTP status line"),
            "expected malformed-status-line detail, got: {err}"
        );
    }

    #[tokio::test]
    async fn wait_for_health_falls_back_to_second_url_when_first_unreachable() {
        // TLS-off daemon with the real default URL list shape: HTTPS:8443 is
        // dead (nothing listens), plain HTTP serves /health. The probe must
        // fall through to the HTTP URL and succeed. (The old code probed ONLY
        // https://127.0.0.1:8443/health, so no-TLS daemons could never pass.)
        let dead = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let dead_port = dead.local_addr().unwrap().port();
        drop(dead);
        let live = spawn_responder(HEALTH_OK_RESPONSE).await;
        let urls = vec![
            format!("https://127.0.0.1:{dead_port}/health"),
            format!("http://127.0.0.1:{}/health", live.port()),
        ];
        let result = wait_for_health(&urls, Duration::from_secs(5)).await;
        assert!(result.is_ok(), "expected fallback URL to report healthy, got {result:?}");
    }

    // --- parse_health_url ---

    #[test]
    fn parse_health_url_parses_https_host_port_path() {
        let t = parse_health_url("https://127.0.0.1:8443/health").unwrap();
        assert!(t.tls);
        assert_eq!(t.host, "127.0.0.1");
        assert_eq!(t.port, 8443);
        assert_eq!(t.path, "/health");
    }

    #[test]
    fn parse_health_url_defaults_scheme_ports_and_path() {
        let t = parse_health_url("http://localhost").unwrap();
        assert!(!t.tls);
        assert_eq!(t.host, "localhost");
        assert_eq!(t.port, 80);
        assert_eq!(t.path, "/");

        let t = parse_health_url("https://localhost/x").unwrap();
        assert!(t.tls);
        assert_eq!(t.port, 443);
    }

    #[test]
    fn parse_health_url_rejects_unsupported_forms() {
        assert!(parse_health_url("ftp://127.0.0.1/health").is_none());
        assert!(parse_health_url("127.0.0.1:8443/health").is_none());
        assert!(parse_health_url("https:///health").is_none());
        assert!(parse_health_url("https://127.0.0.1:notaport/health").is_none());
    }

    #[tokio::test]
    async fn set_rolls_back_license_key_when_health_fails() {
        let fx = build_fixture_with_prod_portal().await;
        let license_key_path = fx._dir.path().join("license.key");
        let license_dir = fx._dir.path().to_path_buf();
        tokio::fs::write(&license_key_path, b"existing-prod-license").await.unwrap();
        // Pre-stage a staging sidecar so the slot swap actually changes license.key.
        let staging_sidecar = license_dir.join("license.staging.key");
        tokio::fs::write(&staging_sidecar, b"staging-license").await.unwrap();
        let initial_active_bytes = tokio::fs::read(&license_key_path).await.unwrap();

        let code = set_with_hooks_and_paths(
            "staging",
            &fx.config_path,
            &fx.apk_arch_path,
            &fx.apk_repo_path,
            &fx.opkg_repo_path,
            &license_key_path,
            &license_dir,
            &fx.service_init,
            &["https://127.0.0.1:0/health".to_string()],
            Duration::from_secs(1),
            |_init: PathBuf| async { Ok(()) },
            |_urls: Vec<String>, _budget| async { Err(std::io::Error::other("simulated health error")) },
        ).await;

        assert_eq!(code, 1);
        let active_after = tokio::fs::read(&license_key_path).await.unwrap();
        assert_eq!(active_after, initial_active_bytes,
            "license.key must be rolled back to its prior value on health-fail");
        // Sidecars are NOT rolled back (append-only). license.production.key
        // newly created by the claim step IS left behind.
        let prod_sidecar = license_dir.join("license.production.key");
        assert!(tokio::fs::metadata(&prod_sidecar).await.is_ok(),
            "claim-created sidecar persists past rollback");
    }
}
