//! Remote access tunnel client.
//!
//! Maintains a persistent outbound WebSocket connection to the CTRL-Cloud portal,
//! enabling remote access to the router's local web services. The portal proxies
//! authenticated user HTTP requests through this tunnel.
//!
//! Feature-gated: only starts if the license includes "remote_access".

use serde::{Deserialize, Serialize};

/// Tunnel frame types sent from portal to router.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
#[allow(dead_code)]
pub enum TunnelRequest {
    /// Proxy an HTTP request to a local service.
    #[serde(rename = "http_request")]
    HttpRequest {
        id: String,
        method: String,
        port: u16,
        path: String,
        headers: Vec<(String, String)>,
        body: Option<String>, // base64-encoded
    },
    /// Start of a chunked HTTP request (body > 256KB).
    #[serde(rename = "http_request_start")]
    HttpRequestStart {
        id: String,
        method: String,
        port: u16,
        path: String,
        headers: Vec<(String, String)>,
    },
    /// Chunk of a request body.
    #[serde(rename = "http_request_chunk")]
    HttpRequestChunk {
        id: String,
        body: String, // base64-encoded chunk
    },
    /// End of a chunked HTTP request.
    #[serde(rename = "http_request_end")]
    HttpRequestEnd {
        id: String,
    },
    /// Request to upgrade a connection to WebSocket.
    #[serde(rename = "ws_upgrade_request")]
    WsUpgradeRequest {
        id: String,
        stream_id: String,
        port: u16,
        path: String,
        headers: Vec<(String, String)>,
    },
    /// A WebSocket frame from the portal (user -> router service).
    #[serde(rename = "ws_frame")]
    WsFrame {
        stream_id: String,
        data: String, // base64-encoded
        is_binary: bool,
    },
    /// Close a proxied WebSocket.
    #[serde(rename = "ws_close")]
    WsClose {
        stream_id: String,
    },
    /// Server-issued challenge nonce, sent immediately on connect. The router
    /// signs the canonical tunnel bytes over this nonce in its TunnelAuth reply.
    /// Item #3 Phase 4.
    #[serde(rename = "tunnel_challenge")]
    TunnelChallenge {
        nonce: String,
    },
    /// Server-emitted rejection signal during auth/config phase.
    /// On receipt, the tunnel client logs a structured line, returns a
    /// rejection sentinel from `connect_and_run`, and `tunnel_client_loop`
    /// forces a 300s backoff for the next reconnect attempt.
    #[serde(rename = "tunnel_rejection")]
    TunnelRejection {
        reason: String,
        message: String,
    },
}

/// Tunnel frame types sent from router to portal.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
#[allow(dead_code)]
pub enum TunnelResponse {
    /// Authentication message (first frame after connect). Item #3 Phase 4 adds
    /// the optional `kid` + `signature` (base64url Ed25519 sig over
    /// `canonical_tunnel`). They are omitted on the wire for the legacy unsigned
    /// shape, which the portal still accepts for unenrolled device rows.
    #[serde(rename = "tunnel_auth")]
    TunnelAuth {
        device_token: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kid: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    /// Tunnel configuration (sent after successful auth).
    #[serde(rename = "tunnel_config")]
    TunnelConfig {
        ports: Vec<u16>,
    },
    /// Complete HTTP response (body fits in one frame).
    #[serde(rename = "http_response")]
    HttpResponse {
        id: String,
        status: u16,
        headers: Vec<(String, String)>,
        body: Option<String>, // base64-encoded
    },
    /// Start of a chunked HTTP response (body > 256KB).
    #[serde(rename = "http_response_start")]
    HttpResponseStart {
        id: String,
        status: u16,
        headers: Vec<(String, String)>,
    },
    /// Chunk of a response body.
    #[serde(rename = "http_response_chunk")]
    HttpResponseChunk {
        id: String,
        body: String, // base64-encoded chunk
    },
    /// End of a chunked HTTP response.
    #[serde(rename = "http_response_end")]
    HttpResponseEnd {
        id: String,
    },
    /// WebSocket upgrade response.
    #[serde(rename = "ws_upgrade_response")]
    WsUpgradeResponse {
        id: String,
        stream_id: String,
        accepted: bool,
    },
    /// A WebSocket frame from a local service (router service -> user).
    #[serde(rename = "ws_frame")]
    WsFrame {
        stream_id: String,
        data: String, // base64-encoded
        is_binary: bool,
    },
    /// Close a proxied WebSocket.
    #[serde(rename = "ws_close")]
    WsClose {
        stream_id: String,
    },
}

/// Chunk size for large bodies (256KB).
#[allow(dead_code)]
pub const CHUNK_SIZE: usize = 256 * 1024;

// ─── Tunnel client implementation ─────────────────────────────────────────────

use base64::Engine as _;
use futures_util::{SinkExt, StreamExt};
use futures_util::stream::SplitSink;
use std::collections::HashMap;
use std::sync::Arc;
// AtomicU64 comes from portable-atomic: mips32 targets (mipsel_24kc) have no
// native 64-bit atomics; on 64-bit targets this is a zero-cost re-export.
use portable_atomic::AtomicU64;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::task::JoinHandle;
use tokio::time::sleep;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};
#[cfg(feature = "tunnel")]
use tokio_tungstenite::{connect_async_tls_with_config, Connector};
use tokio_tungstenite::tungstenite::Message;
#[cfg(feature = "tunnel")]
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tracing::{debug, error, info, warn};

// ─── Self-signed cert verifier for local WS connections ─────────────────────
// Only used for localhost connections to services with self-signed TLS certs.
// Feature-gated: only needed when the tunnel WebSocket proxy is compiled in.

#[cfg(feature = "tunnel")]
mod no_verify_tls {
    use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
    use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
    use rustls::DigitallySignedStruct;

    #[derive(Debug)]
    pub struct NoVerifier;

    impl ServerCertVerifier for NoVerifier {
        fn verify_server_cert(
            &self,
            _end_entity: &CertificateDer<'_>,
            _intermediates: &[CertificateDer<'_>],
            _server_name: &ServerName<'_>,
            _ocsp_response: &[u8],
            _now: UnixTime,
        ) -> Result<ServerCertVerified, rustls::Error> {
            Ok(ServerCertVerified::assertion())
        }

        fn verify_tls12_signature(
            &self,
            _message: &[u8],
            _cert: &CertificateDer<'_>,
            _dss: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            Ok(HandshakeSignatureValid::assertion())
        }

        fn verify_tls13_signature(
            &self,
            _message: &[u8],
            _cert: &CertificateDer<'_>,
            _dss: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            Ok(HandshakeSignatureValid::assertion())
        }

        fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
            rustls::crypto::ring::default_provider()
                .signature_verification_algorithms
                .supported_schemes()
        }
    }
}

#[cfg(feature = "tunnel")]
use no_verify_tls::NoVerifier;
use crate::state::AppState;

/// Write half of the tunnel WebSocket, shared across concurrent handlers.
type TunnelWrite = Arc<tokio::sync::Mutex<
    SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>
>>;

/// Spawn the tunnel client background task.
///
/// Maintains a persistent outbound WSS connection to the portal. Only starts
/// when the license includes the `remote_access` feature and `tunnel.enabled` is true.
/// Skips entirely in mock/dev mode (no real connectivity).
pub fn spawn_tunnel_task(state: Arc<AppState>) {
    if std::env::var("MOCK_HARDWARE").is_ok() {
        debug!("Tunnel task disabled in mock/dev mode");
        return;
    }

    tokio::spawn(async move {
        // Initial delay — let the system stabilize after boot
        sleep(Duration::from_secs(10)).await;
        tunnel_client_loop(state).await;
    });
}

