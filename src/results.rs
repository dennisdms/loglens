//! A Result Tab: the Hits from one run of a Saved Search, rendered as a table.

use std::collections::HashMap;

use chrono::{DateTime, Local, TimeZone, Utc};
use iced::widget::{Id, text_editor};
use serde_json::Value;

use crate::config::{SortKey, TimeUnit, Timeframe, TimeframeMode};
use crate::es::Hit;

/// Draft state for the Search bar's "Custom\u{2026}" timeframe popover: an
/// editable relative (amount + unit) or absolute (from / to) window, applied
/// back onto the Result Tab's [`Timeframe`] when the user confirms.
pub struct TimeframeDraft {
    /// Whether the popover is currently shown.
    pub open: bool,
    pub mode: TimeframeMode,
    pub rel_amount: String,
    pub rel_unit: TimeUnit,
    pub abs_from: String,
    pub abs_to: String,
}

impl TimeframeDraft {
    /// A closed draft pre-filled to describe `tf`.
    pub fn from_timeframe(tf: &Timeframe) -> Self {
        let mut draft = Self {
            open: false,
            mode: TimeframeMode::Relative,
            rel_amount: "15".to_string(),
            rel_unit: TimeUnit::Minutes,
            abs_from: String::new(),
            abs_to: String::new(),
        };
        match tf {
            Timeframe::Relative { amount, unit } => {
                draft.mode = TimeframeMode::Relative;
                draft.rel_amount = amount.to_string();
                draft.rel_unit = *unit;
            }
            Timeframe::Absolute { from, to } => {
                draft.mode = TimeframeMode::Absolute;
                draft.abs_from = from.clone();
                draft.abs_to = to.clone();
            }
        }
        draft
    }

    /// Re-fills the draft from `tf` and opens the popover.
    pub fn seed(&mut self, tf: &Timeframe) {
        *self = Self::from_timeframe(tf);
        self.open = true;
    }

    /// The Timeframe the draft currently describes.
    pub fn to_timeframe(&self) -> Timeframe {
        match self.mode {
            TimeframeMode::Relative => Timeframe::Relative {
                amount: self.rel_amount.trim().parse().unwrap_or(15),
                unit: self.rel_unit,
            },
            TimeframeMode::Absolute => Timeframe::Absolute {
                from: self.abs_from.trim().to_string(),
                to: self.abs_to.trim().to_string(),
            },
        }
    }
}

/// Default height of the Hit detail panel, in pixels.
pub const DETAIL_DEFAULT_H: f32 = 240.0;
pub const DETAIL_MIN_H: f32 = 90.0;
pub const DETAIL_MAX_H: f32 = 680.0;

/// Fixed row height, in pixels — the table renders a windowed slice, so every
/// row must be the same height for scroll maths to line up.
pub const ROW_H: f32 = 22.0;

/// Default table Column width, in pixels, and the range a drag-resize is
/// clamped to. The timestamp Column starts wider than the rest.
pub const COL_DEFAULT_W: f32 = 200.0;
pub const COL_TIMESTAMP_W: f32 = 210.0;
pub const COL_MIN_W: f32 = 60.0;
pub const COL_MAX_W: f32 = 1200.0;

/// Hard ceiling on Hits loaded into one Result Tab (ADR 0002).
pub const RETENTION_CAP: usize = 10_000;

/// Rows rendered above and below the visible viewport as scroll slack.
const WINDOW_BUFFER: usize = 40;

/// Where a Result Tab's run currently stands.
#[derive(Debug, Clone)]
pub enum RunState {
    /// First Page in flight.
    Loading,
    /// At least one Page loaded and shown.
    Loaded,
    /// The run completed with zero Hits.
    Empty,
    /// The cluster (or transport) returned an error; shown verbatim.
    Error(String),
}

