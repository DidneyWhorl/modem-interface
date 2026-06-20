//! Embedded speedtest engine using Cloudflare speed endpoints.
//!
//! Uses progressive payload sizing with p90 percentile aggregation,
//! loaded latency measurement, Server-Timing parsing, cf-meta-*
//! metadata extraction, and AIM quality scoring.
//!
//! Results are persisted in a ring-buffer JSON file on disk.

use std::collections::VecDeque;
use std::path::Path;

use serde::{Deserialize, Serialize};
use tracing;

use crate::hardware::types::{SpeedtestMode, SpeedtestResult};

/// Maximum number of speedtest results to retain.
const MAX_HISTORY: usize = 50;

/// Default persistence path on the router filesystem.
const HISTORY_PATH: &str = "/etc/modem-interface/speedtest-history.json";

// ============================================================================
// History persistence
// ============================================================================

/// Ring-buffer of speedtest results, persisted to JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpeedtestHistory {
    pub results: VecDeque<SpeedtestResult>,
}

impl SpeedtestHistory {
    pub fn new() -> Self {
        Self {
            results: VecDeque::new(),
        }
    }

    /// Push a result, evicting the oldest if at capacity.
    pub fn push(&mut self, result: SpeedtestResult) {
        if self.results.len() >= MAX_HISTORY {
            self.results.pop_front();
        }
        self.results.push_back(result);
    }
}

impl Default for SpeedtestHistory {
    fn default() -> Self {
        Self::new()
    }
}

/// Load history from the default JSON file on disk.
pub fn load_history() -> SpeedtestHistory {
    load_history_from(Path::new(HISTORY_PATH))
}

/// Load history from a specific path (useful for testing).
pub fn load_history_from(path: &Path) -> SpeedtestHistory {
    match std::fs::read_to_string(path) {
        Ok(data) => serde_json::from_str(&data).unwrap_or_else(|e| {
            tracing::warn!("Failed to parse speedtest history: {e}");
            SpeedtestHistory::new()
        }),
        Err(_) => SpeedtestHistory::new(),
    }
}

/// Save history to the default JSON file on disk.
pub fn save_history(history: &SpeedtestHistory) -> std::io::Result<()> {
    save_history_to(history, Path::new(HISTORY_PATH))
}

/// Save history to a specific path (useful for testing).
pub fn save_history_to(history: &SpeedtestHistory, path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(history)
        .map_err(std::io::Error::other)?;
    std::fs::write(path, json)
}

// ============================================================================
// Test configuration (progressive measurement)
// ============================================================================

/// A single measurement step in the test sequence.
#[derive(Debug, Clone)]
pub struct MeasurementStep {
    pub payload_bytes: u64,
    pub request_count: usize,
}

/// Full configuration for a speedtest mode.
#[derive(Debug, Clone)]
pub struct TestConfig {
    pub latency_probes: usize,
    pub download_steps: Vec<MeasurementStep>,
    pub upload_steps: Vec<MeasurementStep>,
    pub warmup: bool,
    pub measure_loaded_latency: bool,
    pub collect_metadata: bool,
    pub calculate_aim: bool,
    pub parse_tcp_stats: bool,
    pub collect_breakdown: bool,
    pub early_termination_ms: Option<f64>,
    pub min_request_duration_ms: f64,
}

impl TestConfig {
    /// Quick mode (~15 MB): fast probe, no advanced metrics.
    pub fn quick() -> Self {
        Self {
            latency_probes: 5,
            download_steps: vec![
                MeasurementStep { payload_bytes: 100_000, request_count: 3 },
                MeasurementStep { payload_bytes: 1_000_000, request_count: 3 },
            ],
            upload_steps: vec![
                MeasurementStep { payload_bytes: 100_000, request_count: 3 },
                MeasurementStep { payload_bytes: 1_000_000, request_count: 2 },
            ],
            warmup: false,
            measure_loaded_latency: false,
            collect_metadata: false,
            calculate_aim: false,
            parse_tcp_stats: false,
            collect_breakdown: false,
            early_termination_ms: Some(1000.0),
            min_request_duration_ms: 0.0,
        }
    }

    /// Medium mode (~80 MB): moderate testing with AIM scores.
    pub fn medium() -> Self {
        Self {
            latency_probes: 10,
            download_steps: vec![
                MeasurementStep { payload_bytes: 100_000, request_count: 5 },
                MeasurementStep { payload_bytes: 1_000_000, request_count: 5 },
                MeasurementStep { payload_bytes: 10_000_000, request_count: 3 },
            ],
            upload_steps: vec![
                MeasurementStep { payload_bytes: 100_000, request_count: 5 },
                MeasurementStep { payload_bytes: 1_000_000, request_count: 3 },
                MeasurementStep { payload_bytes: 10_000_000, request_count: 2 },
            ],
            warmup: false,
            measure_loaded_latency: true,
            collect_metadata: true,
            calculate_aim: true,
            parse_tcp_stats: false,
            collect_breakdown: false,
            early_termination_ms: Some(3000.0),
            min_request_duration_ms: 0.0,
        }
    }

    /// Full mode (auto-adapting): comprehensive with loaded latency and TCP stats.
    pub fn full() -> Self {
        Self {
            latency_probes: 20,
            download_steps: vec![
                MeasurementStep { payload_bytes: 100_000, request_count: 9 },
                MeasurementStep { payload_bytes: 1_000_000, request_count: 8 },
                MeasurementStep { payload_bytes: 10_000_000, request_count: 6 },
                MeasurementStep { payload_bytes: 25_000_000, request_count: 4 },
            ],
            upload_steps: vec![
                MeasurementStep { payload_bytes: 100_000, request_count: 8 },
                MeasurementStep { payload_bytes: 1_000_000, request_count: 6 },
                MeasurementStep { payload_bytes: 10_000_000, request_count: 4 },
                MeasurementStep { payload_bytes: 25_000_000, request_count: 4 },
            ],
            warmup: true,
            measure_loaded_latency: true,
            collect_metadata: true,
            calculate_aim: true,
            parse_tcp_stats: true,
            collect_breakdown: true,
            early_termination_ms: Some(1000.0),
            min_request_duration_ms: 0.0,
        }
    }

