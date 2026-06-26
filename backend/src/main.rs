//! OpenWRT Modem Interface - Main entry point.
//!
//! A web interface for managing cellular modems on OpenWRT routers.
//!
//! ## Usage
//!
//! ```bash
//! # Development with mock hardware
//! MOCK_HARDWARE=1 cargo run
//!
//! # Production (requires real modem)
//! modem-interface --config /etc/modem-interface/config.toml
//! ```

use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tracing::{info, Level};
#[cfg(feature = "tls")]
use tracing::warn;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

mod api;
mod cli;
mod config;
mod hardware;
mod security;
mod state;
#[cfg(feature = "tls")]
mod tls_cert;

use hardware::AppConfig;
#[cfg(feature = "mock-hardware")]
use hardware::ModemHardware as _;
use hardware::profiles::ProfileRegistry;
use security::users::{Role, UiProfile, User, UserStore};
use state::AppState;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Install rustls crypto provider before any TLS operation.
    // ring is the only compiled provider — aws-lc-sys is evicted from the build
    // graph (no MIPS support; guarded by scripts/check-no-aws-lc.sh). Keep this
    // explicit selection so the process-wide default never depends on
    // crate-feature fallback.
    let _ = rustls::crypto::ring::default_provider().install_default();

    if let Some(code) = cli::dispatch().await {
        std::process::exit(code as i32);
    }

    // Initialize logging with console + file output
    let log_dir = std::env::var("LOG_DIR").unwrap_or_else(|_| "/tmp/modem-interface".to_string());
    let file_appender = tracing_appender::rolling::daily(&log_dir, "modem-interface.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);
    // _guard MUST be held until program exit to flush buffered logs

    tracing_subscriber::registry()
        .with(fmt::layer())                                                     // Console → procd → syslog
        .with(fmt::layer().with_ansi(false).with_writer(non_blocking))          // File (no ANSI codes)
        .with(
            EnvFilter::builder()
                .with_default_directive(Level::INFO.into())
                .from_env_lossy(),
        )
        .init();

    // Handle CLI commands (no server startup needed)
    let args: Vec<String> = std::env::args().collect();
    if args.len() >= 4 && args[1] == "--reset-password" {
        return cli_reset_password(&args[2], &args[3]).await;
    }

    info!("Service starting");

    // Load configuration
    let mut config = config::load_config().await;
    #[cfg(feature = "tls")]
    let tls_config = config.tls.clone();
    info!("Configuration loaded");

    // Load user store and run migration if needed
    let users = UserStore::load(&config.auth.users_file).await;
    migrate_single_user_to_multi(&mut config, &users).await;

    // Load AT whitelist overrides
    let whitelist_overrides = security::load_overrides().await;

    // Load saved APN profiles
    let apn_profiles = config::apn_profiles::load_apn_profiles().await;

    // Load SIM slot config (per-slot APN profile assignments)
    let sim_slot_config = config::sim_slots::load_sim_slot_config().await;

    // Load WAN manager config (multi-modem WAN priority)
    let wan_config = config::wan::load_wan_config().await;

    // Compute device token and check license
    let device_token = hardware::fingerprint::generate_device_token();
    // Do NOT log the raw device token — it is a portal credential. Log only a
    // short non-reversible SHA-256 prefix for support correlation.
    {
        use sha2::{Digest, Sha256};
        let digest = Sha256::digest(device_token.as_bytes());
        let prefix: String = digest
            .iter()
            .take(4)
            .map(|b| format!("{b:02x}"))
            .collect();
        info!("Device token loaded (sha256:{}…)", prefix);
    }
    let license_state = security::license::check_license(
        &device_token,
        config::current_env(&config.portal.base_url),
    ).await;
    match &license_state {
        security::license::LicenseState::Valid { tier, expires_at, .. } => {
            info!("License: valid (tier={}, expires={})", tier, expires_at);
        }
        security::license::LicenseState::Expired { .. } => {
            info!("License: expired");
        }
        security::license::LicenseState::InvalidSignature => {
            info!("License: invalid signature");
        }
        security::license::LicenseState::DeviceMismatch => {
            info!("License: device mismatch");
        }
        security::license::LicenseState::Unlicensed => {
            info!("License: not activated");
        }
    }

    // Per-device Ed25519 signing keypair (Item #3 — signed device auth).
    // Generate-once / persist-forever (0600), like the device-token / license.key.
    let device_auth = Arc::new(
        security::device_auth::DeviceAuth::load_or_create(std::path::Path::new(
            security::device_auth::DEVICE_AUTH_KEY_PATH,
        ))
        .expect("failed to load or create device-auth keypair"),
    );
    tracing::info!(key_id = %device_auth.key_id, "device auth keypair ready");

    // Initialize hardware (mock or real based on environment)
    let state = create_app_state(config, users, device_token, device_auth, license_state).await?;

    // Inject loaded whitelist overrides into state
    {
        let mut wl = state.at_whitelist_overrides.write().await;
        *wl = whitelist_overrides;
    }

    // Inject loaded APN profiles into state
    {
        let mut profiles = state.apn_profiles.write().await;
        *profiles = apn_profiles;
    }

    // Inject loaded per-modem SIM slot config into state
    {
        let mut sc = state.sim_slot_config.write().await;
        *sc = sim_slot_config;
    }

    // Inject loaded WAN config into state
    {
        let mut wc = state.wan_config.write().await;
        *wc = wan_config;
    }

    // Load speedtest history from disk
    {
        let history = hardware::speedtest::load_history();
        let count = history.results.len();
        let mut sh = state.speedtest_history.write().await;
        *sh = history;
        if count > 0 {
            info!("Loaded {} speedtest history entries", count);
        }
    }

    // Detect platform capabilities for policy-based routing
    let platform = crate::api::routing::detect_platform();
    {
        let mut caps = state.platform_capabilities.write().await;
        *caps = platform.clone();
    }

    // Initialize policy routing tables if available
    if platform.policy_routing_enabled {
        let wan_config = state.wan_config.read().await;
        let wan_entries: Vec<(String, String, u32)> = wan_config
            .modem_priority
            .iter()
            .enumerate()
            .map(|(i, entry)| (entry.modem_id.clone(), entry.network_device.clone(), i as u32))
            .collect();
        let routing_mode = wan_config.routing_mode.clone();
        let weights: std::collections::HashMap<String, u32> = wan_config
            .modem_priority
            .iter()
            .map(|e| (e.modem_id.clone(), e.weight.unwrap_or(1)))
            .collect();
        let primary_id = wan_config.modem_priority.first().map(|e| e.modem_id.clone());
        drop(wan_config);

        let routing_tables = crate::api::routing::initialize_tables(&wan_entries, &routing_mode, &weights, primary_id.as_deref());
        {
            let mut rs = state.routing_state.write().await;
            *rs = routing_tables;
        }
        info!("Policy-based routing initialized");
    } else {
        info!("Policy routing not available — using metric-based fallback");
    }

    // Initialize traffic steering rules (Level 2)
    {
        let caps = state.platform_capabilities.read().await;
        let fw_backend = caps.firewall_backend.clone();
        let rs = state.routing_state.read().await;
        let steering = crate::api::steering::initialize(
            crate::api::steering::STEERING_CONFIG_PATH,
            &fw_backend,
            &rs,
        );
        drop(rs);
        drop(caps);
        let mut sr = state.steering_rules.write().await;
        *sr = steering;
        info!("Traffic steering initialized ({} rules)", sr.len());
    }

    // Start background cache refresh task (60s master cache for all modems)
    api::websocket::spawn_cache_refresh_task(Arc::clone(&state));
    info!("Cache refresh task started");

    // Start modem reconnect watcher (auto-recovers after reboot/disconnect)
    api::websocket::spawn_reconnect_watcher(Arc::clone(&state));
    info!("Reconnect watcher started");

    // Start WAN connectivity watchdog (multi-modem failover)
    api::websocket::spawn_wan_watchdog(Arc::clone(&state));
    info!("WAN watchdog started");

    // Start portal heartbeat task (30-minute interval, licensed devices only)
    api::heartbeat::spawn_heartbeat_task(Arc::clone(&state));
    info!("Portal heartbeat task started");

    // Start telemetry collector task (5-minute snapshots into ring buffer)
    api::telemetry::spawn_telemetry_collector(Arc::clone(&state), Arc::clone(&state.telemetry_buffer));
    info!("Telemetry collector task started");

    // Start remote config poll task (checks portal for poll-now / fast-mode commands)
    api::telemetry::spawn_remote_config_poll(Arc::clone(&state));
    info!("Remote config poll task started");

    // Start remote access tunnel client (persistent WSS to portal)
    api::tunnel::spawn_tunnel_task(Arc::clone(&state));
    info!("Remote access tunnel task started");

    // Auto-discover unknown modems in the background
    {
        let state_clone = Arc::clone(&state);
        tokio::spawn(async move {
            // Wait for modems to stabilize after detection
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;

            // Collect modem IDs with generic profile
            type ModemDiscoveryTuple = (String, String, String, Arc<tokio::sync::Mutex<Box<dyn hardware::ModemHardware + Send>>>);
            let modems_to_discover: Vec<ModemDiscoveryTuple> = {
                let modems = state_clone.modems.read().await;
                modems.iter()
                    .filter(|(_, context)| context.profile.identity.vendor_id == "0000")
                    .filter_map(|(modem_id, context)| {
                        let vid = context.detected.vendor_id.as_ref()?.clone();
                        let pid = context.detected.product_id.as_ref()?.clone();
                        Some((modem_id.clone(), vid, pid, Arc::clone(&context.handler)))
                    })
                    .collect()
            };

            // Iterate over modems that need discovery
            for (modem_id, vid, pid, handler_arc) in modems_to_discover {
                info!("Auto-discovering unknown modem {}", modem_id);

                if let Ok(handler) = tokio::time::timeout(
                    std::time::Duration::from_secs(2),
                    handler_arc.lock()
                ).await {
                    let mut results = std::collections::HashMap::new();
                    for cmd in ["ATI", "AT+GSN", "AT+CGMR", "AT+CGMI", "AT+CGMM",
                                "AT+CPIN?", "AT+COPS?", "AT+CSQ", "AT+CEREG?", "AT+CGDCONT?"] {
                        if let Ok(resp) = handler.execute_at(cmd).await {
                            results.insert(cmd.to_string(), resp);
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    }
                    drop(handler);

                    // Save discovery results
                    let filename = format!("/tmp/modem-discovery-{vid}-{pid}.json");
                    if let Ok(json) = serde_json::to_string_pretty(&results) {
                        let _ = std::fs::write(&filename, &json);
                        info!("Auto-discovery saved to {}", filename);
                    }
                } else {
                    tracing::warn!("Failed to acquire modem handler lock for discovery: {}", modem_id);
                }
            }
        });
        info!("Auto-discovery background task spawned");
    }

    // Spawn session + WS token cleanup task (purge expired every 5 minutes)
    {
        let sessions = Arc::clone(&state.sessions);
        let ws_tokens = Arc::clone(&state.ws_tokens);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(300));
            loop {
                interval.tick().await;
                sessions.purge_expired().await;
                ws_tokens.purge_expired().await;
            }
        });
    }

    // Spawn rate limiter cleanup task (purge stale buckets every 10 minutes)
    {
        let rate_limiter = Arc::clone(&state.rate_limiter);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(600));
            loop {
                interval.tick().await;
                rate_limiter.cleanup().await;
            }
        });
    }

    // Keep references before router consumes state
    #[cfg(feature = "tls")]
    let state_for_tls = Arc::clone(&state);
    let state_for_shutdown = Arc::clone(&state);

    // Build router
    let app = axum::Router::new()
        .route("/health", axum::routing::get(api::health))
        .merge(api::router(state));

    // Bind HTTP address
    let addr: SocketAddr = std::env::var("LISTEN_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:8080".to_string())
        .parse()?;

    // Attempt TLS if feature enabled and configured
    #[cfg(feature = "tls")]
    {
        if tls_config.enabled {
            match try_start_tls(app.clone(), addr, &tls_config, &state_for_tls.tls_active).await {
                Ok(()) => {
                    return Ok(());
                }
                Err(e) => {
                    warn!("TLS failed to start, falling back to HTTP-only: {}", e);
                }
            }
        }
    }

    // Plain HTTP (fallback or TLS not enabled/compiled)
    // Serving cleartext: if auth is enabled, the session cookie's Secure flag
    // never applies, so warn the operator once at startup. Read auth.enabled the
    // same way the rest of the codebase does (state.config.read().await).
    {
        let auth_enabled = state_for_shutdown.config.read().await.auth.enabled;
        crate::api::warn_if_auth_without_tls(auth_enabled, false);
    }

    let listener = TcpListener::bind(addr).await?;
    info!("Listening on http://{}", addr);

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
        .with_graceful_shutdown(async move {
            tokio::signal::ctrl_c().await.ok();
            info!("Shutdown signal received");
            let caps = state_for_shutdown.platform_capabilities.read().await;
            if caps.policy_routing_enabled {
                crate::api::routing::flush_all_tables();
                info!("Policy routing tables cleaned up");
            }
            crate::api::steering::flush_steering(&caps.firewall_backend);
            info!("Steering rules cleaned up");
        })
        .await?;

    Ok(())
}