/// The total number of Hits matching a Result Tab's query, fetched via
/// `_count` alongside the first Page. Independent of the Retention cap on
/// loaded Hits, so it can (and often will) exceed `hits.len()`.
#[derive(Debug, Clone)]
pub enum TotalHits {
    /// The `_count` request is in flight.
    Loading,
    /// The cluster reported this many matching Hits.
    Known(u64),
    /// The `_count` request failed; no total to show.
    Failed,
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
    /// Draft text for the Search bar's Target input, committed to `target` (and
    /// re-run, with a fresh `_field_caps`) when a suggestion is picked or Enter
    /// is pressed.
    pub target_draft: String,
    /// A Target the user is trying to switch to, still being checked with
    /// `_field_caps`. `target` / `hits` are left alone until the check passes;
    /// if it fails (e.g. the index does not exist) the error goes to
    /// `target_error` and the current results stay put.
    pub target_probe: Option<String>,
    /// The last failed Target switch, shown in the info bar. Cleared when the
    /// user edits the Target again or a switch succeeds.
    pub target_error: Option<String>,
    /// Index / data stream names offered by the Target suggestion dropdown,
    /// from `_cat/indices` + `_data_stream`. Best-effort: empty if the lookup
    /// failed or is still in flight.
    pub target_options: Vec<String>,
    /// Whether the `list_targets` lookup for this tab is still running.
    pub targets_loading: bool,
    /// Whether the Search bar's Target suggestion dropdown is open.
    pub target_panel_open: bool,
    pub query_string: String,
    /// Draft text for the Search bar's query-string input, committed to
    /// `query_string` (and re-run) on Enter.
    pub query_draft: String,
    pub timestamp_field: String,
    pub columns: Vec<String>,
    /// Draft text for the live "add column" control.
    pub column_draft: String,
    /// Per-Column pixel widths set by dragging a header edge. Columns absent
    /// here fall back to a default width (wider for the timestamp Column).
    pub col_widths: HashMap<String, f32>,
    /// Sort order, highest priority first. Empty falls back to the timestamp
    /// field, descending (see [`ResultTab::effective_sort`]).
    pub sort: Vec<SortKey>,
    /// Whether the Search bar's "Sort fields" popover is open.
    pub sort_panel_open: bool,
    /// Column header whose "\u{22ee}" settings menu is open, by column index.
    pub header_menu: Option<usize>,
    /// `_field_caps` for this tab's Target, fetched lazily; empty until it lands
    /// (or if it failed — the pickers then fall back to free text).
    pub all_fields: Vec<String>,
    pub sortable_fields: Vec<String>,
    /// The Timeframe this run covers. Its bounds are re-resolved into `gte` /
    /// `lte` at the start of every run, so a relative window re-anchors to "now".
    pub timeframe: Timeframe,
    /// Draft state for the Search bar's "Custom\u{2026}" timeframe popover.
    pub tf: TimeframeDraft,
    /// Range bounds frozen at the start of this run.
    pub gte: String,
    pub lte: String,
    pub hits: Vec<Hit>,
    pub state: RunState,
    /// True while a re-run (Refresh, edited query / timeframe / target) is in
    /// flight over a tab that had already loaded. Keeps the options and Search
    /// strips pinned and the previous table on screen so nothing flickers while
    /// `state` briefly passes back through `Loading`.
    pub refreshing: bool,
    /// Stable id for the Hit table's `scrollable`, so a completed run can snap it
    /// back to the top even though the widget stays mounted across a refresh.
    pub scroll_id: Id,
    pub paging: Paging,
    /// Total matching Hits (`_count`), loaded asynchronously each run.
    pub total_hits: TotalHits,
    /// Bumped at the start of every run so a late `_count` response from a
    /// superseded run is discarded rather than shown.
    pub total_generation: u64,
    /// Latest scroll offset / viewport height, for windowed rendering.
    pub scroll_y: f32,
    pub viewport_h: f32,
    /// The Hit whose `_source` the bottom detail panel is showing.
    pub selected_hit: Option<usize>,
    /// Pretty-printed `_source` of `selected_hit`, kept selectable.
    pub detail_content: text_editor::Content,
    /// Detail panel height, adjustable by dragging its top edge.
    pub detail_height: f32,
    /// Render `@timestamp`-typed cells in UTC rather than local time.
    pub utc: bool,
}

impl ResultTab {
    /// Whether the options and Search strips should be shown for this tab. True
    /// once a run has produced a table (or an empty result), and held true
    /// through an in-place re-run so the strips — and everything below them —
    /// stay put instead of collapsing while `state` passes back through
    /// `Loading`.
    pub fn strips_visible(&self) -> bool {
        matches!(self.state, RunState::Loaded | RunState::Empty)
            || (self.refreshing && matches!(self.state, RunState::Loading))
    }

    /// Whether to render the Hit table rather than a status placeholder. True
    /// once a run is `Loaded`, and held true through an in-place refresh so the
    /// previous rows and headers stay put instead of flashing out.
    pub fn table_visible(&self) -> bool {
        matches!(self.state, RunState::Loaded)
            || (self.refreshing && matches!(self.state, RunState::Loading) && !self.hits.is_empty())
    }

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