    /// Get config for a given mode.
    pub fn for_mode(mode: SpeedtestMode) -> Self {
        match mode {
            SpeedtestMode::Quick => Self::quick(),
            SpeedtestMode::Medium => Self::medium(),
            SpeedtestMode::Full => Self::full(),
        }
    }

    /// Total estimated bytes for all download steps.
    fn total_download_bytes(&self) -> u64 {
        self.download_steps.iter().map(|s| s.payload_bytes * s.request_count as u64).sum()
    }

    /// Total estimated bytes for all upload steps.
    fn total_upload_bytes(&self) -> u64 {
        self.upload_steps.iter().map(|s| s.payload_bytes * s.request_count as u64).sum()
    }
}

// ============================================================================
// Speedtest engine (requires reqwest via tunnel feature)
// ============================================================================

#[cfg(feature = "tunnel")]
mod engine {
    use super::TestConfig;
    use crate::hardware::types::{
        AimScores, BandwidthPoint, ConnectionMetadata, MeasurementBreakdown,
        SpeedtestPhase, SpeedtestProgress, TcpStats,
    };

    use std::sync::Arc;
    use std::time::Instant;
    use tokio::sync::{broadcast, watch};

    const CLOUDFLARE_BASE: &str = "https://speed.cloudflare.com";

    /// Build an HTTP client bound to a specific network interface.
    pub fn build_client(interface: &str) -> Result<reqwest::Client, String> {
        reqwest::Client::builder()
            .interface(interface)
            .timeout(std::time::Duration::from_secs(90))
            .build()
            .map_err(|e| format!("Failed to build HTTP client for {interface}: {e}"))
    }

    // ========================================================================
    // Helper functions
    // ========================================================================

    /// Calculate the Nth percentile from a slice of values.
    /// Uses linear interpolation between closest ranks.
    fn percentile(values: &[f64], p: f64) -> f64 {
        if values.is_empty() {
            return 0.0;
        }
        let mut sorted = values.to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        if sorted.len() == 1 {
            return sorted[0];
        }
        let rank = p / 100.0 * (sorted.len() - 1) as f64;
        let lower = rank.floor() as usize;
        let upper = rank.ceil() as usize;
        if lower == upper {
            sorted[lower]
        } else {
            let frac = rank - lower as f64;
            sorted[lower] * (1.0 - frac) + sorted[upper] * frac
        }
    }

    /// Calculate jitter as average absolute difference between consecutive samples.
    fn calculate_jitter(samples: &[f64]) -> f64 {
        if samples.len() < 2 {
            return 0.0;
        }
        let diffs: Vec<f64> = samples.windows(2).map(|w| (w[1] - w[0]).abs()).collect();
        diffs.iter().sum::<f64>() / diffs.len() as f64
    }

    /// Format a byte count into a human-readable size label.
    fn format_size_label(bytes: u64) -> String {
        if bytes >= 50_000_000 {
            "50MB".to_string()
        } else if bytes >= 25_000_000 {
            "25MB".to_string()
        } else if bytes >= 10_000_000 {
            "10MB".to_string()
        } else if bytes >= 1_000_000 {
            "1MB".to_string()
        } else if bytes >= 100_000 {
            "100KB".to_string()
        } else {
            format!("{}B", bytes)
        }
    }