/// Persistent reconnection loop for the tunnel client.
///
/// Checks license and config before each connection attempt. If not eligible,
/// waits 60s and re-checks (allows dynamic enable). Uses exponential backoff
/// with jitter on connection errors.
async fn tunnel_client_loop(state: Arc<AppState>) {
    let mut backoff_secs: u64 = 1;

    loop {
        // Check whether we should attempt a connection at all
        let (eligible, tunnel_url, ports) = {
            let license = state.license_state.read().await;
            let has_feature = license.has_feature("remote_access");
            drop(license);

            let config = state.config.read().await;
            let enabled = config.tunnel.enabled;
            let url = config.portal.resolved_tunnel_url();
            let ports = config.tunnel.ports.clone();
            drop(config);

            (has_feature && enabled, url, ports)
        };

        if !eligible {
            debug!("Tunnel client: not eligible (license or config), sleeping 60s");
            tokio::select! {
                _ = sleep(Duration::from_secs(60)) => {}
                _ = state.tunnel_shutdown.notified() => {
                    info!("Tunnel client: shutdown signal received, exiting");
                    return;
                }
            }
            continue;
        }

        let device_token = state.device_token.clone();
        info!("Tunnel client: connecting to {tunnel_url}");

        match connect_and_run(&state, &tunnel_url, &device_token, &ports).await {
            Ok(()) => {
                // Clean close — reset backoff
                info!("Tunnel client: connection closed cleanly, will reconnect");
                backoff_secs = 1;
            }
            Err(TunnelRunError::Rejected { reason }) => {
                // Server rejected the auth/config phase. Force a long backoff for
                // the next reconnect — short reconnect on persistent rejection
                // (e.g. unknown_device, feature_not_licensed) would hammer the
                // portal. The cap-at-300 line below keeps subsequent iterations
                // at 300 too, until a successful connection resets backoff to 1.
                //
                // `incumbent_active` is the exception (Item #3 Phase 4): a live
                // incumbent — often this router's own prior half-open connection —
                // already holds the tunnel. The portal reaps a stale incumbent
                // within ~45s, so a short 60s retry recovers quickly without
                // hammering, rather than waiting the full 300s.
                if reason == "incumbent_active" {
                    info!("Tunnel client: rejected (reason: {reason}), backing off 60s");
                    backoff_secs = 60;
                } else {
                    info!("Tunnel client: rejected (reason: {reason}), backing off 300s");
                    backoff_secs = 300;
                }
            }
            Err(TunnelRunError::Other(e)) => {
                error!("Tunnel client: connection error: {e}");
            }
        }

        // Exponential backoff with simple jitter (±25%)
        let jitter_ms = backoff_secs * 250; // 25% of backoff in ms
        let sleep_ms = backoff_secs * 1000 + jitter_ms;
        info!("Tunnel client: reconnecting in {backoff_secs}s");

        tokio::select! {
            _ = sleep(Duration::from_millis(sleep_ms)) => {}
            _ = state.tunnel_shutdown.notified() => {
                info!("Tunnel client: shutdown signal received, exiting");
                return;
            }
        }

        // Cap backoff at 300s
        backoff_secs = (backoff_secs * 2).min(300);
    }
}

/// Outcome of a single tunnel WebSocket lifecycle.
///
/// `Rejected` carries the rejection reason from the server (matches one of the
/// four documented `reason` codes in the design spec §4). `tunnel_client_loop`
/// uses this to force a 300s backoff distinct from generic connection errors.
///
/// `Other` wraps any other error path (connect failure, send failure, read
/// error, ping-send failure, etc.). `From<anyhow::Error>` is implemented so
/// existing `?` operators continue to work.
#[derive(Debug)]
pub enum TunnelRunError {
    Rejected { reason: String },
    Other(anyhow::Error),
}

impl From<anyhow::Error> for TunnelRunError {
    fn from(e: anyhow::Error) -> Self {
        TunnelRunError::Other(e)
    }
}

