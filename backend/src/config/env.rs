//! Baked-in environment registry. Not user-extensible — adding a new env
//! requires a code change + release. Prevents misconfigured routers
//! pointing at arbitrary hostnames.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnvConfig {
    pub portal_base: &'static str,
    pub tunnel_url: &'static str,
    pub apk_feed: &'static str,
    pub opkg_feed: &'static str,
}

const ENVIRONMENTS: &[(&str, EnvConfig)] = &[
    (
        "production",
        EnvConfig {
            portal_base: "https://portal.ctrl-modem.com",
            tunnel_url: "wss://portal.ctrl-modem.com/api/v1/tunnel",
            apk_feed: "https://packages.ctrl-modem.com/stable/apk",
            opkg_feed: "https://packages.ctrl-modem.com/stable/feed",
        },
    ),
    (
        "staging",
        EnvConfig {
            portal_base: "https://staging.ctrl-modem.com",
            tunnel_url: "wss://tunnel-staging.ctrl-modem.com/api/v1/tunnel",
            apk_feed: "https://packages.ctrl-modem.com/testing/apk",
            opkg_feed: "https://packages.ctrl-modem.com/testing/feed",
        },
    ),
];

impl EnvConfig {
    pub fn lookup(name: &str) -> Option<EnvConfig> {
        ENVIRONMENTS.iter().find(|(n, _)| *n == name).map(|(_, e)| *e)
    }

    #[allow(dead_code)]
    pub fn all() -> &'static [(&'static str, EnvConfig)] {
        ENVIRONMENTS
    }

    /// Identify the env by its portal base URL (for `env show`).
    /// Returns `None` when the router is pointed at a URL not in the registry.
    pub fn from_portal_base(base: &str) -> Option<&'static str> {
        let normalized = base.trim_end_matches('/');
        ENVIRONMENTS
            .iter()
            .find(|(_, e)| e.portal_base == normalized)
            .map(|(n, _)| *n)
    }
}

/// Resolve the env name from a portal base URL, defaulting to "(custom)".
///
/// Used by every license verify call site to thread env into security/license.rs.
/// `(custom)` falls through to the production pubkey in `load_verifying_key`,
/// matching the pre-1b behavior for non-canonical portals.
pub fn current_env(base_url: &str) -> &'static str {
    EnvConfig::from_portal_base(base_url).unwrap_or("(custom)")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_env_exists_and_has_expected_urls() {
        let e = EnvConfig::lookup("production").expect("production must exist");
        assert_eq!(e.portal_base, "https://portal.ctrl-modem.com");
        assert_eq!(e.tunnel_url, "wss://portal.ctrl-modem.com/api/v1/tunnel");
        assert_eq!(e.apk_feed, "https://packages.ctrl-modem.com/stable/apk");
        assert_eq!(e.opkg_feed, "https://packages.ctrl-modem.com/stable/feed");
    }

    #[test]
    fn staging_env_points_at_staging_hostnames() {
        let e = EnvConfig::lookup("staging").expect("staging must exist");
        assert_eq!(e.portal_base, "https://staging.ctrl-modem.com");
        assert_eq!(e.tunnel_url, "wss://tunnel-staging.ctrl-modem.com/api/v1/tunnel");
        assert_eq!(e.apk_feed, "https://packages.ctrl-modem.com/testing/apk");
        assert_eq!(e.opkg_feed, "https://packages.ctrl-modem.com/testing/feed");
    }

    #[test]
    fn unknown_env_returns_none() {
        assert!(EnvConfig::lookup("prod").is_none());
        assert!(EnvConfig::lookup("PRODUCTION").is_none());
        assert!(EnvConfig::lookup("dev").is_none());
        assert!(EnvConfig::lookup("").is_none());
    }

    #[test]
    fn all_envs_enumerable() {
        let names: Vec<&str> = EnvConfig::all().iter().map(|(n, _)| *n).collect();
        assert_eq!(names, vec!["production", "staging"]);
    }

    #[test]
    fn current_env_detects_from_portal_base_url() {
        assert_eq!(EnvConfig::from_portal_base("https://portal.ctrl-modem.com"), Some("production"));
        assert_eq!(EnvConfig::from_portal_base("https://staging.ctrl-modem.com"), Some("staging"));
        assert_eq!(EnvConfig::from_portal_base("https://portal.ctrl-modem.com/"), Some("production"));
        assert_eq!(EnvConfig::from_portal_base("https://example.com"), None);
    }

    #[test]
    fn current_env_resolves_known_envs_to_their_names() {
        assert_eq!(current_env("https://portal.ctrl-modem.com"), "production");
        assert_eq!(current_env("https://staging.ctrl-modem.com"), "staging");
    }

    #[test]
    fn current_env_normalizes_trailing_slash() {
        assert_eq!(current_env("https://portal.ctrl-modem.com/"), "production");
        assert_eq!(current_env("https://staging.ctrl-modem.com/"), "staging");
    }

    #[test]
    fn current_env_falls_back_to_custom_for_unknown_url() {
        assert_eq!(current_env("https://portal.example.com"), "(custom)");
        assert_eq!(current_env(""), "(custom)");
    }
}