    /// Parse server processing time from Server-Timing header.
    /// Looks for `dur=X.X` in the header value. Falls back to 10ms.
    fn parse_server_time_ms(headers: &reqwest::header::HeaderMap) -> f64 {
        headers
            .get("server-timing")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| {
                // Look for dur=N.N pattern
                s.split(';')
                    .find_map(|part| {
                        let trimmed = part.trim();
                        if let Some(val) = trimmed.strip_prefix("dur=") {
                            val.parse::<f64>().ok()
                        } else {
                            None
                        }
                    })
            })
            .unwrap_or(10.0)
    }

    /// Parse TCP stats from Server-Timing cfL4 header.
    /// Format: `cfL4;desc="?proto=TCP&rtt=1234&min_rtt=1000&lost=0&retrans=2&cwnd=10&delivery_rate=50000000"`
    fn parse_server_timing_tcp(headers: &reqwest::header::HeaderMap) -> Option<TcpStats> {
        let header_val = headers.get("server-timing")?.to_str().ok()?;

        // Find the cfL4 section
        let cfl4_section = header_val.split(',').find(|s| s.trim().starts_with("cfL4"))?;

        // Extract the desc="..." value
        let desc_start = cfl4_section.find("desc=\"")? + 6;
        let desc_end = cfl4_section[desc_start..].find('"')? + desc_start;
        let desc = &cfl4_section[desc_start..desc_end];

        // Parse query-string style params
        let params: std::collections::HashMap<&str, &str> = desc
            .trim_start_matches('?')
            .split('&')
            .filter_map(|pair| pair.split_once('='))
            .collect();

        Some(TcpStats {
            rtt_us: params.get("rtt").and_then(|v| v.parse().ok()).unwrap_or(0),
            min_rtt_us: params.get("min_rtt").and_then(|v| v.parse().ok()).unwrap_or(0),
            lost: params.get("lost").and_then(|v| v.parse().ok()).unwrap_or(0),
            retrans: params.get("retrans").and_then(|v| v.parse().ok()).unwrap_or(0),
            cwnd: params.get("cwnd").and_then(|v| v.parse().ok()).unwrap_or(0),
            delivery_rate_bps: params.get("delivery_rate").and_then(|v| v.parse().ok()).unwrap_or(0),
        })
    }

    /// Parse Cloudflare connection metadata from cf-meta-* headers.
    fn parse_cf_metadata(headers: &reqwest::header::HeaderMap) -> ConnectionMetadata {
        let get_str = |name: &str| -> Option<String> {
            headers.get(name).and_then(|v| v.to_str().ok()).map(|s| s.to_string())
        };
        let get_f64 = |name: &str| -> Option<f64> {
            headers.get(name).and_then(|v| v.to_str().ok()).and_then(|s| s.parse().ok())
        };

        let asn: Option<u32> = headers
            .get("cf-meta-asn")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse().ok());

        let asn_name = asn.map(|n| asn_to_name(n).to_string());

        ConnectionMetadata {
            ip: get_str("cf-meta-ip"),
            colo: get_str("cf-meta-colo"),
            city: None, // Resolved later from colo
            country: get_str("cf-meta-country"),
            asn,
            asn_name,
            latitude: get_f64("cf-meta-latitude"),
            longitude: get_f64("cf-meta-longitude"),
        }
    }

    /// Lookup table for major US ISP ASNs.
    fn asn_to_name(asn: u32) -> &'static str {
        match asn {
            7018 => "AT&T",
            7922 | 33491 | 33650 | 33651 | 33652 | 33668 => "Comcast",
            701 | 702 | 22394 => "Verizon",
            20001 | 20115 => "Charter/Spectrum",
            7843 | 33363 => "Cox",
            10796 => "CenturyLink/Lumen",
            5650 | 6939 => "Frontier",
            22773 | 3356 => "Lumen/Level3",
            21928 => "T-Mobile",
            5730 | 14593 => "Windstream",
            11427 | 20214 => "TWC/Spectrum",
            6167 => "Verizon Business",
            174 => "Cogent",
            6461 => "Zayo",
            _ => "Unknown",
        }
    }

    /// Lookup table for Cloudflare PoP IATA codes to city names.
    fn colo_to_city(iata: &str) -> &str {
        match iata {
            "ATL" => "Atlanta, GA",
            "BOS" => "Boston, MA",
            "BUF" => "Buffalo, NY",
            "CLT" => "Charlotte, NC",
            "CMH" => "Columbus, OH",
            "DEN" => "Denver, CO",
            "DFW" => "Dallas, TX",
            "DTW" => "Detroit, MI",
            "EWR" => "Newark, NJ",
            "IAD" => "Ashburn, VA",
            "IAH" => "Houston, TX",
            "IND" => "Indianapolis, IN",
            "JAX" => "Jacksonville, FL",
            "JFK" => "New York, NY",
            "LAS" => "Las Vegas, NV",
            "LAX" => "Los Angeles, CA",
            "MCI" => "Kansas City, MO",
            "MEM" => "Memphis, TN",
            "MIA" => "Miami, FL",
            "MSP" => "Minneapolis, MN",
            "ORD" => "Chicago, IL",
            "PDX" => "Portland, OR",
            "PHL" => "Philadelphia, PA",
            "PHX" => "Phoenix, AZ",
            "PIT" => "Pittsburgh, PA",
            "SAN" => "San Diego, CA",
            "SEA" => "Seattle, WA",
            "SFO" => "San Francisco, CA",
            "SJC" => "San Jose, CA",
            "SLC" => "Salt Lake City, UT",
            "STL" => "St. Louis, MO",
            "TPA" => "Tampa, FL",
            _ => iata,
        }
    }

    /// Group bandwidth points into per-size-label breakdown.
    fn build_breakdown(points: &[BandwidthPoint]) -> Vec<MeasurementBreakdown> {
        let mut map: std::collections::HashMap<String, Vec<f64>> = std::collections::HashMap::new();
        for p in points {
            map.entry(p.size_label.clone()).or_default().push(p.bps);
        }
        // Sort by first occurrence order (roughly small → large)
        let mut seen_order: Vec<String> = Vec::new();
        for p in points {
            if !seen_order.contains(&p.size_label) {
                seen_order.push(p.size_label.clone());
            }
        }
        seen_order
            .into_iter()
            .filter_map(|label| {
                map.remove(&label).map(|points_bps| MeasurementBreakdown {
                    count: points_bps.len(),
                    points_bps,
                    size_label: label,
                })
            })
            .collect()
    }

    // ========================================================================
    // AIM scoring
    // ========================================================================

    /// Interpolate a score from thresholds and corresponding point values.
    fn aim_interpolate(value: f64, thresholds: &[f64], points: &[f64]) -> f64 {
        // If below first threshold, return first point value
        if value <= thresholds[0] {
            return points[0];
        }
        // If above last threshold, return last point value
        if value >= *thresholds.last().unwrap() {
            return *points.last().unwrap();
        }
        // Find the bracket and interpolate
        for i in 0..thresholds.len() - 1 {
            if value >= thresholds[i] && value < thresholds[i + 1] {
                let frac = (value - thresholds[i]) / (thresholds[i + 1] - thresholds[i]);
                return points[i] + frac * (points[i + 1] - points[i]);
            }
        }
        *points.last().unwrap()
    }

    /// Classify a category score into a quality label.
    fn aim_classify(score: f64, thresholds: &[f64; 4]) -> &'static str {
        if score < thresholds[0] {
            "bad"
        } else if score < thresholds[1] {
            "poor"
        } else if score < thresholds[2] {
            "average"
        } else if score < thresholds[3] {
            "good"
        } else {
            "great"
        }
    }

    /// Calculate AIM quality scores from measurement results.
    fn calculate_aim_scores(
        latency_ms: f64,
        jitter_ms: f64,
        packet_loss_ratio: f64,
        download_bps: f64,
        upload_bps: f64,
        loaded_latency_increase_ms: f64,
    ) -> AimScores {
        // Latency scoring: [10,20,50,100,500]ms -> [20,10,5,0,-10,-20]
        let latency_score = aim_interpolate(
            latency_ms,
            &[10.0, 20.0, 50.0, 100.0, 500.0],
            &[20.0, 10.0, 5.0, 0.0, -10.0, -20.0],
        );

        // Packet loss scoring: [0.01,0.05,0.25,0.5] ratio -> [10,5,0,-10,-20]
        let loss_score = aim_interpolate(
            packet_loss_ratio,
            &[0.01, 0.05, 0.25, 0.5],
            &[10.0, 5.0, 0.0, -10.0, -20.0],
        );

        // Jitter scoring: [10,20,100,500]ms -> [10,5,0,-10,-20]
        let jitter_score = aim_interpolate(
            jitter_ms,
            &[10.0, 20.0, 100.0, 500.0],
            &[10.0, 5.0, 0.0, -10.0, -20.0],
        );

        // Download scoring: [1M,10M,50M,100M]bps -> [0,5,10,20,30]
        let download_score = aim_interpolate(
            download_bps,
            &[1_000_000.0, 10_000_000.0, 50_000_000.0, 100_000_000.0],
            &[0.0, 5.0, 10.0, 20.0, 30.0],
        );

        // Upload scoring: same thresholds as download
        let upload_score = aim_interpolate(
            upload_bps,
            &[1_000_000.0, 10_000_000.0, 50_000_000.0, 100_000_000.0],
            &[0.0, 5.0, 10.0, 20.0, 30.0],
        );

        // Loaded latency increase: [10,20,50,100,500]ms -> [20,10,5,0,-10,-20]
        let loaded_score = aim_interpolate(
            loaded_latency_increase_ms,
            &[10.0, 20.0, 50.0, 100.0, 500.0],
            &[20.0, 10.0, 5.0, 0.0, -10.0, -20.0],
        );

        // Streaming = latency + loss + download + loadedIncrease
        let streaming_raw = latency_score + loss_score + download_score + loaded_score;
        let streaming = aim_classify(streaming_raw, &[15.0, 20.0, 40.0, 60.0]);

        // Gaming = latency + loss + loadedIncrease
        let gaming_raw = latency_score + loss_score + loaded_score;
        let gaming = aim_classify(gaming_raw, &[5.0, 15.0, 25.0, 30.0]);

        // Video calls = latency + jitter + loss + loadedIncrease
        let _upload_factor = upload_score; // Available for future use
        let video_raw = latency_score + jitter_score + loss_score + loaded_score;
        let video = aim_classify(video_raw, &[5.0, 15.0, 25.0, 40.0]);

        AimScores {
            streaming: streaming.to_string(),
            gaming: gaming.to_string(),
            video_calls: video.to_string(),
        }
    }

    // ========================================================================
    // Measurement phases
    // ========================================================================

    /// TLS warmup: establish connection and prime the path.
    async fn run_warmup(client: &reqwest::Client) -> Result<(), String> {
        // Zero-byte GET for TLS handshake
        client
            .get(format!("{CLOUDFLARE_BASE}/__down?bytes=0"))
            .send()
            .await
            .map_err(|e| format!("Warmup TLS handshake failed: {e}"))?;

        // One 100KB GET to prime the path
        let resp = client
            .get(format!("{CLOUDFLARE_BASE}/__down?bytes=100000"))
            .send()
            .await
            .map_err(|e| format!("Warmup prime failed: {e}"))?;
        // Consume the body
        let _ = resp.bytes().await;

        Ok(())
    }

    /// Measure unloaded latency via zero-byte GET requests.
    /// Returns (median_ms, jitter_ms, raw samples).
    pub async fn measure_latency(
        client: &reqwest::Client,
        config: &TestConfig,
        test_id: &str,
        progress_tx: &broadcast::Sender<SpeedtestProgress>,
    ) -> Result<(f64, f64, Vec<f64>), String> {
        let mut samples = Vec::with_capacity(config.latency_probes);

        for i in 0..config.latency_probes {
            let start = Instant::now();
            let resp = client
                .get(format!("{CLOUDFLARE_BASE}/__down?bytes=0"))
                .send()
                .await
                .map_err(|e| format!("Latency probe {i} failed: {e}"))?;

            let ttfb = start.elapsed().as_secs_f64() * 1000.0;
            // Consume body to complete the request
            let _ = resp.bytes().await;

            // Subtract estimated server processing time (10ms)
            let adjusted = (ttfb - 10.0).max(0.1);
            samples.push(adjusted);

            let pct = ((i + 1) as f64 / config.latency_probes as f64 * 100.0) as u8;
            let _ = progress_tx.send(SpeedtestProgress {
                test_id: test_id.to_string(),
                phase: SpeedtestPhase::Latency,
                progress_pct: pct,
                current_speed_mbps: 0.0,
                bytes_transferred: 0,
                running_p90_mbps: None,
                size_label: None,
            });
        }

        if samples.is_empty() {
            return Err("No latency samples collected".to_string());
        }

        let median = percentile(&samples, 50.0);
        let jitter = calculate_jitter(&samples);

        Ok((median, jitter, samples))
    }

    /// Internal result from a download or upload measurement phase.
    struct PhaseResult {
        p90_mbps: f64,
        total_bytes: u64,
        points: Vec<BandwidthPoint>,
        server: String,
        metadata: Option<ConnectionMetadata>,
        tcp_stats: Option<TcpStats>,
        loaded_latency_samples: Vec<f64>,
    }

    /// Measure download speed using progressive payload sizes.
    async fn measure_download_progressive(
        client: &reqwest::Client,
        config: &TestConfig,
        test_id: &str,
        progress_tx: &broadcast::Sender<SpeedtestProgress>,
    ) -> Result<PhaseResult, String> {
        let mut all_points: Vec<BandwidthPoint> = Vec::new();
        let mut total_bytes: u64 = 0;
        let mut server = String::from("Cloudflare");
        let mut metadata: Option<ConnectionMetadata> = None;
        let mut last_tcp_stats: Option<TcpStats> = None;
        let total_expected = config.total_download_bytes();
        let mut early_terminated = false;

        // Loaded latency background task (if enabled)
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let loaded_samples: Arc<tokio::sync::Mutex<Vec<f64>>> =
            Arc::new(tokio::sync::Mutex::new(Vec::new()));

        let loaded_handle = if config.measure_loaded_latency {
            let client_clone = client.clone();
            let samples_clone = loaded_samples.clone();
            let mut rx = cancel_rx.clone();
            Some(tokio::spawn(async move {
                loop {
                    tokio::select! {
                        _ = rx.changed() => break,
                        _ = tokio::time::sleep(std::time::Duration::from_millis(400)) => {}
                    }
                    if *rx.borrow() {
                        break;
                    }
                    let start = Instant::now();
                    if let Ok(resp) = client_clone
                        .get(format!("{CLOUDFLARE_BASE}/__down?bytes=0&during=download"))
                        .send()
                        .await
                    {
                        let _ = resp.bytes().await;
                        let latency = start.elapsed().as_secs_f64() * 1000.0;
                        samples_clone.lock().await.push(latency);
                    }
                }
            }))
        } else {
            None
        };

        // Iterate through progressive steps
        for step in &config.download_steps {
            if early_terminated {
                break;
            }

            let size_label = format_size_label(step.payload_bytes);

            for _req_idx in 0..step.request_count {
                let url = format!("{CLOUDFLARE_BASE}/__down?bytes={}", step.payload_bytes);
                let start = Instant::now();

                let resp = client
                    .get(&url)
                    .send()
                    .await
                    .map_err(|e| format!("Download request failed: {e}"))?;

                if !resp.status().is_success() {
                    let status = resp.status().as_u16();
                    tracing::warn!(
                        test_id = %test_id,
                        status,
                        payload_bytes = step.payload_bytes,
                        "Download got non-200, treating as early termination"
                    );
                    // If we have data points, finish gracefully instead of failing
                    if !all_points.is_empty() {
                        early_terminated = true;
                        break;
                    }
                    return Err(format!("Download got HTTP {status}"));
                }

                // Parse metadata from first successful response
                if metadata.is_none() && config.collect_metadata {
                    metadata = Some(parse_cf_metadata(resp.headers()));
                }

                // Capture colo for server string from first response
                if all_points.is_empty() {
                    if let Some(colo) = resp
                        .headers()
                        .get("cf-meta-colo")
                        .or_else(|| resp.headers().get("colo"))
                        .and_then(|v| v.to_str().ok())
                    {
                        server = format!("Cloudflare {}", colo_to_city(colo));
                    }
                }

                // Parse TCP stats if enabled
                if config.parse_tcp_stats {
                    if let Some(stats) = parse_server_timing_tcp(resp.headers()) {
                        last_tcp_stats = Some(stats);
                    }
                }

                let server_time_ms = parse_server_time_ms(resp.headers());

                // Consume body
                let body = resp
                    .bytes()
                    .await
                    .map_err(|e| format!("Download read failed: {e}"))?;

                let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
                let transfer_ms = (elapsed_ms - server_time_ms).max(0.1);
                let bytes_received = body.len() as u64;
                total_bytes += bytes_received;

                // Check early termination
                if let Some(threshold) = config.early_termination_ms {
                    if elapsed_ms > threshold && step.payload_bytes > 1_000_000 {
                        early_terminated = true;
                    }
                }

                // Min duration filter
                if transfer_ms >= config.min_request_duration_ms {
                    let bps = bytes_received as f64 * 8.0 / (transfer_ms / 1000.0);
                    all_points.push(BandwidthPoint {
                        size_label: size_label.clone(),
                        bytes: bytes_received,
                        duration_ms: transfer_ms,
                        bps,
                    });
                }

                // Calculate running p90 and send progress
                let bps_values: Vec<f64> = all_points.iter().map(|p| p.bps).collect();
                let running_p90 = if bps_values.is_empty() {
                    0.0
                } else {
                    percentile(&bps_values, 90.0)
                };
                let running_p90_mbps = running_p90 / 1_000_000.0;

                let pct = ((total_bytes as f64 / total_expected as f64) * 100.0).min(99.0) as u8;
                let _ = progress_tx.send(SpeedtestProgress {
                    test_id: test_id.to_string(),
                    phase: SpeedtestPhase::Download,
                    progress_pct: pct,
                    current_speed_mbps: running_p90_mbps,
                    bytes_transferred: total_bytes,
                    running_p90_mbps: Some(running_p90_mbps),
                    size_label: Some(size_label.clone()),
                });

                if early_terminated {
                    break;
                }
            }
        }

        // Stop loaded latency probes
        let _ = cancel_tx.send(true);
        if let Some(handle) = loaded_handle {
            let _ = handle.await;
        }

        // Final p90
        let bps_values: Vec<f64> = all_points.iter().map(|p| p.bps).collect();
        let p90_bps = if bps_values.is_empty() {
            0.0
        } else {
            percentile(&bps_values, 90.0)
        };
        let p90_mbps = p90_bps / 1_000_000.0;

        // Final 100% progress
        let _ = progress_tx.send(SpeedtestProgress {
            test_id: test_id.to_string(),
            phase: SpeedtestPhase::Download,
            progress_pct: 100,
            current_speed_mbps: p90_mbps,
            bytes_transferred: total_bytes,
            running_p90_mbps: Some(p90_mbps),
            size_label: None,
        });

        let loaded_latency_samples = loaded_samples.lock().await.clone();

        Ok(PhaseResult {
            p90_mbps,
            total_bytes,
            points: all_points,
            server,
            metadata,
            tcp_stats: last_tcp_stats,
            loaded_latency_samples,
        })
    }

    /// Measure upload speed using progressive payload sizes.
    async fn measure_upload_progressive(
        client: &reqwest::Client,
        config: &TestConfig,
        test_id: &str,
        progress_tx: &broadcast::Sender<SpeedtestProgress>,
    ) -> Result<PhaseResult, String> {
        let mut all_points: Vec<BandwidthPoint> = Vec::new();
        let mut total_bytes: u64 = 0;
        let mut last_tcp_stats: Option<TcpStats> = None;
        let total_expected = config.total_upload_bytes();
        let mut early_terminated = false;

        // Upload chunk size limit: 2MB per POST for smooth progress (v1.0.135 lesson)
        const UPLOAD_CHUNK_LIMIT: u64 = 2 * 1024 * 1024;

        // Loaded latency background task (if enabled)
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let loaded_samples: Arc<tokio::sync::Mutex<Vec<f64>>> =
            Arc::new(tokio::sync::Mutex::new(Vec::new()));

        let loaded_handle = if config.measure_loaded_latency {
            let client_clone = client.clone();
            let samples_clone = loaded_samples.clone();
            let mut rx = cancel_rx.clone();
            Some(tokio::spawn(async move {
                loop {
                    tokio::select! {
                        _ = rx.changed() => break,
                        _ = tokio::time::sleep(std::time::Duration::from_millis(400)) => {}
                    }
                    if *rx.borrow() {
                        break;
                    }
                    let start = Instant::now();
                    if let Ok(resp) = client_clone
                        .get(format!("{CLOUDFLARE_BASE}/__down?bytes=0&during=upload"))
                        .send()
                        .await
                    {
                        let _ = resp.bytes().await;
                        let latency = start.elapsed().as_secs_f64() * 1000.0;
                        samples_clone.lock().await.push(latency);
                    }
                }
            }))
        } else {
            None
        };

        let url = format!("{CLOUDFLARE_BASE}/__up");

        for step in &config.upload_steps {
            if early_terminated {
                break;
            }

            let size_label = format_size_label(step.payload_bytes);

            for _req_idx in 0..step.request_count {
                // For payloads > 2MB, split into chunk-sized POSTs and aggregate
                let chunk_count = if step.payload_bytes > UPLOAD_CHUNK_LIMIT {
                    ((step.payload_bytes + UPLOAD_CHUNK_LIMIT - 1) / UPLOAD_CHUNK_LIMIT) as usize
                } else {
                    1
                };

                let chunk_size = if chunk_count > 1 {
                    UPLOAD_CHUNK_LIMIT as usize
                } else {
                    step.payload_bytes as usize
                };

                let start = Instant::now();
                let mut step_bytes: u64 = 0;
                let payload = vec![0u8; chunk_size];

                let mut chunk_failed = false;
                for _ in 0..chunk_count {
                    let resp = client
                        .post(&url)
                        .body(payload.clone())
                        .send()
                        .await
                        .map_err(|e| format!("Upload request failed: {e}"))?;

                    if !resp.status().is_success() {
                        let status = resp.status().as_u16();
                        tracing::warn!(
                            test_id = %test_id,
                            status,
                            payload_bytes = step.payload_bytes,
                            "Upload got non-200, treating as early termination"
                        );
                        if !all_points.is_empty() {
                            early_terminated = true;
                            chunk_failed = true;
                            break;
                        }
                        return Err(format!("Upload got HTTP {status}"));
                    }

                    // Parse TCP stats if enabled
                    if config.parse_tcp_stats {
                        if let Some(stats) = parse_server_timing_tcp(resp.headers()) {
                            last_tcp_stats = Some(stats);
                        }
                    }

                    step_bytes += chunk_size as u64;
                }

                if chunk_failed {
                    break;
                }

                let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
                let server_time_ms = 10.0; // No Server-Timing dur from upload responses
                let transfer_ms = (elapsed_ms - server_time_ms * chunk_count as f64).max(0.1);
                total_bytes += step_bytes;

                // Check early termination
                if let Some(threshold) = config.early_termination_ms {
                    if elapsed_ms > threshold && step.payload_bytes > 1_000_000 {
                        early_terminated = true;
                    }
                }

                // Min duration filter
                if transfer_ms >= config.min_request_duration_ms {
                    let bps = step_bytes as f64 * 8.0 / (transfer_ms / 1000.0);
                    all_points.push(BandwidthPoint {
                        size_label: size_label.clone(),
                        bytes: step_bytes,
                        duration_ms: transfer_ms,
                        bps,
                    });
                }

                // Calculate running p90 and send progress
                let bps_values: Vec<f64> = all_points.iter().map(|p| p.bps).collect();
                let running_p90 = if bps_values.is_empty() {
                    0.0
                } else {
                    percentile(&bps_values, 90.0)
                };
                let running_p90_mbps = running_p90 / 1_000_000.0;

                let pct = ((total_bytes as f64 / total_expected as f64) * 100.0).min(99.0) as u8;
                let _ = progress_tx.send(SpeedtestProgress {
                    test_id: test_id.to_string(),
                    phase: SpeedtestPhase::Upload,
                    progress_pct: pct,
                    current_speed_mbps: running_p90_mbps,
                    bytes_transferred: total_bytes,
                    running_p90_mbps: Some(running_p90_mbps),
                    size_label: Some(size_label.clone()),
                });

                if early_terminated {
                    break;
                }
            }
        }

        // Stop loaded latency probes
        let _ = cancel_tx.send(true);
        if let Some(handle) = loaded_handle {
            let _ = handle.await;
        }

        // Final p90
        let bps_values: Vec<f64> = all_points.iter().map(|p| p.bps).collect();
        let p90_bps = if bps_values.is_empty() {
            0.0
        } else {
            percentile(&bps_values, 90.0)
        };
        let p90_mbps = p90_bps / 1_000_000.0;

        // Final 100% progress
        let _ = progress_tx.send(SpeedtestProgress {
            test_id: test_id.to_string(),
            phase: SpeedtestPhase::Upload,
            progress_pct: 100,
            current_speed_mbps: p90_mbps,
            bytes_transferred: total_bytes,
            running_p90_mbps: Some(p90_mbps),
            size_label: None,
        });

        let loaded_latency_samples = loaded_samples.lock().await.clone();

        Ok(PhaseResult {
            p90_mbps,
            total_bytes,
            points: all_points,
            server: String::new(), // Not captured from upload
            metadata: None,
            tcp_stats: last_tcp_stats,
            loaded_latency_samples,
        })
    }

    // ========================================================================
    // Main orchestrator
    // ========================================================================

    /// Run a full speedtest: warmup -> latency -> download -> upload -> scoring.
    ///
    /// Progress updates are sent via `progress_tx`. Returns the final result.
    pub async fn run_speedtest(
        interface: &str,
        mode: crate::hardware::types::SpeedtestMode,
        wan_id: &str,
        wan_name: &str,
        progress_tx: broadcast::Sender<SpeedtestProgress>,
    ) -> Result<crate::hardware::types::SpeedtestResult, String> {
        let test_id = uuid::Uuid::new_v4().to_string();
        let config = TestConfig::for_mode(mode);

        tracing::info!(
            test_id = %test_id,
            interface = %interface,
            mode = ?mode,
            latency_probes = config.latency_probes,
            download_steps = config.download_steps.len(),
            upload_steps = config.upload_steps.len(),
            "Starting progressive speedtest"
        );

        let client = build_client(interface)?;

        // Phase 0: Warmup (Full mode only)
        if config.warmup {
            tracing::debug!(test_id = %test_id, "Running TLS warmup");
            run_warmup(&client).await?;
        }

        // Phase 1: Latency
        let (latency_ms, jitter_ms, _latency_samples) =
            measure_latency(&client, &config, &test_id, &progress_tx).await?;
        tracing::info!(test_id = %test_id, latency_ms, jitter_ms, "Latency measurement complete");

        // Phase 2: Download
        let dl_result =
            measure_download_progressive(&client, &config, &test_id, &progress_tx).await?;
        tracing::info!(
            test_id = %test_id,
            download_mbps = dl_result.p90_mbps,
            download_bytes = dl_result.total_bytes,
            points = dl_result.points.len(),
            "Download measurement complete"
        );

        // Phase 3: Upload
        let ul_result =
            measure_upload_progressive(&client, &config, &test_id, &progress_tx).await?;
        tracing::info!(
            test_id = %test_id,
            upload_mbps = ul_result.p90_mbps,
            upload_bytes = ul_result.total_bytes,
            points = ul_result.points.len(),
            "Upload measurement complete"
        );

        // Post-processing: loaded latency
        let dl_loaded_latency_ms = if !dl_result.loaded_latency_samples.is_empty() {
            Some(percentile(&dl_result.loaded_latency_samples, 50.0))
        } else {
            None
        };
        let dl_loaded_jitter_ms = if dl_result.loaded_latency_samples.len() >= 2 {
            Some(calculate_jitter(&dl_result.loaded_latency_samples))
        } else {
            None
        };
        let ul_loaded_latency_ms = if !ul_result.loaded_latency_samples.is_empty() {
            Some(percentile(&ul_result.loaded_latency_samples, 50.0))
        } else {
            None
        };
        let ul_loaded_jitter_ms = if ul_result.loaded_latency_samples.len() >= 2 {
            Some(calculate_jitter(&ul_result.loaded_latency_samples))
        } else {
            None
        };

        // Bufferbloat = max(dl_loaded, ul_loaded) - unloaded
        let bufferbloat_ms = match (dl_loaded_latency_ms, ul_loaded_latency_ms) {
            (Some(dl), Some(ul)) => Some(dl.max(ul) - latency_ms),
            (Some(dl), None) => Some(dl - latency_ms),
            (None, Some(ul)) => Some(ul - latency_ms),
            (None, None) => None,
        }
        .map(|v| v.max(0.0));

        // TCP loss ratio from last tcp stats
        let tcp_loss_ratio = dl_result
            .tcp_stats
            .as_ref()
            .or(ul_result.tcp_stats.as_ref())
            .and_then(|stats| {
                let total = stats.lost + stats.retrans;
                if total > 0 {
                    // Approximate: lost packets / (lost + delivered)
                    // delivery_rate_bps is not a packet count, so use lost/retrans ratio
                    Some(stats.lost as f64 / (stats.lost + stats.retrans + stats.cwnd) as f64)
                } else {
                    Some(0.0)
                }
            });

        // Connection metadata with colo resolved to city
        let connection = dl_result.metadata.map(|mut meta| {
            if let Some(ref colo) = meta.colo {
                let city = colo_to_city(colo);
                if city != colo.as_str() {
                    meta.city = Some(city.to_string());
                }
            }
            meta
        });

        // AIM scores
        let scores = if config.calculate_aim {
            let loaded_increase = match (dl_loaded_latency_ms, ul_loaded_latency_ms) {
                (Some(dl), Some(ul)) => dl.max(ul) - latency_ms,
                (Some(dl), None) => dl - latency_ms,
                (None, Some(ul)) => ul - latency_ms,
                (None, None) => 0.0,
            }
            .max(0.0);

            let loss = tcp_loss_ratio.unwrap_or(0.0);
            let dl_bps = dl_result.p90_mbps * 1_000_000.0;
            let ul_bps = ul_result.p90_mbps * 1_000_000.0;

            Some(calculate_aim_scores(
                latency_ms,
                jitter_ms,
                loss,
                dl_bps,
                ul_bps,
                loaded_increase,
            ))
        } else {
            None
        };

        // Measurement breakdowns
        let download_measurements = if config.collect_breakdown {
            Some(build_breakdown(&dl_result.points))
        } else {
            None
        };
        let upload_measurements = if config.collect_breakdown {
            Some(build_breakdown(&ul_result.points))
        } else {
            None
        };

        let result = crate::hardware::types::SpeedtestResult {
            id: test_id,
            timestamp: chrono::Utc::now().to_rfc3339(),
            mode,
            wan_id: wan_id.to_string(),
            wan_name: wan_name.to_string(),
            interface: interface.to_string(),
            download_mbps: dl_result.p90_mbps,
            upload_mbps: ul_result.p90_mbps,
            latency_ms,
            jitter_ms,
            bytes_consumed: dl_result.total_bytes + ul_result.total_bytes,
            server: dl_result.server,
            download_loaded_latency_ms: dl_loaded_latency_ms,
            download_loaded_jitter_ms: dl_loaded_jitter_ms,
            upload_loaded_latency_ms: ul_loaded_latency_ms,
            upload_loaded_jitter_ms: ul_loaded_jitter_ms,
            bufferbloat_ms,
            connection,
            scores,
            tcp_loss_ratio,
            download_measurements,
            upload_measurements,
        };

        Ok(result)
    }
}

