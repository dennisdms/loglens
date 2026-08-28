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