impl std::fmt::Display for TunnelRunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TunnelRunError::Rejected { reason } => write!(f, "tunnel rejected: {reason}"),
            TunnelRunError::Other(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for TunnelRunError {}

/// Run a single WebSocket connection lifecycle.
///
/// Sends auth + config frames, then enters the main select loop:
/// - Incoming messages dispatched to handlers
/// - Keepalive pings every 30s
/// - Graceful shutdown on signal
async fn connect_and_run(
    state: &Arc<AppState>,
    tunnel_url: &str,
    device_token: &str,
    ports: &[u16],
) -> Result<(), TunnelRunError> {
    let (ws_stream, _) = connect_async(tunnel_url).await.map_err(anyhow::Error::from)?;
    let (write_half, mut read_half) = ws_stream.split();
    let write: TunnelWrite = Arc::new(tokio::sync::Mutex::new(write_half));

    // ── Handshake: read the server challenge, reply with a signed auth ─────────
    // Item #3 Phase 4/5. The portal sends a `TunnelChallenge { nonce }` as its
    // first frame; the router signs the canonical tunnel bytes over that nonce and
    // replies with a signed `TunnelAuth`. The challenge nonce is ephemeral — verified
    // within this handshake only, never stored/rotated.
    //
    // Phase 5b: legacy unsigned-auth fallback removed. The portal no longer accepts
    // unsigned `TunnelAuth` (all enrolled devices required). If no valid challenge
    // arrives, disconnect immediately so the reconnection loop retries with backoff.
    let auth = match tokio::time::timeout(Duration::from_secs(10), read_half.next()).await {
        Ok(Some(Ok(Message::Text(text)))) => {
            match serde_json::from_str::<TunnelRequest>(&text) {
                Ok(TunnelRequest::TunnelChallenge { nonce }) => {
                    let canonical =
                        crate::security::device_auth::canonical_tunnel(device_token, &nonce);
                    let sig = state.device_auth.sign(&canonical);
                    TunnelResponse::TunnelAuth {
                        device_token: device_token.to_string(),
                        kid: Some(state.device_auth.key_id.clone()),
                        signature: Some(sig),
                    }
                }
                // A parse error or a non-challenge first frame: the portal must send
                // a challenge before auth. Close the connection and let the
                // reconnection loop retry; do not send unsigned auth (Phase 5b).
                _ => {
                    warn!(
                        "Tunnel client: unexpected first frame (expected TunnelChallenge); \
                         closing connection"
                    );
                    return Err(anyhow::anyhow!(
                        "tunnel handshake failed: unexpected first frame"
                    )
                    .into());
                }
            }
        }
        // Timeout, a non-Text frame, a read error, or a closed stream before any
        // challenge: close and let the reconnection loop retry (Phase 5b).
        _ => {
            warn!(
                "Tunnel client: no challenge received before timeout; \
                 closing connection"
            );
            return Err(anyhow::anyhow!(
                "tunnel handshake failed: no challenge received"
            )
            .into());
        }
    };
    send_response(&write, &auth).await?;

    // Send config frame
    let config_frame = TunnelResponse::TunnelConfig {
        ports: ports.to_vec(),
    };
    send_response(&write, &config_frame).await?;

    info!("Tunnel client: authenticated and configured");

    // WebSocket stream tracking
    let ws_streams: WsStreamMap = Arc::new(tokio::sync::RwLock::new(HashMap::new()));

    // Spawn idle stream checker
    let idle_checker = {
        let ws_streams = Arc::clone(&ws_streams);
        let write = Arc::clone(&write);
        tokio::spawn(async move {
            ws_idle_checker(ws_streams, write).await;
        })
    };

    let mut ping_interval = tokio::time::interval(Duration::from_secs(30));
    ping_interval.tick().await; // consume the immediate first tick

    let result: Result<(), TunnelRunError> = async {
        loop {
            tokio::select! {
                msg = read_half.next() => {
                    match msg {
                        Some(Ok(Message::Text(text))) => {
                            let request: TunnelRequest = match serde_json::from_str(&text) {
                                Ok(r) => r,
                                Err(e) => {
                                    warn!("Tunnel client: failed to deserialize request: {e}");
                                    continue;
                                }
                            };
                            // Intercept server rejection signals before spawning a handler.
                            // Operator-actionable reasons log at warn; transient/engineer-facing
                            // reasons log at info. See spec §6.3 for the level table.
                            if let TunnelRequest::TunnelRejection { reason, message } = &request {
                                match reason.as_str() {
                                    // Operator-actionable: unknown/unlicensed device, or
                                    // (Item #3 Phase 4) an enrolled device whose tunnel
                                    // signature was absent/invalid, or signing required but
                                    // no key is pinned.
                                    "unknown_device"
                                    | "feature_not_licensed"
                                    | "bad_signature"
                                    | "not_enrolled" => {
                                        warn!(
                                            "Tunnel client: rejected by portal ({reason}): {message}"
                                        );
                                    }
                                    // Transient/self-resolving (e.g. `incumbent_active` — a
                                    // live incumbent the portal reaps within ~45s).
                                    "incumbent_active" => {
                                        info!(
                                            "Tunnel client: rejected by portal ({reason}): {message}"
                                        );
                                    }
                                    _ => {
                                        info!(
                                            "Tunnel client: rejected by portal ({reason}): {message}"
                                        );
                                    }
                                }
                                return Err(TunnelRunError::Rejected {
                                    reason: reason.clone(),
                                });
                            }
                            let state_clone = Arc::clone(state);
                            let write_clone = Arc::clone(&write);
                            let ws_streams_clone = Arc::clone(&ws_streams);
                            tokio::spawn(async move {
                                handle_tunnel_message(
                                    state_clone, request, write_clone, ws_streams_clone,
                                ).await;
                            });
                        }
                        Some(Ok(Message::Close(_))) => {
                            info!("Tunnel client: received Close frame from portal");
                            return Ok(());
                        }
                        Some(Ok(Message::Ping(data))) => {
                            // Respond to pings
                            let mut w = write.lock().await;
                            let _ = w.send(Message::Pong(data)).await;
                        }
                        Some(Ok(_)) => {
                            // Ignore binary, pong, etc.
                        }
                        Some(Err(e)) => {
                            return Err(anyhow::anyhow!("WebSocket read error: {e}").into());
                        }
                        None => {
                            info!("Tunnel client: stream ended");
                            return Ok(());
                        }
                    }
                }
                _ = ping_interval.tick() => {
                    let mut w = write.lock().await;
                    if let Err(e) = w.send(Message::Ping(vec![])).await {
                        return Err(anyhow::anyhow!("Failed to send ping: {e}").into());
                    }
                    debug!("Tunnel client: sent keepalive ping");
                }
                _ = state.tunnel_shutdown.notified() => {
                    info!("Tunnel client: shutdown, sending Close frame");
                    let mut w = write.lock().await;
                    let _ = w.send(Message::Close(None)).await;
                    return Ok(());
                }
            }
        }
    }.await;

    // ── Connection cleanup: abort idle checker and all WS relay tasks ──────────
    idle_checker.abort();
    {
        let mut streams = ws_streams.write().await;
        for (id, handle) in streams.drain() {
            debug!("Tunnel cleanup: closing WS stream {id}");
            for task in handle.relay_tasks {
                task.abort();
            }
        }
    }

    result
}

/// Dispatch an incoming tunnel message to the appropriate handler.
async fn handle_tunnel_message(
    state: Arc<AppState>,
    request: TunnelRequest,
    write: TunnelWrite,
    ws_streams: WsStreamMap,
) {
    match request {
        TunnelRequest::HttpRequest { id, method, port, path, headers, body } => {
            if let Err(e) = handle_http_request(&state, &write, id, method, port, path, headers, body).await {
                error!("Tunnel client: HTTP proxy error: {e}");
            }
        }
        TunnelRequest::HttpRequestStart { .. }
        | TunnelRequest::HttpRequestChunk { .. }
        | TunnelRequest::HttpRequestEnd { .. } => {
            debug!("Tunnel client: chunked HTTP request not yet supported");
        }
        TunnelRequest::WsUpgradeRequest { id, stream_id, port, path, headers } => {
            handle_ws_upgrade(
                &state, &write, &ws_streams,
                id, stream_id, port, path, headers,
            ).await;
        }
        TunnelRequest::WsFrame { stream_id, data, is_binary } => {
            handle_ws_frame(&ws_streams, stream_id, data, is_binary).await;
        }
        TunnelRequest::WsClose { stream_id } => {
            handle_ws_close(&ws_streams, stream_id).await;
        }
        TunnelRequest::TunnelRejection { reason, message } => {
            // Defensive: the read loop intercepts `TunnelRejection` before spawning
            // `handle_tunnel_message`, so this arm should be unreachable in practice.
            // Logged at debug to surface any future code path that bypasses the
            // pre-dispatch peek.
            debug!(
                "Tunnel client: TunnelRejection reached handler ({reason}): {message}"
            );
        }
        TunnelRequest::TunnelChallenge { .. } => {
            // The challenge is consumed during the pre-auth handshake in
            // `connect_and_run`; a challenge arriving on the main loop is unexpected
            // (the portal issues exactly one, before auth). Logged at debug only.
            debug!("Tunnel client: unexpected TunnelChallenge on main loop, ignoring");
        }
    }
}

/// Maximum response body size accepted from local services (50 MB).
#[cfg(feature = "tunnel")]
const MAX_RESPONSE_SIZE: usize = 50 * 1024 * 1024;

/// Maximum request body size accepted from the portal (10 MB).
#[cfg(feature = "tunnel")]
const MAX_REQUEST_SIZE: usize = 10 * 1024 * 1024;

/// Timeout for local HTTP requests proxied through the tunnel.
/// Must be long enough for slow endpoints like speedtest run-sync (~90s).
#[cfg(feature = "tunnel")]
const LOCAL_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// Maximum number of concurrent WebSocket streams per tunnel connection.
#[cfg(feature = "tunnel")]
const MAX_WS_STREAMS: usize = 4;

/// Close WebSocket streams that have been idle for this many seconds.
#[cfg(feature = "tunnel")]
const WS_IDLE_TIMEOUT_SECS: u64 = 300;

/// How often to check for idle WebSocket streams (seconds).
#[cfg(feature = "tunnel")]
const WS_IDLE_CHECK_INTERVAL_SECS: u64 = 30;

/// Maximum WebSocket frames per second per stream (rate limiting).
#[cfg(feature = "tunnel")]
const WS_MAX_FRAMES_PER_SEC: u32 = 30;

/// Maximum size of a single WebSocket frame payload (1 MB).
#[cfg(feature = "tunnel")]
const WS_MAX_FRAME_SIZE: usize = 1_048_576;

/// Simple atomic token bucket rate limiter.
///
/// Allows up to `max_tokens` requests, refilling at `refill_rate` tokens per second.
/// Lock-free: uses atomic operations only.
#[allow(dead_code)]
pub struct TokenBucket {
    tokens: AtomicU32,
    max_tokens: u32,
    last_refill: AtomicU64, // epoch millis
    refill_rate: u32,       // tokens per second
}

impl TokenBucket {
    /// Create a new token bucket with the given capacity and refill rate.
    #[allow(dead_code)]
    fn new(max_tokens: u32, refill_rate: u32) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        Self {
            tokens: AtomicU32::new(max_tokens),
            max_tokens,
            last_refill: AtomicU64::new(now),
            refill_rate,
        }
    }

    /// Try to consume one token. Returns `true` if allowed, `false` if rate-limited.
    #[allow(dead_code)]
    fn try_consume(&self) -> bool {
        // Refill tokens based on elapsed time
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let last = self.last_refill.load(Ordering::Relaxed);
        let elapsed_ms = now.saturating_sub(last);

        if elapsed_ms > 0 {
            let new_tokens = (elapsed_ms as u32 * self.refill_rate) / 1000;
            if new_tokens > 0 {
                // Try to update last_refill (best-effort, races are acceptable)
                let _ = self.last_refill.compare_exchange(
                    last, now, Ordering::Relaxed, Ordering::Relaxed,
                );
                let current = self.tokens.load(Ordering::Relaxed);
                let refilled = current.saturating_add(new_tokens).min(self.max_tokens);
                self.tokens.store(refilled, Ordering::Relaxed);
            }
        }

        // Try to consume one token
        loop {
            let current = self.tokens.load(Ordering::Relaxed);
            if current == 0 {
                return false;
            }
            match self.tokens.compare_exchange(
                current, current - 1, Ordering::Relaxed, Ordering::Relaxed,
            ) {
                Ok(_) => return true,
                Err(_) => continue, // retry on contention
            }
        }
    }
}