/// Start HTTPS server with HTTP redirect.
#[cfg(feature = "tls")]
async fn try_start_tls(
    app: axum::Router,
    http_addr: SocketAddr,
    tls_config: &hardware::TlsConfig,
    tls_active: &std::sync::atomic::AtomicBool,
) -> anyhow::Result<()> {
    use axum_server::tls_rustls::RustlsConfig;
    use std::path::Path;

    let cert_path = Path::new(&tls_config.cert_path);
    let key_path = Path::new(&tls_config.key_path);

    // If the cert/key are missing, self-generate them in-binary (replaces the
    // old openssl-util first-boot dependency). Idempotent: a no-op when both
    // files already exist, so existing installs keep their current cert.
    tls_cert::ensure_self_signed_cert(cert_path, key_path)?;

    let rustls_config = RustlsConfig::from_pem_file(&tls_config.cert_path, &tls_config.key_path)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to load TLS config: {e}"))?;

    let https_addr = SocketAddr::from(([0, 0, 0, 0], tls_config.https_port));

    // Mark TLS as active now that certs loaded successfully
    tls_active.store(true, std::sync::atomic::Ordering::Relaxed);
    info!(
        "TLS enabled: HTTPS on {}, HTTP on {}",
        https_addr, http_addr
    );

    // Spawn HTTP redirect server (only if TLS is actually active)
    if tls_config.redirect_http && tls_active.load(std::sync::atomic::Ordering::Relaxed) {
        let https_port = tls_config.https_port;
        tokio::spawn(async move {
            if let Err(e) = run_http_redirect(http_addr, https_port).await {
                warn!("HTTP redirect server error: {}", e);
            }
        });
    }

    // Run HTTPS server (blocking)
    axum_server::bind_rustls(https_addr, rustls_config)
        .serve(app.into_make_service_with_connect_info::<SocketAddr>())
        .await?;

    Ok(())
}

