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
    /// Sort order, highest priority first (`_shard_doc` is always appended as a
    /// tiebreaker). Empty falls back to the timestamp field, descending. Legacy
    /// `sort_field` / `sort_desc` keys are migrated in on load (see [`load`]).
    #[serde(default)]
    pub sort: Vec<SortKey>,
}

/// One field in a Saved Search's sort order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SortKey {
    pub field: String,
    /// Descending when true.
    #[serde(default = "default_true")]
    pub desc: bool,
}

impl SortKey {
    pub fn new(field: impl Into<String>, desc: bool) -> Self {
        Self {
            field: field.into(),
            desc,
        }
    }
}

fn default_true() -> bool {
    true
}

/// The time window a Saved Search restricts Hits to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Timeframe {
    /// e.g. the last 15 minutes. Re-anchors to "now" on every run.
    Relative { amount: u64, unit: TimeUnit },
    /// A frozen start/end, as Elasticsearch date-math / ISO strings.
    Absolute { from: String, to: String },
}

/// Which Timeframe kind a timeframe editor's toggle has selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeframeMode {
    Relative,
    Absolute,
}

/// A Search bar timeframe quick-pick: one of a fixed set of relative presets, or
/// `Custom` (which opens the popover for a bespoke relative / absolute window).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeframeChoice {
    Preset { amount: u64, unit: TimeUnit },
    Custom,
}

impl TimeframeChoice {
    /// Every entry the Search bar's timeframe dropdown offers, in order.
    pub const ALL: [TimeframeChoice; 7] = [
        TimeframeChoice::Preset {
            amount: 5,
            unit: TimeUnit::Minutes,
        },
        TimeframeChoice::Preset {
            amount: 15,
            unit: TimeUnit::Minutes,
        },
        TimeframeChoice::Preset {
            amount: 1,
            unit: TimeUnit::Hours,
        },
        TimeframeChoice::Preset {
            amount: 6,
            unit: TimeUnit::Hours,
        },
        TimeframeChoice::Preset {
            amount: 24,
            unit: TimeUnit::Hours,
        },
        TimeframeChoice::Preset {
            amount: 7,
            unit: TimeUnit::Days,
        },
        TimeframeChoice::Custom,
    ];

    /// The Timeframe a preset stands for; `None` for `Custom`.
    pub fn to_timeframe(self) -> Option<Timeframe> {
        match self {
            TimeframeChoice::Preset { amount, unit } => Some(Timeframe::Relative { amount, unit }),
            TimeframeChoice::Custom => None,
        }
    }
}

impl std::fmt::Display for TimeframeChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TimeframeChoice::Preset { amount, unit } => {
                let unit = match unit {
                    TimeUnit::Minutes => "minute",
                    TimeUnit::Hours => "hour",
                    TimeUnit::Days => "day",
                };
                let plural = if *amount == 1 { "" } else { "s" };
                write!(f, "Last {amount} {unit}{plural}")
            }
            TimeframeChoice::Custom => write!(f, "Custom\u{2026}"),
        }
    }
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

    /// The [`TimeframeChoice`] preset this Timeframe is exactly, if any — used to
    /// show the Search bar dropdown's current selection.
    pub fn matches_preset(&self) -> Option<TimeframeChoice> {
        match self {
            Timeframe::Relative { amount, unit } => {
                let choice = TimeframeChoice::Preset {
                    amount: *amount,
                    unit: *unit,
                };
                TimeframeChoice::ALL.contains(&choice).then_some(choice)
            }
            Timeframe::Absolute { .. } => None,
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
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Config::default();
    };
    let Ok(mut raw) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Config::default();
    };
    migrate_legacy_sort(&mut raw);
    serde_json::from_value(raw).unwrap_or_default()
}

/// Rewrites the pre-multisort `sort_field` / `sort_desc` pair on each Saved
/// Search into a one-entry `sort` array, so older configs keep their sort.
fn migrate_legacy_sort(raw: &mut serde_json::Value) {
    let Some(connections) = raw.get_mut("connections").and_then(|c| c.as_array_mut()) else {
        return;
    };
    for connection in connections {
        let Some(searches) = connection
            .get_mut("searches")
            .and_then(|s| s.as_array_mut())
        else {
            continue;
        };
        for search in searches {
            let Some(obj) = search.as_object_mut() else {
                continue;
            };
            if obj.contains_key("sort") {
                continue;
            }
            let Some(field) = obj
                .remove("sort_field")
                .and_then(|v| v.as_str().map(str::to_string))
            else {
                continue;
            };
            let desc = obj
                .remove("sort_desc")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            obj.insert(
                "sort".to_string(),
                serde_json::json!([{ "field": field, "desc": desc }]),
            );
        }
    }
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