/// Handle for an active proxied WebSocket stream.
///
/// Holds the channel sender for inbound frames (portal -> local service),
/// the relay task handles, and activity/rate-limit state.
#[allow(dead_code)]
pub struct WsStreamHandle {
    /// Send frames to the local WebSocket sink.
    inbound_tx: tokio::sync::mpsc::Sender<(Vec<u8>, bool)>,
    /// Background relay tasks (inbound + outbound) — aborted on cleanup.
    relay_tasks: Vec<JoinHandle<()>>,
    /// Last activity timestamp (epoch millis) for idle detection.
    last_activity: Arc<AtomicU64>,
    /// Per-stream rate limiter.
    rate_limiter: Arc<TokenBucket>,
}

/// Concurrent map of active WebSocket streams, keyed by stream_id.
type WsStreamMap = Arc<tokio::sync::RwLock<HashMap<String, WsStreamHandle>>>;

/// Hop-by-hop headers that must not be forwarded through the tunnel proxy.
#[cfg(feature = "tunnel")]
const HOP_BY_HOP_HEADERS: &[&str] = &[
    "transfer-encoding",
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "upgrade",
];

/// Real HTTP proxy handler — forwards requests to local 127.0.0.1:{port} via reqwest.
///
/// Enforces port allowlist, size limits, and chunked transfer for large responses.
#[cfg(feature = "tunnel")]
#[allow(clippy::too_many_arguments)]
async fn handle_http_request(
    state: &AppState,
    write: &TunnelWrite,
    id: String,
    method: String,
    port: u16,
    path: String,
    headers: Vec<(String, String)>,
    body: Option<String>,
) -> anyhow::Result<()> {
    use base64::engine::general_purpose::STANDARD as BASE64;

    // ── 1. Port allowlist check ────────────────────────────────────────────────
    let allowed_ports = {
        let config = state.config.read().await;
        config.tunnel.ports.clone()
    };
    if !allowed_ports.contains(&port) {
        warn!("Tunnel: blocked request to port {} (not in allowed list)", port);
        return send_response(write, &TunnelResponse::HttpResponse {
            id,
            status: 403,
            headers: vec![("content-type".into(), "text/plain".into())],
            body: Some(BASE64.encode(b"Port not allowed")),
        }).await;
    }

    // ── 2. Build local URL ─────────────────────────────────────────────────────
    // Only 443 and 8443 use TLS locally (nginx and modem-interface).
    // All other ports (80, 7681/ttyd, etc.) are plain HTTP.
    let scheme = if port == 443 || port == 8443 { "https" } else { "http" };
    let url = format!("{}://127.0.0.1:{}{}", scheme, port, path);

    // ── 3. Decode request body ─────────────────────────────────────────────────
    let request_body: Option<Vec<u8>> = match body {
        Some(ref b64) => {
            let decoded = BASE64.decode(b64).map_err(|e| {
                anyhow::anyhow!("Failed to decode request body: {e}")
            })?;
            if decoded.len() > MAX_REQUEST_SIZE {
                warn!("Tunnel: request body too large ({} bytes), rejecting", decoded.len());
                return send_response(write, &TunnelResponse::HttpResponse {
                    id,
                    status: 413,
                    headers: vec![("content-type".into(), "text/plain".into())],
                    body: Some(BASE64.encode(b"Request body too large")),
                }).await;
            }
            Some(decoded)
        }
        None => None,
    };

    // ── 4. Build reqwest client ────────────────────────────────────────────────
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(LOCAL_REQUEST_TIMEOUT)
        .build()
        .map_err(|e| anyhow::anyhow!("Failed to build HTTP client: {e}"))?;

    // ── 5. Build request ───────────────────────────────────────────────────────
    let http_method = reqwest::Method::from_bytes(method.as_bytes())
        .unwrap_or(reqwest::Method::GET);

    let mut req_builder = client.request(http_method, &url);

    // Pass through headers, dropping hop-by-hop headers only.
    // Host is passed through unchanged so local services (LuCI/nginx) generate
    // redirect URLs and cookie domains matching the tunnel subdomain, not 127.0.0.1.
    for (key, value) in &headers {
        let lower = key.to_lowercase();
        if HOP_BY_HOP_HEADERS.contains(&lower.as_str()) {
            // Skip hop-by-hop headers
        } else {
            req_builder = req_builder.header(key.as_str(), value.as_str());
        }
    }

    if let Some(body_bytes) = request_body {
        req_builder = req_builder.body(body_bytes);
    }

    // ── 6. Execute request ─────────────────────────────────────────────────────
    let response = match req_builder.send().await {
        Ok(r) => r,
        Err(e) => {
            warn!("Tunnel: local HTTP request failed: {e}");
            return send_response(write, &TunnelResponse::HttpResponse {
                id,
                status: 502,
                headers: vec![("content-type".into(), "text/plain".into())],
                body: Some(BASE64.encode(format!("Bad gateway: {e}").as_bytes())),
            }).await;
        }
    };

    // ── 7. Read response metadata ──────────────────────────────────────────────
    let status = response.status().as_u16();

    let mut response_headers: Vec<(String, String)> = Vec::new();
    for (key, value) in response.headers() {
        let lower = key.as_str().to_lowercase();
        if HOP_BY_HOP_HEADERS.contains(&lower.as_str()) {
            continue;
        }
        if let Ok(v) = value.to_str() {
            response_headers.push((lower, v.to_string()));
        }
    }

    // ── 8. Read response body with size limit ──────────────────────────────────
    let body_bytes = match response.bytes().await {
        Ok(b) => b,
        Err(e) => {
            warn!("Tunnel: failed to read local HTTP response body: {e}");
            return send_response(write, &TunnelResponse::HttpResponse {
                id,
                status: 502,
                headers: vec![("content-type".into(), "text/plain".into())],
                body: Some(BASE64.encode(format!("Failed to read response: {e}").as_bytes())),
            }).await;
        }
    };

    if body_bytes.len() > MAX_RESPONSE_SIZE {
        warn!("Tunnel: response body too large ({} bytes), rejecting", body_bytes.len());
        return send_response(write, &TunnelResponse::HttpResponse {
            id,
            status: 502,
            headers: vec![("content-type".into(), "text/plain".into())],
            body: Some(BASE64.encode(b"Response body too large")),
        }).await;
    }

    // ── 9. Send response — single frame or chunked ────────────────────────────
    if body_bytes.len() <= CHUNK_SIZE {
        // Single-frame response
        let body_b64 = if body_bytes.is_empty() {
            None
        } else {
            Some(BASE64.encode(&body_bytes))
        };
        send_response(write, &TunnelResponse::HttpResponse {
            id,
            status,
            headers: response_headers,
            body: body_b64,
        }).await
    } else {
        // Chunked response
        send_response(write, &TunnelResponse::HttpResponseStart {
            id: id.clone(),
            status,
            headers: response_headers,
        }).await?;

        for chunk in body_bytes.chunks(CHUNK_SIZE) {
            send_response(write, &TunnelResponse::HttpResponseChunk {
                id: id.clone(),
                body: BASE64.encode(chunk),
            }).await?;
        }

        send_response(write, &TunnelResponse::HttpResponseEnd {
            id,
        }).await
    }
}

