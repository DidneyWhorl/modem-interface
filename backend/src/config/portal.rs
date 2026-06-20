//! `[portal]` config section. Derives URLs for heartbeat, telemetry,
//! license activation, and tunnel from a single base URL.

use serde::{Deserialize, Serialize};

/// `[portal]` section of `config.toml`.
///
/// All outbound portal traffic (heartbeat, telemetry config poll, license
/// activation, tunnel WSS) derives from this section. Switching
/// `base_url` redirects every portal call in one step.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PortalConfig {
    /// Base URL for all portal HTTPS calls. No trailing slash required;
    /// one is tolerated and trimmed.
    #[serde(default = "default_base_url")]
    pub base_url: String,

    /// Explicit tunnel WSS endpoint. If `None`, derive from `base_url`
    /// by swapping scheme to `wss://` and appending `/api/v1/tunnel`.
    #[serde(default)]
    pub tunnel_url: Option<String>,
}

fn default_base_url() -> String {
    "https://portal.ctrl-modem.com".to_string()
}

impl Default for PortalConfig {
    fn default() -> Self {
        Self {
            base_url: default_base_url(),
            tunnel_url: Some("wss://portal.ctrl-modem.com/api/v1/tunnel".to_string()),
        }
    }
}

// Accessors are wired into callers in Tasks 2.3–2.6 (heartbeat, telemetry,
// licensing, tunnel). Until then they compile as dead code in this binary
// crate; tests exercise them but dead-code lint only considers non-test
// call sites.
#[allow(dead_code)]
impl PortalConfig {
    fn base_trimmed(&self) -> &str {
        self.base_url.trim_end_matches('/')
    }

    pub fn heartbeat_url(&self) -> String {
        format!("{}/api/v1/heartbeat", self.base_trimmed())
    }

    pub fn poll_config_url(&self) -> String {
        format!("{}/api/v1/device/poll-config", self.base_trimmed())
    }

    pub fn license_activate_url(&self) -> String {
        format!("{}/api/v1/license/activate", self.base_trimmed())
    }

    /// Explicit `tunnel_url` wins; otherwise derive from `base_url`.
    pub fn resolved_tunnel_url(&self) -> String {
        if let Some(url) = &self.tunnel_url {
            return url.clone();
        }
        let base = self.base_trimmed();
        let host_path = base
            .strip_prefix("https://")
            .or_else(|| base.strip_prefix("http://"))
            .unwrap_or(base);
        format!("wss://{host_path}/api/v1/tunnel")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_portal_points_at_production() {
        let c = PortalConfig::default();
        assert_eq!(c.base_url, "https://portal.ctrl-modem.com");
        assert_eq!(
            c.tunnel_url.as_deref(),
            Some("wss://portal.ctrl-modem.com/api/v1/tunnel")
        );
    }

    #[test]
    fn derived_urls_are_built_from_base() {
        let c = PortalConfig {
            base_url: "https://staging.ctrl-modem.com".into(),
            tunnel_url: None,
        };
        assert_eq!(c.heartbeat_url(), "https://staging.ctrl-modem.com/api/v1/heartbeat");
        assert_eq!(c.poll_config_url(), "https://staging.ctrl-modem.com/api/v1/device/poll-config");
        assert_eq!(c.license_activate_url(), "https://staging.ctrl-modem.com/api/v1/license/activate");
    }

    #[test]
    fn derived_tunnel_url_falls_back_to_base_wss() {
        let c = PortalConfig {
            base_url: "https://staging.ctrl-modem.com".into(),
            tunnel_url: None,
        };
        assert_eq!(
            c.resolved_tunnel_url(),
            "wss://staging.ctrl-modem.com/api/v1/tunnel"
        );
    }

    #[test]
    fn explicit_tunnel_url_wins_over_derivation() {
        let c = PortalConfig {
            base_url: "https://staging.ctrl-modem.com".into(),
            tunnel_url: Some("wss://tunnel-staging.ctrl-modem.com/api/v1/tunnel".into()),
        };
        assert_eq!(
            c.resolved_tunnel_url(),
            "wss://tunnel-staging.ctrl-modem.com/api/v1/tunnel"
        );
    }

    #[test]
    fn base_url_with_trailing_slash_is_normalized() {
        let c = PortalConfig {
            base_url: "https://staging.ctrl-modem.com/".into(),
            tunnel_url: None,
        };
        // Trailing slash must not produce "//api/v1/...".
        assert_eq!(c.heartbeat_url(), "https://staging.ctrl-modem.com/api/v1/heartbeat");
    }

    #[test]
    fn toml_round_trip_preserves_fields() {
        let c = PortalConfig {
            base_url: "https://staging.ctrl-modem.com".into(),
            tunnel_url: Some("wss://tunnel-staging.ctrl-modem.com/api/v1/tunnel".into()),
        };
        let s = toml::to_string(&c).unwrap();
        let back: PortalConfig = toml::from_str(&s).unwrap();
        assert_eq!(c.base_url, back.base_url);
        assert_eq!(c.tunnel_url, back.tunnel_url);
    }

    #[test]
    fn toml_missing_tunnel_url_parses_as_none() {
        let s = r#"base_url = "https://portal.ctrl-modem.com""#;
        let c: PortalConfig = toml::from_str(s).unwrap();
        assert!(c.tunnel_url.is_none());
    }
}