#[cfg(feature = "tunnel")]
pub use engine::*;

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_history_ring_buffer() {
        let mut history = SpeedtestHistory::new();
        assert_eq!(history.results.len(), 0);

        // Fill beyond capacity
        for i in 0..55 {
            history.push(SpeedtestResult {
                id: format!("test-{i}"),
                timestamp: "2026-04-07T00:00:00Z".to_string(),
                mode: SpeedtestMode::Quick,
                wan_id: "wan1".to_string(),
                wan_name: "WAN 1".to_string(),
                interface: "wwan0".to_string(),
                download_mbps: 100.0,
                upload_mbps: 50.0,
                latency_ms: 20.0,
                jitter_ms: 2.0,
                bytes_consumed: 1000,
                server: "speed.cloudflare.com".to_string(),
                download_loaded_latency_ms: None,
                download_loaded_jitter_ms: None,
                upload_loaded_latency_ms: None,
                upload_loaded_jitter_ms: None,
                bufferbloat_ms: None,
                connection: None,
                scores: None,
                tcp_loss_ratio: None,
                download_measurements: None,
                upload_measurements: None,
            });
        }

        assert_eq!(history.results.len(), MAX_HISTORY);
        // Oldest should be test-5 (first 5 evicted)
        assert_eq!(history.results.front().unwrap().id, "test-5");
        assert_eq!(history.results.back().unwrap().id, "test-54");
    }

    #[test]
    fn test_history_persistence_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.json");

        let mut history = SpeedtestHistory::new();
        history.push(SpeedtestResult {
            id: "abc-123".to_string(),
            timestamp: "2026-04-07T12:00:00Z".to_string(),
            mode: SpeedtestMode::Full,
            wan_id: "2c7c:0122:abc".to_string(),
            wan_name: "Quectel RM551E".to_string(),
            interface: "wwan0".to_string(),
            download_mbps: 150.5,
            upload_mbps: 45.2,
            latency_ms: 18.3,
            jitter_ms: 1.5,
            bytes_consumed: 150_000_000,
            server: "speed.cloudflare.com".to_string(),
            download_loaded_latency_ms: None,
            download_loaded_jitter_ms: None,
            upload_loaded_latency_ms: None,
            upload_loaded_jitter_ms: None,
            bufferbloat_ms: None,
            connection: None,
            scores: None,
            tcp_loss_ratio: None,
            download_measurements: None,
            upload_measurements: None,
        });

        save_history_to(&history, &path).unwrap();
        let loaded = load_history_from(&path);

        assert_eq!(loaded.results.len(), 1);
        assert_eq!(loaded.results[0].id, "abc-123");
        assert!((loaded.results[0].download_mbps - 150.5).abs() < f64::EPSILON);
    }

    #[test]
    fn test_load_missing_file() {
        let history = load_history_from(Path::new("/tmp/nonexistent-speedtest-history.json"));
        assert_eq!(history.results.len(), 0);
    }

    #[test]
    fn test_config_quick_vs_full() {
        let quick = TestConfig::quick();
        let full = TestConfig::full();

        assert_eq!(quick.latency_probes, 5);
        assert_eq!(full.latency_probes, 20);
        assert!(full.download_steps.len() > quick.download_steps.len());
        assert!(full.upload_steps.len() > quick.upload_steps.len());
        assert!(full.warmup);
        assert!(!quick.warmup);
        assert!(full.measure_loaded_latency);
        assert!(!quick.measure_loaded_latency);
    }

    #[test]
    fn test_config_medium() {
        let medium = TestConfig::medium();
        assert_eq!(medium.latency_probes, 10);
        assert_eq!(medium.download_steps.len(), 3);
        assert_eq!(medium.upload_steps.len(), 3);
        assert!(medium.collect_metadata);
        assert!(medium.calculate_aim);
        assert!(medium.measure_loaded_latency);
        assert!(!medium.parse_tcp_stats);
    }

    #[test]
    fn test_config_payload_sizes_are_safe() {
        // Verify no payload falls in Cloudflare forbidden ranges:
        // 11,000,000-19,999,999 or >= 100,000,000
        for config in [TestConfig::quick(), TestConfig::medium(), TestConfig::full()] {
            for step in config.download_steps.iter().chain(config.upload_steps.iter()) {
                assert!(
                    step.payload_bytes < 11_000_000 || step.payload_bytes >= 20_000_000,
                    "Payload {} is in forbidden 11M-20M range",
                    step.payload_bytes
                );
                assert!(
                    step.payload_bytes < 100_000_000,
                    "Payload {} is >= 100MB forbidden range",
                    step.payload_bytes
                );
            }
        }
    }

    #[test]
    fn test_config_total_bytes() {
        let quick = TestConfig::quick();
        // Quick: DL = 100K*3 + 1M*3 = 3.3MB, UL = 100K*3 + 1M*2 = 2.3MB
        assert_eq!(quick.total_download_bytes(), 3_300_000);
        assert_eq!(quick.total_upload_bytes(), 2_300_000);

        let medium = TestConfig::medium();
        // Medium: DL = 100K*5 + 1M*5 + 10M*3 = 35.5MB
        assert_eq!(medium.total_download_bytes(), 35_500_000);
    }

    #[test]
    fn test_speedtest_types_serde() {
        use crate::hardware::types::{SpeedtestPhase, SpeedtestProgress};

        let progress = SpeedtestProgress {
            test_id: "test-1".to_string(),
            phase: SpeedtestPhase::Download,
            progress_pct: 50,
            current_speed_mbps: 95.5,
            bytes_transferred: 5_000_000,
            running_p90_mbps: Some(92.3),
            size_label: Some("10MB".to_string()),
        };

        let json = serde_json::to_string(&progress).unwrap();
        assert!(json.contains("\"download\""));
        assert!(json.contains("\"progress_pct\":50"));
        assert!(json.contains("\"running_p90_mbps\":92.3"));
        assert!(json.contains("\"size_label\":\"10MB\""));

        let result = SpeedtestResult {
            id: "test-1".to_string(),
            timestamp: "2026-04-07T00:00:00Z".to_string(),
            mode: SpeedtestMode::Quick,
            wan_id: "wan1".to_string(),
            wan_name: "WAN 1".to_string(),
            interface: "wwan0".to_string(),
            download_mbps: 100.0,
            upload_mbps: 50.0,
            latency_ms: 20.0,
            jitter_ms: 2.0,
            bytes_consumed: 15_000_000,
            server: "speed.cloudflare.com".to_string(),
            download_loaded_latency_ms: None,
            download_loaded_jitter_ms: None,
            upload_loaded_latency_ms: None,
            upload_loaded_jitter_ms: None,
            bufferbloat_ms: None,
            connection: None,
            scores: None,
            tcp_loss_ratio: None,
            download_measurements: None,
            upload_measurements: None,
        };

        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"quick\""));
        assert!(json.contains("\"download_mbps\":100.0"));
    }
}