/// Stub HTTP proxy handler for when the `tunnel` feature is disabled — returns 501.
#[cfg(not(feature = "tunnel"))]
#[allow(clippy::too_many_arguments)]
async fn handle_http_request(
    _state: &AppState,
    write: &TunnelWrite,
    id: String,
    _method: String,
    _port: u16,
    _path: String,
    _headers: Vec<(String, String)>,
    _body: Option<String>,
) -> anyhow::Result<()> {
    use base64::engine::general_purpose::STANDARD as BASE64;
    send_response(write, &TunnelResponse::HttpResponse {
        id,
        status: 501,
        headers: vec![("content-type".into(), "application/json".into())],
        body: Some(BASE64.encode(br#"{"error":"HTTP proxy not available (tunnel feature disabled)"}"#)),
    }).await
}

/// Periodically check for idle WebSocket streams and close them.
#[cfg(feature = "tunnel")]
async fn ws_idle_checker(ws_streams: WsStreamMap, write: TunnelWrite) {
    let mut interval = tokio::time::interval(Duration::from_secs(WS_IDLE_CHECK_INTERVAL_SECS));
    loop {
        interval.tick().await;

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let mut expired = Vec::new();
        {
            let streams = ws_streams.read().await;
            for (id, handle) in streams.iter() {
                let last = handle.last_activity.load(Ordering::Relaxed);
                if now.saturating_sub(last) > WS_IDLE_TIMEOUT_SECS * 1000 {
                    expired.push(id.clone());
                }
            }
        }

        for stream_id in expired {
            info!("Tunnel WS: closing idle stream {stream_id}");
            // Send close notification to portal
            let _ = send_response(&write, &TunnelResponse::WsClose {
                stream_id: stream_id.clone(),
            }).await;
            // Remove handle — dropping inbound_tx closes the channel so the inbound
            // relay sends Message::Close to the local WS before exiting.
            let handle = {
                let mut streams = ws_streams.write().await;
                streams.remove(&stream_id)
            };
            if let Some(handle) = handle {
                drop(handle.inbound_tx);
                // Yield to let the inbound relay send the Close frame.
                tokio::task::yield_now().await;
                for task in handle.relay_tasks {
                    task.abort();
                }
            }
        }
    }
}

/// Stub idle checker for non-tunnel builds.
#[cfg(not(feature = "tunnel"))]
async fn ws_idle_checker(_ws_streams: WsStreamMap, _write: TunnelWrite) {
    // No-op: tunnel feature disabled
    std::future::pending::<()>().await;
}

/// Handle a WebSocket upgrade request — connect to local service and set up bidirectional relay.
#[cfg(feature = "tunnel")]
#[allow(clippy::too_many_arguments)]
async fn handle_ws_upgrade(
    state: &AppState,
    write: &TunnelWrite,
    ws_streams: &WsStreamMap,
    id: String,
    stream_id: String,
    port: u16,
    path: String,
    headers: Vec<(String, String)>,
) {
    use base64::engine::general_purpose::STANDARD as BASE64;

    // ── 1. Port allowlist check ───────────────────────────────────────────────
    let allowed_ports = {
        let config = state.config.read().await;
        config.tunnel.ports.clone()
    };
    if !allowed_ports.contains(&port) {
        warn!("Tunnel WS: blocked upgrade to port {port} (not in allowed list)");
        let _ = send_response(write, &TunnelResponse::WsUpgradeResponse {
            id, stream_id, accepted: false,
        }).await;
        return;
    }

    // ── 2. Max concurrent streams check ───────────────────────────────────────
    {
        let streams = ws_streams.read().await;
        if streams.len() >= MAX_WS_STREAMS {
            warn!("Tunnel WS: max concurrent streams ({MAX_WS_STREAMS}) reached, rejecting");
            let _ = send_response(write, &TunnelResponse::WsUpgradeResponse {
                id, stream_id, accepted: false,
            }).await;
            return;
        }
    }

    // ── 3. Build local WS URL ─────────────────────────────────────────────────
    // Only 443 and 8443 use TLS locally. Same scheme logic as the HTTP proxy handler.
    let ws_scheme = if port == 443 || port == 8443 { "wss" } else { "ws" };
    let url = format!("{ws_scheme}://127.0.0.1:{port}{path}");
    debug!("Tunnel WS: connecting to local {url}");

    // ── 4. Connect to local WebSocket service ─────────────────────────────────
    // Build a request with forwarded headers (e.g., Sec-WebSocket-Protocol for ttyd).
    let mut ws_request = url.into_client_request()
        .expect("valid WS URL");
    for (key, value) in &headers {
        if let (Ok(name), Ok(val)) = (
            reqwest::header::HeaderName::from_bytes(key.as_bytes()),
            reqwest::header::HeaderValue::from_str(value),
        ) {
            ws_request.headers_mut().insert(name, val);
        }
    }

    // For TLS ports (443, 8443), use a custom rustls config that accepts self-signed certs.
    // All other ports use plain ws://.
    let local_ws = if port != 443 && port != 8443 {
        match connect_async(ws_request).await {
            Ok((ws, _)) => ws,
            Err(e) => {
                warn!("Tunnel WS: failed to connect to local service: {e}");
                let _ = send_response(write, &TunnelResponse::WsUpgradeResponse {
                    id, stream_id, accepted: false,
                }).await;
                return;
            }
        }
    } else {
        let tls_config = rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoVerifier))
            .with_no_client_auth();
        let connector = Connector::Rustls(Arc::new(tls_config));
        match connect_async_tls_with_config(ws_request, None, false, Some(connector)).await {
            Ok((ws, _)) => ws,
            Err(e) => {
                warn!("Tunnel WS: failed to connect to local service: {e}");
                let _ = send_response(write, &TunnelResponse::WsUpgradeResponse {
                    id, stream_id, accepted: false,
                }).await;
                return;
            }
        }
    };

    // ── 5. Send success response ──────────────────────────────────────────────
    if let Err(e) = send_response(write, &TunnelResponse::WsUpgradeResponse {
        id, stream_id: stream_id.clone(), accepted: true,
    }).await {
        error!("Tunnel WS: failed to send upgrade response: {e}");
        return;
    }

    // ── 6. Split local WS and set up relay ────────────────────────────────────
    let (local_sink, mut local_stream) = local_ws.split();
    let local_sink = Arc::new(tokio::sync::Mutex::new(local_sink));

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let last_activity = Arc::new(AtomicU64::new(now));
    let rate_limiter = Arc::new(TokenBucket::new(WS_MAX_FRAMES_PER_SEC, WS_MAX_FRAMES_PER_SEC));

    // Channel for inbound frames (portal -> local service)
    let (inbound_tx, mut inbound_rx) = tokio::sync::mpsc::channel::<(Vec<u8>, bool)>(64);

    // ── 7. Inbound relay: portal -> local service ─────────────────────────────
    let inbound_task = {
        let local_sink = Arc::clone(&local_sink);
        let last_activity = Arc::clone(&last_activity);
        tokio::spawn(async move {
            while let Some((data, is_binary)) = inbound_rx.recv().await {
                let msg = if is_binary {
                    Message::Binary(data)
                } else {
                    match String::from_utf8(data) {
                        Ok(text) => Message::Text(text),
                        Err(e) => {
                            // Fall back to binary if not valid UTF-8
                            Message::Binary(e.into_bytes())
                        }
                    }
                };
                let mut sink = local_sink.lock().await;
                if let Err(e) = sink.send(msg).await {
                    debug!("Tunnel WS inbound relay: send error: {e}");
                    break;
                }
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                last_activity.store(now, Ordering::Relaxed);
            }
            // Channel closed (sender dropped) — send a clean Close frame to local WS
            let mut sink = local_sink.lock().await;
            let _ = sink.send(Message::Close(None)).await;
        })
    };

    // ── 8. Outbound relay: local service -> portal ────────────────────────────
    let outbound_task = {
        let stream_id_clone = stream_id.clone();
        let write_clone = Arc::clone(write);
        let ws_streams_clone = Arc::clone(ws_streams);
        let last_activity = Arc::clone(&last_activity);
        tokio::spawn(async move {
            while let Some(msg_result) = local_stream.next().await {
                match msg_result {
                    Ok(Message::Text(text)) => {
                        let data = text.as_bytes();
                        if data.len() > WS_MAX_FRAME_SIZE {
                            warn!("Tunnel WS outbound: frame too large ({} bytes), dropping", data.len());
                            continue;
                        }
                        let encoded = BASE64.encode(data);
                        let _ = send_response(&write_clone, &TunnelResponse::WsFrame {
                            stream_id: stream_id_clone.clone(),
                            data: encoded,
                            is_binary: false,
                        }).await;
                    }
                    Ok(Message::Binary(data)) => {
                        if data.len() > WS_MAX_FRAME_SIZE {
                            warn!("Tunnel WS outbound: frame too large ({} bytes), dropping", data.len());
                            continue;
                        }
                        let encoded = BASE64.encode(&data);
                        let _ = send_response(&write_clone, &TunnelResponse::WsFrame {
                            stream_id: stream_id_clone.clone(),
                            data: encoded,
                            is_binary: true,
                        }).await;
                    }
                    Ok(Message::Close(_)) => {
                        info!("Tunnel WS: local service closed stream {stream_id_clone}");
                        break;
                    }
                    Ok(Message::Ping(data)) => {
                        // Respond to local pings
                        let mut sink = local_sink.lock().await;
                        let _ = sink.send(Message::Pong(data)).await;
                    }
                    Ok(_) => {} // Ignore Pong, Frame
                    Err(e) => {
                        debug!("Tunnel WS outbound relay: read error: {e}");
                        break;
                    }
                }
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                last_activity.store(now, Ordering::Relaxed);
            }

            // Local WS closed — notify portal and clean up
            let _ = send_response(&write_clone, &TunnelResponse::WsClose {
                stream_id: stream_id_clone.clone(),
            }).await;

            let mut streams = ws_streams_clone.write().await;
            if let Some(handle) = streams.remove(&stream_id_clone) {
                for task in handle.relay_tasks {
                    task.abort();
                }
            }
        })
    };

    // ── 9. Register stream ────────────────────────────────────────────────────
    let handle = WsStreamHandle {
        inbound_tx,
        relay_tasks: vec![inbound_task, outbound_task],
        last_activity,
        rate_limiter,
    };
    // Re-check max streams under write lock to prevent TOCTOU race: two concurrent
    // upgrade requests could both pass the read-locked check above, then both insert.
    {
        let mut map = ws_streams.write().await;
        if map.len() >= MAX_WS_STREAMS {
            warn!("Tunnel WS: max concurrent streams ({MAX_WS_STREAMS}) reached during upgrade (race), closing");
            for task in handle.relay_tasks {
                task.abort();
            }
            let _ = send_response(write, &TunnelResponse::WsClose {
                stream_id,
            }).await;
            return;
        }
        map.insert(stream_id.clone(), handle);
    }
    info!("Tunnel WS: stream {stream_id} established");
}