/// HTTP server that redirects all requests to HTTPS.
#[cfg(feature = "tls")]
async fn run_http_redirect(bind_addr: SocketAddr, https_port: u16) -> anyhow::Result<()> {
    use axum::{
        extract::Host,
        http::{uri::Authority, StatusCode, Uri},
        response::Redirect,
    };

    let redirect_app = axum::Router::new().fallback(
        move |Host(host): Host, uri: Uri| async move {
            // Strip port from host if present, replace with HTTPS port
            let host_without_port = host
                .rsplit_once(':')
                .map(|(h, _)| h)
                .unwrap_or(&host);

            let authority = format!("{host_without_port}:{https_port}");
            let authority: Authority = match authority.parse() {
                Ok(a) => a,
                Err(_) => return Err(StatusCode::BAD_REQUEST),
            };

            let mut parts = uri.into_parts();
            parts.scheme = Some(axum::http::uri::Scheme::HTTPS);
            parts.authority = Some(authority);
            if parts.path_and_query.is_none() {
                parts.path_and_query = Some("/".parse().unwrap());
            }

            let uri = Uri::from_parts(parts).map_err(|_| StatusCode::BAD_REQUEST)?;
            Ok(Redirect::temporary(&uri.to_string()))
        },
    );

    let listener = TcpListener::bind(bind_addr).await?;
    info!("HTTP redirect server listening on http://{}", bind_addr);
    axum::serve(listener, redirect_app).await?;

    Ok(())
}

