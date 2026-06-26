//! BH-08 persisted reboot-ledger: anti-boot-loop record of guarded auto-reboots.
use serde::{Deserialize, Serialize};

pub const WINDOW_SECS: i64 = 86_400;
const STATE_FILE: &str = "/etc/modem-interface/wedge-reboot-state.json";

fn state_path() -> String {
    std::env::var("WEDGE_REBOOT_STATE_PATH").unwrap_or_else(|_| STATE_FILE.to_string())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RebootEntry {
    pub ts: i64,
    pub modem_id: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RebootLedger {
    #[serde(default)]
    pub entries: Vec<RebootEntry>,
}

impl RebootLedger {
    pub fn count_since(&self, now_unix: i64, window_secs: i64) -> usize {
        self.entries
            .iter()
            .filter(|e| now_unix - e.ts <= window_secs)
            .count()
    }

    pub fn pruned(mut self, now_unix: i64, window_secs: i64) -> Self {
        self.entries.retain(|e| now_unix - e.ts <= window_secs);
        self
    }

    pub fn with_recorded(mut self, now_unix: i64, modem_id: &str, reason: &str) -> Self {
        self.entries.push(RebootEntry {
            ts: now_unix,
            modem_id: modem_id.to_string(),
            reason: reason.to_string(),
        });
        self
    }
}

/// Ok(empty) when absent; Err when present-but-corrupt (caller must suppress reboot).
pub async fn load_ledger() -> Result<RebootLedger, String> {
    let path = state_path();
    match tokio::fs::read_to_string(&path).await {
        Ok(content) => serde_json::from_str(&content)
            .map_err(|e| format!("corrupt wedge-reboot ledger: {e}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(RebootLedger::default()),
        Err(e) => Err(format!("unreadable wedge-reboot ledger: {e}")),
    }
}

pub async fn save_ledger(l: &RebootLedger) -> Result<(), String> {
    let path = state_path();

    if let Some(parent) = std::path::Path::new(&path).parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("create ledger dir: {e}"))?;
    }

    let json = serde_json::to_string_pretty(l).map_err(|e| format!("serialize ledger: {e}"))?;
    crate::config::write_secret_file(&path, json)
        .await
        .map_err(|e| format!("write ledger: {e}"))
}

#[cfg(test)]
mod ledger_tests {
    use super::*;

    fn e(ts: i64) -> RebootEntry {
        RebootEntry {
            ts,
            modem_id: "m".into(),
            reason: "wedge".into(),
        }
    }

    #[test]
    fn count_since_only_counts_in_window() {
        let now = 1_000_000;
        let l = RebootLedger {
            entries: vec![e(now - 10), e(now - 100_000), e(now - 50)],
        };
        assert_eq!(l.count_since(now, WINDOW_SECS), 2); // -10 and -50 are within 86400; -100000 is not
    }

    #[test]
    fn pruned_drops_old_entries() {
        let now = 1_000_000;
        let l = RebootLedger {
            entries: vec![e(now - 10), e(now - 100_000)],
        };
        assert_eq!(l.pruned(now, WINDOW_SECS).entries.len(), 1);
    }

    #[test]
    fn with_recorded_appends() {
        let now = 1_000_000;
        let l = RebootLedger { entries: vec![] }.with_recorded(now, "2c7c:0801:x", "wedge");
        assert_eq!(l.entries.len(), 1);
        assert_eq!(l.entries[0].ts, now);
    }
}