/// Stub WS upgrade handler for non-tunnel builds — always rejects.
#[cfg(not(feature = "tunnel"))]
#[allow(clippy::too_many_arguments)]
async fn handle_ws_upgrade(
    _state: &AppState,
    write: &TunnelWrite,
    _ws_streams: &WsStreamMap,
    id: String,
    stream_id: String,
    _port: u16,
    _path: String,
    _headers: Vec<(String, String)>,
) {
    let _ = send_response(write, &TunnelResponse::WsUpgradeResponse {
        id, stream_id, accepted: false,
    }).await;
}

/// Handle an incoming WebSocket frame from the portal — forward to local service.
#[cfg(feature = "tunnel")]
async fn handle_ws_frame(
    ws_streams: &WsStreamMap,
    stream_id: String,
    data: String,
    is_binary: bool,
) {
    use base64::engine::general_purpose::STANDARD as BASE64;

    let streams = ws_streams.read().await;
    let handle = match streams.get(&stream_id) {
        Some(h) => h,
        None => {
            debug!("Tunnel WS: frame for unknown stream {stream_id}, dropping");
            return;
        }
    };

    // Rate limit check
    if !handle.rate_limiter.try_consume() {
        debug!("Tunnel WS: rate limited on stream {stream_id}, dropping frame");
        return;
    }

    // Decode payload
    let decoded = match BASE64.decode(&data) {
        Ok(d) => d,
        Err(e) => {
            warn!("Tunnel WS: base64 decode error on stream {stream_id}: {e}");
            return;
        }
    };

    // Size check
    if decoded.len() > WS_MAX_FRAME_SIZE {
        warn!("Tunnel WS: frame too large ({} bytes) on stream {stream_id}, dropping", decoded.len());
        return;
    }

    // Update activity
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    handle.last_activity.store(now, Ordering::Relaxed);

    // Forward to local service
    if handle.inbound_tx.try_send((decoded, is_binary)).is_err() {
        debug!("Tunnel WS: inbound channel full/closed on stream {stream_id}");
    }
}

/// Stub WS frame handler for non-tunnel builds — drops frames.
#[cfg(not(feature = "tunnel"))]
async fn handle_ws_frame(
    _ws_streams: &WsStreamMap,
    _stream_id: String,
    _data: String,
    _is_binary: bool,
) {
    // No-op: tunnel feature disabled
}

/// Handle a WebSocket close from the portal — tear down the local connection.
///
/// Drops `inbound_tx` first (by removing the handle from the map), which causes the
/// inbound relay's `recv()` to return `None`. The relay then sends `Message::Close`
/// to the local WebSocket before exiting. A brief yield gives the relay task a chance
/// to run and send that close frame before we abort both relay tasks.
#[cfg(feature = "tunnel")]
async fn handle_ws_close(ws_streams: &WsStreamMap, stream_id: String) {
    let handle = {
        let mut streams = ws_streams.write().await;
        streams.remove(&stream_id)
    };
    if let Some(handle) = handle {
        info!("Tunnel WS: portal closed stream {stream_id}");
        // Drop inbound_tx — this closes the channel, causing the inbound relay to exit
        // its loop and send Message::Close to the local WS sink.
        drop(handle.inbound_tx);
        // Yield to let the inbound relay task run and send the Close frame.
        tokio::task::yield_now().await;
        // Now abort both relay tasks.
        for task in handle.relay_tasks {
            task.abort();
        }
    } else {
        debug!("Tunnel WS: close for unknown stream {stream_id}");
    }
}

