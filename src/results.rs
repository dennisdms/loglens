//! A Result Tab: the Hits from one run of a Saved Search, rendered as a table.

use chrono::{DateTime, Local, TimeZone, Utc};
use serde_json::Value;

use crate::es::Hit;

/// Fixed row height, in pixels — the table renders a windowed slice, so every
/// row must be the same height for scroll maths to line up.
pub const ROW_H: f32 = 22.0;

/// Hard ceiling on Hits loaded into one Result Tab (ADR 0002).
pub const RETENTION_CAP: usize = 10_000;

/// Rows rendered above and below the visible viewport as scroll slack.
const WINDOW_BUFFER: usize = 40;

/// Where a Result Tab's run currently stands.
#[derive(Debug, Clone)]
pub enum RunState {
    /// PIT opening or first Page in flight.
    Loading,
    /// At least one Page loaded and shown.
    Loaded,
    /// The run completed with zero Hits.
    Empty,
    /// The cluster (or transport) returned an error; shown verbatim.
    Error(String),
}

/// Where paging past the first Page stands.
#[derive(Debug, Clone, PartialEq)]
pub enum Paging {
    /// Ready to fetch the next Page on scroll.
    Idle,
    /// A next-Page request is in flight (only ever one at a time).
    Loading,
    /// The cluster returned every matching Hit; nothing more to fetch.
    Exhausted,
    /// Stopped at the 10,000-Hit Retention cap.
    Capped,
    /// The last next-Page fetch failed; loaded Hits are untouched, retry resumes.
    Failed(String),
}

/// One open Result Tab.
pub struct ResultTab {
    /// Stable id so async results find their tab across reordering / closing.
    pub run_id: u64,
    pub connection_id: String,
    pub saved_id: String,
    pub saved_name: String,
    pub target: String,
    pub query_string: String,
    pub timestamp_field: String,
    pub columns: Vec<String>,
    pub sort_field: String,
    pub sort_desc: bool,
    /// Range bounds frozen at the start of this run.
    pub gte: String,
    pub lte: String,
    /// The open Point-in-Time for this tab, once opened.
    pub pit_id: Option<String>,
    pub hits: Vec<Hit>,
    pub state: RunState,
    pub paging: Paging,
    /// Latest scroll offset / viewport height, for windowed rendering.
    pub scroll_y: f32,
    pub viewport_h: f32,
    /// Render `@timestamp`-typed cells in UTC rather than local time.
    pub utc: bool,
}

impl ResultTab {
    /// The `[start, end)` slice of `hits` to actually build widgets for,
    /// given the current scroll offset.
    pub fn row_window(&self) -> (usize, usize) {
        let total = self.hits.len();
        if total == 0 {
            return (0, 0);
        }
        let first_visible = (self.scroll_y / ROW_H).floor().max(0.0) as usize;
        let visible = (self.viewport_h / ROW_H).ceil() as usize;
        let start = first_visible.saturating_sub(WINDOW_BUFFER);
        let end = (first_visible + visible + WINDOW_BUFFER).min(total);
        (start.min(end), end)
    }

    /// Whether a scroll to `offset_y` (viewport `viewport_h`, content
    /// `content_h`) should kick off the next Page.
    pub fn wants_more(&self, offset_y: f32, viewport_h: f32, content_h: f32) -> bool {
        matches!(self.state, RunState::Loaded)
            && self.paging == Paging::Idle
            && self.hits.len() < RETENTION_CAP
            && content_h - (offset_y + viewport_h) < 600.0
    }

    /// The `search_after` cursor for the next Page: the last Hit's sort values.
    pub fn next_cursor(&self) -> Option<Vec<serde_json::Value>> {
        self.hits
            .last()
            .map(|h| h.sort.clone())
            .filter(|s| !s.is_empty())
    }
}

/// The display string for one Hit / Column pair.
///
/// Dotted paths resolve through nested objects (falling back to a literal
/// dotted key); arrays join with `, `; objects render as compact JSON; missing
/// or null fields are blank. The Timeframe's timestamp field is formatted as a
/// local (or UTC) datetime.
pub fn cell(source: &Value, path: &str, timestamp_field: &str, utc: bool) -> String {
    let Some(value) = resolve(source, path) else {
        return String::new();
    };
    if path == timestamp_field {
        if let Some(text) = value.as_str() {
            return format_timestamp_str(text, utc);
        }
        if let Some(millis) = value.as_i64() {
            return format_timestamp_millis(millis, utc);
        }
    }
    render_value(value)
}

fn resolve<'a>(source: &'a Value, path: &str) -> Option<&'a Value> {
    if let Some(value) = source.get(path) {
        if !value.is_null() {
            return Some(value);
        }
    }
    let mut current = source;
    for segment in path.split('.') {
        current = current.get(segment)?;
    }
    (!current.is_null()).then_some(current)
}

fn render_value(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .map(render_value)
            .collect::<Vec<_>>()
            .join(", "),
        Value::Object(_) => serde_json::to_string(value).unwrap_or_default(),
    }
}

fn format_timestamp_str(raw: &str, utc: bool) -> String {
    match DateTime::parse_from_rfc3339(raw) {
        Ok(dt) => format_dt(dt.with_timezone(&Utc), utc),
        Err(_) => raw.to_string(),
    }
}

fn format_timestamp_millis(millis: i64, utc: bool) -> String {
    match Utc.timestamp_millis_opt(millis).single() {
        Some(dt) => format_dt(dt, utc),
        None => millis.to_string(),
    }
}

fn format_dt(dt: DateTime<Utc>, utc: bool) -> String {
    const FMT: &str = "%Y-%m-%d %H:%M:%S%.3f";
    if utc {
        format!("{} UTC", dt.format(FMT))
    } else {
        dt.with_timezone(&Local).format(FMT).to_string()
    }
}