/// Create AppState with appropriate hardware implementation.
async fn create_app_state(
    config: AppConfig,
    users: UserStore,
    device_token: String,
    device_auth: Arc<security::device_auth::DeviceAuth>,
    license_state: security::license::LicenseState,
) -> anyhow::Result<Arc<AppState>> {
    // Log which features are enabled
    #[cfg(feature = "mock-hardware")]
    info!("Mode: simulation");
    #[cfg(feature = "real-hardware")]
    info!("Mode: hardware");

    // Initialize profile registry
    let registry = ProfileRegistry::load();

    // Try to detect hardware (before consuming registry)
    #[cfg(feature = "mock-hardware")]
    let use_mock = std::env::var("MOCK_HARDWARE").is_ok();
    #[cfg(not(feature = "mock-hardware"))]
    let use_mock = false;

    let detected = if use_mock {
        Vec::new()
    } else {
        info!("Attempting real hardware detection...");
        let detected = hardware::detect_modems(&registry, hardware::DetectionVerbosity::Verbose);
        info!("Detection returned {} device(s)", detected.len());
        detected
    };

    // Create empty AppState (consumes registry)
    let state = Arc::new(AppState::new(config.clone(), users, registry, device_token, device_auth, license_state));

    #[cfg(feature = "mock-hardware")]
    {
        if use_mock {
            info!("Using mock hardware implementation (MOCK_HARDWARE env set)");
            let modem = Box::new(hardware::MockHardware::new());
            let mock_detected = hardware::DetectedModem {
                device_path: "/dev/ttyUSB2".to_string(),
                protocol: hardware::ModemProtocol::At,
                description: "Mock Modem".to_string(),
                vendor_id: Some("2c7c".to_string()),
                product_id: Some("0800".to_string()),
                bus_port: Some("mock-0".to_string()),
                profile_id: Some("quectel_rm551e_gl".to_string()),
                has_profile: true,
                all_ports: vec!["/dev/ttyUSB2".to_string()],
            };
            let profile = state.profile_registry.find_by_id("quectel_rm551e_gl")
                .unwrap_or_else(|| state.profile_registry.generic()).clone();
            let discovery = modem.get_discovery_info().await.unwrap_or_else(|e| {
                tracing::warn!("Mock discovery failed: {e}, using defaults");
                hardware::DiscoveryInfo::default()
            });
            let modem_id = hardware::generate_modem_id(&mock_detected)?;
            state.add_modem(modem_id, modem, profile, mock_detected, config.connection.clone(), discovery).await;
            return Ok(state);
        }
    }
    // Add all detected modems to state
    for modem_info in &detected {
        info!(
            "Detected modem: {} at {} (profile: {})",
            modem_info.description,
            modem_info.device_path,
            modem_info.profile_id.as_deref().unwrap_or("none")
        );

        // Look up the profile for this modem
        let profile = match (&modem_info.vendor_id, &modem_info.product_id) {
            (Some(vid), Some(pid)) => state.profile_registry.match_profile(vid, pid).clone(),
            _ => state.profile_registry.generic().clone(),
        };

        // Wrap handler creation in spawn_blocking to prevent blocking the main thread
        // Serial port open can hang indefinitely on unresponsive/wrong ports
        let modem_info_clone = modem_info.clone();
        let profile_clone = profile.clone();
        let handler_result = tokio::task::spawn_blocking(move || {
            hardware::create_modem_handler(&modem_info_clone, profile_clone)
        });

        let handler_result = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            handler_result
        ).await;

        match handler_result {
            Ok(Ok(Ok(handler))) => {
                info!("Successfully initialized modem handler with profile: {}", profile.profile_id());
                let discovery = handler.get_discovery_info().await.unwrap_or_else(|e| {
                    tracing::warn!("Discovery failed for {}: {e}, using defaults", modem_info.description);
                    hardware::DiscoveryInfo::default()
                });
                let modem_id = hardware::generate_modem_id(modem_info)?;
                state.add_modem(
                    modem_id.clone(),
                    handler,
                    profile,
                    modem_info.clone(),
                    config.connection.clone(),
                    discovery,
                ).await;

                // Boot-time USB-net mode detection (diagnostic only; never blocks bring-up).
                // Per spec §3.10 detect_usbnet_mode never returns Err; failure cached as Unknown.
                state.detect_and_cache_usbnet_mode(&modem_id).await;
            }
            Ok(Ok(Err(e))) => {
                tracing::warn!("Failed to initialize modem {}: {}", modem_info.description, e);
            }
            Ok(Err(e)) => {
                tracing::warn!("Handler creation task panicked for modem {}: {}", modem_info.description, e);
            }
            Err(_) => {
                tracing::warn!("Handler creation timed out (30s) for modem {} - port may be unresponsive", modem_info.description);
            }
        }
    }

    // Store detected modems list
    *state.detected_modems.write().await = detected;

    // Fall back to mock if no modems were added
    #[cfg(feature = "mock-hardware")]
    {
        if state.modems.read().await.is_empty() {
            info!("No real hardware detected, using mock");
            let modem = Box::new(hardware::MockHardware::new());
            let mock_detected = hardware::DetectedModem {
                device_path: "/dev/ttyUSB2".to_string(),
                protocol: hardware::ModemProtocol::At,
                description: "Mock Modem (fallback)".to_string(),
                vendor_id: Some("2c7c".to_string()),
                product_id: Some("0800".to_string()),
                bus_port: Some("mock-0".to_string()),
                profile_id: Some("quectel_rm551e_gl".to_string()),
                has_profile: true,
                all_ports: vec!["/dev/ttyUSB2".to_string()],
            };
            let profile = state.profile_registry.find_by_id("quectel_rm551e_gl")
                .unwrap_or_else(|| state.profile_registry.generic()).clone();
            let discovery = modem.get_discovery_info().await.unwrap_or_else(|e| {
                tracing::warn!("Mock fallback discovery failed: {e}, using defaults");
                hardware::DiscoveryInfo::default()
            });
            let modem_id = hardware::generate_modem_id(&mock_detected)?;
            state.add_modem(modem_id.clone(), modem, profile, mock_detected, config.connection.clone(), discovery).await;

            // Boot-time USB-net mode detection (diagnostic only; never blocks bring-up).
            state.detect_and_cache_usbnet_mode(&modem_id).await;
        }
    }

    #[cfg(not(feature = "mock-hardware"))]
    {
        if state.modems.read().await.is_empty() {
            anyhow::bail!(
                "No modem detected and mock-hardware feature not enabled. \
                 Set MOCK_HARDWARE=1 or connect a modem."
            )
        }
    }

    Ok(state)
}