/// Stub WS close handler for non-tunnel builds.
#[cfg(not(feature = "tunnel"))]
async fn handle_ws_close(_ws_streams: &WsStreamMap, _stream_id: String) {
    // No-op: tunnel feature disabled
}

/// Serialize a `TunnelResponse` and send it as a WebSocket Text message.
async fn send_response(write: &TunnelWrite, response: &TunnelResponse) -> anyhow::Result<()> {
    let text = serde_json::to_string(response)?;
    let mut w = write.lock().await;
    w.send(Message::Text(text)).await
        .map_err(|e| anyhow::anyhow!("WebSocket send error: {e}"))
}

// ─── REST API handlers ────────────────────────────────────────────────────────

use axum::{extract::State, Json};
use axum::http::StatusCode;

/// License feature name for remote access / tunnel.
pub const REMOTE_ACCESS_FEATURE: &str = "remote_access";

/// GET /ctrl-modem/api/tunnel/config
pub async fn get_tunnel_config(
    State(state): State<Arc<AppState>>,
) -> Json<serde_json::Value> {
    let config = state.config.read().await;
    let license = state.license_state.read().await;
    let has_feature = license.has_feature(REMOTE_ACCESS_FEATURE);

    Json(serde_json::json!({
        "enabled": config.tunnel.enabled,
        "ports": config.tunnel.ports,
        "url": config.portal.resolved_tunnel_url(),
        "feature_available": has_feature,
    }))
}

/// Request body for PUT /ctrl-modem/api/tunnel/config
#[derive(Debug, Deserialize)]
pub struct UpdateTunnelConfig {
    pub enabled: Option<bool>,
    pub ports: Option<Vec<u16>>,
}

