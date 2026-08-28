//! On-disk application configuration.
//!
//! Everything here is safe to write to a plain file: Connection secrets live in
//! the OS keyring (see [`crate::secrets`]), never in this struct.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// The whole persisted config document.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub connections: Vec<Connection>,
    /// Render Hit timestamps in UTC rather than local time.
    #[serde(default)]
    pub utc_timestamps: bool,
}

/// A named Elasticsearch endpoint plus how to reach it. Secretless.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Connection {
    /// Stable identity, independent of `name`. Also the keyring account key.
    pub id: String,
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub auth: Auth,
    #[serde(default)]
    pub skip_tls_verify: bool,
    #[serde(default)]
    pub searches: Vec<SavedSearch>,
}

/// A persisted, named query belonging to one Connection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedSearch {
    pub id: String,
    pub name: String,
    /// Index, data stream, or pattern (e.g. `logs-*`).
    pub target: String,
    /// Lucene syntax, passed straight to `query_string`. Empty matches all.
    pub query_string: String,
    pub timeframe: Timeframe,
    /// Timestamp field the Timeframe filters on.
    #[serde(default = "default_timestamp_field")]
    pub timestamp_field: String,
    /// Fields projected into table columns, in display order.
    #[serde(default = "default_columns")]
    pub columns: Vec<String>,
    /// Field the Hits are sorted on (`_shard_doc` is always appended after it).
    #[serde(default = "default_timestamp_field")]
    pub sort_field: String,
    /// Sort direction: descending when true.
    #[serde(default = "default_true")]
    pub sort_desc: bool,
}

fn default_true() -> bool {
    true
}

/// The time window a Saved Search restricts Hits to.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Timeframe {
    /// e.g. the last 15 minutes. Re-anchors to "now" on every run.
    Relative { amount: u64, unit: TimeUnit },
    /// A frozen start/end, as Elasticsearch date-math / ISO strings.
    Absolute { from: String, to: String },
}

impl Default for Timeframe {
    fn default() -> Self {
        Timeframe::Relative {
            amount: 15,
            unit: TimeUnit::Minutes,
        }
    }
}

impl Timeframe {
    /// The `gte` / `lte` range bounds this Timeframe resolves to right now.
    /// Relative frames yield Elasticsearch date-math (`now-15m` .. `now`), so
    /// the cluster re-anchors them on every run.
    pub fn bounds(&self) -> (String, String) {
        match self {
            Timeframe::Relative { amount, unit } => {
                (format!("now-{amount}{}", unit.suffix()), "now".to_string())
            }
            Timeframe::Absolute { from, to } => (from.clone(), to.clone()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeUnit {
    Minutes,
    Hours,
    Days,
}

impl TimeUnit {
    pub const ALL: [TimeUnit; 3] = [TimeUnit::Minutes, TimeUnit::Hours, TimeUnit::Days];

    /// The Elasticsearch date-math suffix (`now-15m`).
    pub fn suffix(self) -> char {
        match self {
            TimeUnit::Minutes => 'm',
            TimeUnit::Hours => 'h',
            TimeUnit::Days => 'd',
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            TimeUnit::Minutes => "minutes",
            TimeUnit::Hours => "hours",
            TimeUnit::Days => "days",
        }
    }
}

pub fn default_timestamp_field() -> String {
    "@timestamp".to_string()
}

pub fn default_columns() -> Vec<String> {
    vec!["@timestamp".to_string(), "message".to_string()]
}

/// The auth scheme for a Connection. Carries no secret material.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Auth {
    #[default]
    None,
    Basic {
        username: String,
    },
    ApiKey,
}

/// A fresh, process-unique Connection id.
pub fn new_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("conn-{nanos:x}")
}

/// `~/.config/loglens/config.json` (or the platform equivalent).
pub fn config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("loglens").join("config.json"))
}

/// Reads the config, returning defaults if it is missing or unreadable.
pub fn load() -> Config {
    let Some(path) = config_path() else {
        return Config::default();
    };
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Writes the config, creating the parent directory as needed.
pub fn save(config: &Config) -> Result<(), String> {
    let path = config_path().ok_or("no platform config directory")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let body = serde_json::to_string_pretty(config).map_err(|e| e.to_string())?;
    std::fs::write(&path, body).map_err(|e| e.to_string())
}