/// Migrate v0.3.0 single-user password to multi-user system.
///
/// If config.toml has a password_hash but users.json has no users,
/// create an "admin" user with the migrated hash.
async fn migrate_single_user_to_multi(config: &mut AppConfig, users: &UserStore) {
    let legacy_hash = match &config.auth.password_hash {
        Some(hash) if !users.has_users().await => hash.clone(),
        _ => return,
    };

    info!("Migrating v0.3.0 single-user password to multi-user system");

    let admin = User {
        username: "admin".to_string(),
        role: Role::Admin,
        password_hash: Some(legacy_hash),
        allowed_panels: None,
        allowed_features: None,
        ui_profile: UiProfile::default(),
        disabled: false,
    };

    users.create_user_unchecked(admin).await;
    if let Err(e) = users.save().await {
        tracing::warn!("Failed to save migrated users: {e}");
        return;
    }

    // Clear legacy hash from config
    config.auth.password_hash = None;
    if let Err(e) = config::save_config(config).await {
        tracing::warn!("Failed to update config after migration: {e}");
    }

    info!("Migration complete: created 'admin' user from legacy password");
}

/// CLI: Reset a user's password without starting the server.
async fn cli_reset_password(username: &str, new_password: &str) -> anyhow::Result<()> {
    let config = config::load_config().await;
    let users = UserStore::load(&config.auth.users_file).await;

    if username == "root" {
        anyhow::bail!("Cannot reset root password here. Use 'passwd' via SSH on the router.");
    }

    if users.get_user(username).await.is_none() {
        anyhow::bail!("User '{username}' not found");
    }

    if !security::users::password_meets_min_len(new_password) {
        anyhow::bail!("Password must be at least 12 characters");
    }

    use argon2::{password_hash::SaltString, Argon2, PasswordHasher};
    use rand::rngs::OsRng;

    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(new_password.as_bytes(), &salt)
        .map_err(|e| anyhow::anyhow!("Failed to hash password: {e}"))?
        .to_string();

    users
        .set_password_hash(username, hash)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    println!("Password reset successfully for user '{username}'");
    Ok(())
}