/// PUT /ctrl-modem/api/tunnel/config
pub async fn update_tunnel_config(
    State(state): State<Arc<AppState>>,
    axum::Extension(session_user): axum::Extension<crate::api::auth_middleware::SessionUser>,
    Json(update): Json<UpdateTunnelConfig>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    // Tunnel config is a control operation — require Admin.
    if !session_user.is_admin() {
        return Err((StatusCode::FORBIDDEN, Json(serde_json::json!({
            "error": "Admin access required"
        }))));
    }

    let mut config = state.config.write().await;

    if let Some(enabled) = update.enabled {
        config.tunnel.enabled = enabled;
    }

    if let Some(ports) = update.ports {
        if ports.is_empty() {
            return Err((StatusCode::BAD_REQUEST, Json(serde_json::json!({
                "error": "At least one port must be configured"
            }))));
        }
        for &port in &ports {
            if port == 0 {
                return Err((StatusCode::BAD_REQUEST, Json(serde_json::json!({
                    "error": "Port 0 is not valid"
                }))));
            }
        }
        config.tunnel.ports = ports;
    }

    let config_snapshot = config.clone();
    drop(config);

    if let Err(e) = crate::config::save_config(&config_snapshot).await {
        error!("Failed to save tunnel config: {}", e);
    }

    let license = state.license_state.read().await;
    let has_feature = license.has_feature(REMOTE_ACCESS_FEATURE);

    Ok(Json(serde_json::json!({
        "enabled": config_snapshot.tunnel.enabled,
        "ports": config_snapshot.tunnel.ports,
        "url": config_snapshot.portal.resolved_tunnel_url(),
        "feature_available": has_feature,
    })))
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn update_tunnel_config_forbidden_for_readonly() {
        use axum::extract::State;
        use crate::hardware::AppConfig;
        use crate::hardware::profiles::ProfileRegistry;
        use crate::security::license::LicenseState;
        use crate::security::users::{Role, UserStore};

        let users = UserStore::load("/nonexistent/users.json").await;
        let state = Arc::new(AppState::new(
            AppConfig::default(),
            users,
            ProfileRegistry::load(),
            "test-device-token".to_string(),
            Arc::new(crate::security::device_auth::DeviceAuth::ephemeral()),
            LicenseState::Unlicensed,
        ));
        let readonly = crate::api::auth_middleware::SessionUser {
            username: "viewer".to_string(),
            role: Role::ReadOnly,
        };

        let res = update_tunnel_config(
            State(state),
            axum::Extension(readonly),
            Json(UpdateTunnelConfig { enabled: Some(true), ports: None }),
        )
        .await;

        let (status, _body) = res.expect_err("ReadOnly must be forbidden from tunnel config");
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[test]
    fn test_tunnel_request_serialize_http() {
        let req = TunnelRequest::HttpRequest {
            id: "req_001".into(),
            method: "GET".into(),
            port: 8443,
            path: "/ctrl-modem/".into(),
            headers: vec![("host".into(), "127.0.0.1:8443".into())],
            body: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"type\":\"http_request\""));
        assert!(json.contains("\"port\":8443"));
    }

    #[test]
    fn test_tunnel_request_deserialize_http() {
        let json = r#"{"type":"http_request","id":"req_001","method":"GET","port":8443,"path":"/","headers":[],"body":null}"#;
        let req: TunnelRequest = serde_json::from_str(json).unwrap();
        match req {
            TunnelRequest::HttpRequest { id, port, .. } => {
                assert_eq!(id, "req_001");
                assert_eq!(port, 8443);
            }
            _ => panic!("Expected HttpRequest"),
        }
    }

    #[test]
    fn test_tunnel_response_serialize_http() {
        let resp = TunnelResponse::HttpResponse {
            id: "req_001".into(),
            status: 200,
            headers: vec![("content-type".into(), "text/html".into())],
            body: Some("PGh0bWw+".into()),
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"type\":\"http_response\""));
        assert!(json.contains("\"status\":200"));
    }

    #[test]
    fn test_multi_value_headers_roundtrip() {
        // Verify that multiple Set-Cookie headers are preserved through serialization.
        // This is the key regression test — HashMap would silently drop duplicates.
        let resp = TunnelResponse::HttpResponse {
            id: "req_002".into(),
            status: 302,
            headers: vec![
                ("set-cookie".into(), "session=abc; Path=/; HttpOnly".into()),
                ("set-cookie".into(), "csrf=xyz; Path=/".into()),
                ("location".into(), "/dashboard".into()),
            ],
            body: None,
        };
        let json = serde_json::to_string(&resp).unwrap();
        let deserialized: TunnelResponse = serde_json::from_str(&json).unwrap();
        match deserialized {
            TunnelResponse::HttpResponse { id, status, headers, body } => {
                assert_eq!(id, "req_002");
                assert_eq!(status, 302);
                assert_eq!(body, None);
                // Both Set-Cookie headers must survive the roundtrip
                let set_cookies: Vec<&str> = headers.iter()
                    .filter(|(k, _)| k == "set-cookie")
                    .map(|(_, v)| v.as_str())
                    .collect();
                assert_eq!(set_cookies.len(), 2, "Both Set-Cookie headers must be preserved");
                assert!(set_cookies.contains(&"session=abc; Path=/; HttpOnly"));
                assert!(set_cookies.contains(&"csrf=xyz; Path=/"));
            }
            _ => panic!("Expected HttpResponse"),
        }
    }

    #[test]
    fn test_tunnel_response_deserialize_auth() {
        // Legacy unsigned shape must still deserialize (dual-accept): kid/signature
        // default to None.
        let json = r#"{"type":"tunnel_auth","device_token":"MFRGGZDFMY4T"}"#;
        let resp: TunnelResponse = serde_json::from_str(json).unwrap();
        match resp {
            TunnelResponse::TunnelAuth { device_token, kid, signature } => {
                assert_eq!(device_token, "MFRGGZDFMY4T");
                assert_eq!(kid, None);
                assert_eq!(signature, None);
            }
            _ => panic!("Expected TunnelAuth"),
        }
    }

    #[test]
    fn test_tunnel_response_serialize_auth_legacy_omits_optional_fields() {
        // The unsigned shape must NOT emit kid/signature keys on the wire.
        let resp = TunnelResponse::TunnelAuth {
            device_token: "MFRGGZDFMY4T".into(),
            kid: None,
            signature: None,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"type\":\"tunnel_auth\""));
        assert!(json.contains("\"device_token\":\"MFRGGZDFMY4T\""));
        assert!(!json.contains("kid"));
        assert!(!json.contains("signature"));
    }

    #[test]
    fn test_tunnel_response_serialize_auth_signed_shape() {
        // The signed shape (Item #3 Phase 4) emits kid + signature and round-trips.
        let resp = TunnelResponse::TunnelAuth {
            device_token: "MFRGGZDFMY4T".into(),
            kid: Some("kt1p-9U1qAJn52Oo".into()),
            signature: Some("SIG_B64".into()),
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"kid\":\"kt1p-9U1qAJn52Oo\""));
        assert!(json.contains("\"signature\":\"SIG_B64\""));
        let deserialized: TunnelResponse = serde_json::from_str(&json).unwrap();
        match deserialized {
            TunnelResponse::TunnelAuth { device_token, kid, signature } => {
                assert_eq!(device_token, "MFRGGZDFMY4T");
                assert_eq!(kid.as_deref(), Some("kt1p-9U1qAJn52Oo"));
                assert_eq!(signature.as_deref(), Some("SIG_B64"));
            }
            _ => panic!("Expected TunnelAuth"),
        }
    }

    #[test]
    fn test_tunnel_challenge_deserialize() {
        let json = r#"{"type":"tunnel_challenge","nonce":"CHALLENGE_NONCE_42"}"#;
        let req: TunnelRequest = serde_json::from_str(json).unwrap();
        match req {
            TunnelRequest::TunnelChallenge { nonce } => {
                assert_eq!(nonce, "CHALLENGE_NONCE_42");
            }
            _ => panic!("Expected TunnelChallenge"),
        }
    }

    #[test]
    fn test_tunnel_signature_verifies_against_public_key() {
        // Prove the router's tunnel signature — sign(canonical_tunnel(token, nonce)) —
        // verifies against the public key the portal pins. Item #3 Phase 4.
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
        use ed25519_dalek::{Signature, Verifier, VerifyingKey};

        let da = crate::security::device_auth::DeviceAuth::ephemeral();
        let token = "MFRGGZDFMY4T";
        let nonce = "CHALLENGE_NONCE_42";
        let canonical = crate::security::device_auth::canonical_tunnel(token, nonce);
        let sig_b64 = da.sign(&canonical);

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
            .verify_strict(&canonical, &Signature::from_bytes(&sig_bytes))
            .is_ok());
        // A signature over a different nonce must NOT verify against this challenge.
        let other = crate::security::device_auth::canonical_tunnel(token, "DIFFERENT_NONCE");
        assert!(vk
            .verify_strict(&other, &Signature::from_bytes(&sig_bytes))
            .is_err());
    }

    #[test]
    fn test_token_bucket_basic() {
        let bucket = TokenBucket::new(3, 3);
        // Should allow 3 consecutive consumes
        assert!(bucket.try_consume());
        assert!(bucket.try_consume());
        assert!(bucket.try_consume());
        // 4th should be rate-limited
        assert!(!bucket.try_consume());
    }

    #[test]
    fn test_ws_upgrade_request_deserialize() {
        let json = r#"{"type":"ws_upgrade_request","id":"req_010","stream_id":"ws_001","port":8443,"path":"/api/ws","headers":[["host","127.0.0.1:8443"]]}"#;
        let req: TunnelRequest = serde_json::from_str(json).unwrap();
        match req {
            TunnelRequest::WsUpgradeRequest { id, stream_id, port, path, .. } => {
                assert_eq!(id, "req_010");
                assert_eq!(stream_id, "ws_001");
                assert_eq!(port, 8443);
                assert_eq!(path, "/api/ws");
            }
            _ => panic!("Expected WsUpgradeRequest"),
        }
    }

    #[test]
    fn test_ws_close_roundtrip() {
        let close = TunnelResponse::WsClose {
            stream_id: "ws_001".into(),
        };
        let json = serde_json::to_string(&close).unwrap();
        assert!(json.contains("\"type\":\"ws_close\""));
        assert!(json.contains("\"stream_id\":\"ws_001\""));
        let deserialized: TunnelResponse = serde_json::from_str(&json).unwrap();
        match deserialized {
            TunnelResponse::WsClose { stream_id } => {
                assert_eq!(stream_id, "ws_001");
            }
            _ => panic!("Expected WsClose"),
        }
    }

    #[test]
    fn test_tunnel_ws_frame_roundtrip() {
        let frame = TunnelResponse::WsFrame {
            stream_id: "ws_001".into(),
            data: "aGVsbG8=".into(),
            is_binary: false,
        };
        let json = serde_json::to_string(&frame).unwrap();
        let deserialized: TunnelResponse = serde_json::from_str(&json).unwrap();
        match deserialized {
            TunnelResponse::WsFrame { stream_id, data, is_binary } => {
                assert_eq!(stream_id, "ws_001");
                assert_eq!(data, "aGVsbG8=");
                assert!(!is_binary);
            }
            _ => panic!("Expected WsFrame"),
        }
    }

    #[test]
    fn test_tunnel_rejection_deserialize_unknown_device() {
        let json = r#"{"type":"tunnel_rejection","reason":"unknown_device","message":"unknown device token"}"#;
        let req: TunnelRequest = serde_json::from_str(json).unwrap();
        match req {
            TunnelRequest::TunnelRejection { reason, message } => {
                assert_eq!(reason, "unknown_device");
                assert_eq!(message, "unknown device token");
            }
            _ => panic!("Expected TunnelRejection"),
        }
    }

    #[test]
    fn test_tunnel_rejection_roundtrip_all_reasons() {
        for reason in [
            "unknown_device",
            "feature_not_licensed",
            "protocol_error",
            "server_error",
        ] {
            let original = TunnelRequest::TunnelRejection {
                reason: reason.to_string(),
                message: format!("test message for {reason}"),
            };
            let json = serde_json::to_string(&original).unwrap();
            assert!(json.contains("\"type\":\"tunnel_rejection\""));
            assert!(json.contains(&format!("\"reason\":\"{reason}\"")));
            let deserialized: TunnelRequest = serde_json::from_str(&json).unwrap();
            match deserialized {
                TunnelRequest::TunnelRejection { reason: r, message: m } => {
                    assert_eq!(r, reason);
                    assert_eq!(m, format!("test message for {reason}"));
                }
                _ => panic!("Expected TunnelRejection for reason={reason}"),
            }
        }
    }

    #[test]
    fn test_legacy_untagged_error_falls_through() {
        // Verifies that the old-server `{"error":"..."}` shape does NOT parse as
        // any TunnelRequest variant — it falls through to the `Err(_)` branch,
        // preserving the existing `failed to deserialize` warn during the
        // cross-boundary rollout window (Spec §6.2 step 4 / §10.2 T6).
        let legacy_json = r#"{"error":"unknown device"}"#;
        let result: Result<TunnelRequest, _> = serde_json::from_str(legacy_json);
        assert!(result.is_err(), "Untagged legacy error must not deserialize as a TunnelRequest variant");
    }
}