    /// Suggestions for the current Target draft: case-insensitive substring
    /// matches over the loaded index / data stream names, capped for the
    /// dropdown. An empty draft offers the first few names as-is; the committed
    /// Target is kept in the list (Kibana-style) — picking it is just a no-op.
    pub fn target_matches(&self) -> Vec<&String> {
        let draft = self.target_draft.trim();
        // Offer the whole list until the user actually edits away from the
        // committed Target, then narrow to substring matches.
        let show_all = draft.is_empty() || draft == self.target;
        let needle = draft.to_lowercase();
        self.target_options
            .iter()
            .filter(|opt| show_all || opt.to_lowercase().contains(&needle))
            .take(8)
            .collect()
    }

    pub fn add_column(&mut self, field: &str) {
        let field = field.trim();
        if !field.is_empty() && !self.columns.iter().any(|c| c == field) {
            self.columns.push(field.to_string());
        }
        self.column_draft.clear();
    }

    /// The current pixel width of `col`: a drag override if one is recorded,
    /// otherwise the default for that Column.
    pub fn col_width(&self, col: &str) -> f32 {
        if let Some(width) = self.col_widths.get(col) {
            return *width;
        }
        if col == self.timestamp_field {
            COL_TIMESTAMP_W
        } else {
            COL_DEFAULT_W
        }
    }

    /// Nudges Column `index`'s width by `delta` pixels, clamped to the resize
    /// range, recording the result as an explicit override.
    pub fn resize_column(&mut self, index: usize, delta: f32) {
        let Some(col) = self.columns.get(index).cloned() else {
            return;
        };
        let next = (self.col_width(&col) + delta).clamp(COL_MIN_W, COL_MAX_W);
        self.col_widths.insert(col, next);
    }

    pub fn remove_column(&mut self, index: usize) {
        if index < self.columns.len() {
            self.columns.remove(index);
        }
    }

    pub fn move_column(&mut self, index: usize, delta: isize) {
        let target = index as isize + delta;
        if index < self.columns.len() && target >= 0 && (target as usize) < self.columns.len() {
            self.columns.swap(index, target as usize);
        }
    }

    /// The sort keys to actually send to Elasticsearch as `(field, descending)`
    /// pairs: the tab's own keys, or the timestamp field descending when none
    /// are set. Never empty.
    pub fn effective_sort(&self) -> Vec<(String, bool)> {
        if self.sort.is_empty() {
            return vec![(self.timestamp_field.clone(), true)];
        }
        self.sort
            .iter()
            .map(|key| (key.field.clone(), key.desc))
            .collect()
    }

    /// This field's position in the sort order, if it is sorted on.
    pub fn sort_index(&self, field: &str) -> Option<usize> {
        self.sort.iter().position(|key| key.field == field)
    }

    /// Sets `field`'s sort direction, appending it to the order if it is not
    /// already sorted on. Returns whether anything changed.
    pub fn set_sort_dir(&mut self, field: &str, desc: bool) -> bool {
        match self.sort.iter_mut().find(|key| key.field == field) {
            Some(key) => {
                if key.desc == desc {
                    return false;
                }
                key.desc = desc;
            }
            None => self.sort.push(SortKey::new(field, desc)),
        }
        true
    }

    /// Drops `field` from the sort order. Returns whether it was there.
    pub fn remove_sort(&mut self, field: &str) -> bool {
        let before = self.sort.len();
        self.sort.retain(|key| key.field != field);
        self.sort.len() != before
    }

    /// Moves the sort key at `index` by `delta` places. Returns whether it moved.
    pub fn move_sort(&mut self, index: usize, delta: isize) -> bool {
        let target = index as isize + delta;
        if index < self.sort.len() && target >= 0 && (target as usize) < self.sort.len() {
            self.sort.swap(index, target as usize);
            return true;
        }
        false
    }

    /// Clears the sort order (Hits fall back to timestamp descending). Returns
    /// whether there was anything to clear.
    pub fn clear_sort(&mut self) -> bool {
        let had = !self.sort.is_empty();
        self.sort.clear();
        had
    }

    /// Toggles the detail panel for Hit `index`: opens it (loading that Hit's
    /// pretty-printed `_source`), swaps to it, or closes it on a repeat click.
    pub fn toggle_detail(&mut self, index: usize) {
        if self.selected_hit == Some(index) {
            self.selected_hit = None;
            return;
        }
        let Some(hit) = self.hits.get(index) else {
            return;
        };
        let json =
            serde_json::to_string_pretty(&hit.source).unwrap_or_else(|_| hit.source.to_string());
        self.detail_content = text_editor::Content::with_text(&json);
        self.selected_hit = Some(index);
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
    if let Some(value) = source.get(path)
        && !value.is_null()
    {
        return Some(value);
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
