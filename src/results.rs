//! A Result Tab: the Hits from one run of a Saved Search, rendered as a table.

use std::cell::RefCell;
use std::collections::HashMap;

use iced::widget::{Id, text_editor};

use crate::config::{SortKey, TimeUnit, Timeframe, TimeframeMode};
use crate::es;
use crate::es::Hit;
use crate::line::{Layout, LayoutMode, LineCache, WrapCtx};

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

/// Row height, in pixels, of a single-line (unwrapped) row — and the height
/// of the *first* visual line of a wrapped row, so an unwrapped row looks
/// identical whether or not Wrap is on. Additional wrapped lines each add
/// [`WRAP_LINE_H`].
pub const ROW_H: f32 = 22.0;

/// Height added per extra visual line in a wrapped row — the shaped line
/// height for [`CELL_TEXT_SIZE`] monospace text. The wrapping `text` widget
/// is pinned to this via `LineHeight::Absolute` so the row-height model and
/// the drawn text always agree.
pub const WRAP_LINE_H: f32 = 16.0;

/// Extra height reserved at the bottom of a wrapped row that carries an
/// expand / collapse affordance (or the hard-cap "line truncated" note).
pub const WRAP_AFFORDANCE_H: f32 = 18.0;

/// Wrap width is bucketed to this many pixels before it keys the row-height
/// model, so viewport jitter during a window resize doesn't rebuild the
/// per-Hit estimates every frame. The same bucketed width sizes the wrapping
/// column, so height and draw stay consistent.
pub const WRAP_WIDTH_BUCKET: f32 = 8.0;

/// Hard ceiling on the visual rows a single wrapped Hit can occupy, even
/// expanded or with no user cap — bounds how much text is ever handed to a
/// `text` widget for shaping. A line past this shows a "line truncated —
/// open Hit detail" note. 400 \u{d7} [`WRAP_LINE_H`] \u{2248} 6400px, one long
/// scroll but finite.
pub const WRAP_HARD_MAX: u32 = 400;

/// Font size Hit text renders at, in Table and Text mode alike. The one input
/// [`crate::advance_cache::AdvanceCache::shared`] is built from.
pub const CELL_TEXT_SIZE: f32 = 12.0;

/// A cell-text truncation budget for a `Length::Fill` table Column, in
/// pixels, wider than any real viewport. `hit_table` doesn't track live
/// window width, so the flexible last Column can't ask
/// [`crate::advance_cache::AdvanceCache`] for its true available width — this
/// stands in as a safe upper bound instead: nothing a real window could show
/// is wider than this, so nothing visible is ever cut off, while a
/// pathologically long Hit still gets bounded rather than shaping its entire
/// length every scroll frame.
pub const FILL_COLUMN_MAX_W: f32 = 4000.0;

/// Default table Column width, in pixels, and the range a drag-resize is
/// clamped to. The timestamp Column starts wider than the rest.
pub const COL_DEFAULT_W: f32 = 200.0;
pub const COL_TIMESTAMP_W: f32 = 210.0;
pub const COL_MIN_W: f32 = 60.0;
pub const COL_MAX_W: f32 = 1200.0;

/// Rows rendered above and below the visible viewport as scroll slack. Kept
/// small on purpose: at the default window ~30 rows are visible, so every extra
/// buffer row is a per-frame widget build (and, below `view()`, an iced
/// layout/draw pass) for a row the user cannot see. 8 gives ~175px of fling
/// slack each way while roughly halving the built-row count versus the visible
/// slice — see `docs/plans/wide-line-perf-followups.md` item 1.
const WINDOW_BUFFER: usize = 8;

/// Where a Result Tab's run currently stands.
#[derive(Debug, Clone)]
pub enum RunState {
    /// First Page in flight.
    Loading,
    /// At least one Page loaded and shown.
    Loaded,
    /// The run completed with zero Hits.
    Empty,
    /// The run failed; the message is shown verbatim. Not an [`es::Error`]:
    /// a run can also fail before it reaches a cluster at all, when the
    /// Connection's secret isn't available.
    Error(String),
}

/// The total number of Hits matching a Result Tab's query, fetched via
/// `_count` alongside the first Page. Independent of Max Results, so it can
/// (and often will) exceed `hits.len()`.
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
    /// Stopped at Max Results. The cluster may hold more.
    Capped,
    /// The last next-Page fetch failed; loaded Hits are untouched, retry resumes.
    Failed(es::Error),
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
    /// field, descending — `es` applies that default.
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
    /// Bumped at the start of every run, so a Page or `_count` that arrives
    /// from a superseded run is discarded rather than applied.
    pub generation: u64,
    /// Latest scroll offset / viewport height, for windowed rendering.
    pub scroll_y: f32,
    pub viewport_h: f32,
    /// Latest horizontal scroll offset / viewport width. Table mode is
    /// vertical-only so `scroll_x` stays 0 there; raw text mode slices each
    /// row to `[scroll_x, scroll_x + viewport_w]` before shaping (see
    /// `raw_text_view`), the horizontal analogue of `row_window`.
    pub scroll_x: f32,
    pub viewport_w: f32,
    /// The Hit whose `_source` the bottom detail panel is showing.
    pub selected_hit: Option<usize>,
    /// Pretty-printed `_source` of `selected_hit`, kept selectable.
    pub detail_content: text_editor::Content,
    /// Detail panel height, adjustable by dragging its top edge.
    pub detail_height: f32,
    /// Render `@timestamp`-typed cells in UTC rather than local time.
    pub utc: bool,
    /// The most Hits this tab will load, from `Config.es.max_results`. Paging
    /// stops once `hits.len()` reaches it.
    pub max_results: usize,
    /// Documents pulled per `_search` request, from `Config.es.fetch_size`.
    pub fetch_size: usize,
    /// Table or raw text — the Layout mode, carried from the Saved Search.
    pub mode: LayoutMode,
    /// Wrap long Hit text onto multiple visual rows instead of truncating /
    /// scrolling horizontally. Off by default; toggled from the options strip
    /// and persisted on the Saved Search.
    pub wrap: bool,
    /// Raw text mode's template. Empty until resolved from field caps the
    /// first time raw text mode renders (see [`Layout::default_template`]).
    pub template: String,
    /// Draft text for the Search bar's template input, committed to
    /// `template` on Enter (mirrors `query_draft`).
    pub template_draft: String,
    /// Whether the raw-text "Format" modal (template + field list + preview) is
    /// open for this tab.
    pub format_open: bool,
    /// Rendered-`Line` cache for the windowed table / raw-text row loop, keyed
    /// by Hit position. `RefCell` because `view` only has `&self`; the two
    /// render loops that touch it are Layout-mode-exclusive, so the borrow is
    /// never re-entrant. Reset via [`ResultTab::reset_line_cache`] whenever
    /// `hits` is cleared or replaced.
    pub line_cache: RefCell<LineCache>,
    /// The run this tab's Hits are coming from. `None` while a Page is in
    /// flight — [`Run::next_page`](es::Run::next_page) consumes it and the
    /// landing Page hands it back — which is also what stops two Page fetches
    /// overlapping.
    pub run: Option<es::Run>,
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
    /// given the current scroll offset. Reads the row-height model
    /// ([`LineCache::prepare_heights`] must have run this frame), so it is
    /// correct whether rows are a flat [`ROW_H`] grid or variable-height
    /// wrapped rows.
    pub fn row_window(&self, model: &LineCache) -> (usize, usize) {
        let total = self.hits.len();
        if total == 0 {
            return (0, 0);
        }
        let first = model.row_at(self.scroll_y);
        let last = model.row_at(self.scroll_y + self.viewport_h) + 1;
        let start = first.saturating_sub(WINDOW_BUFFER);
        let end = (last + WINDOW_BUFFER).min(total);
        (start.min(end), end)
    }

    /// The wrap context for this tab's current viewport and the global row
    /// cap: whether wrapping is on, and the bucketed pixel width the wrapped
    /// column / line is laid out at. Width is bucketed to
    /// [`WRAP_WIDTH_BUCKET`] so viewport jitter during a resize doesn't
    /// rebuild the per-Hit estimates every frame.
    pub fn wrap_ctx(&self, cap: Option<usize>) -> WrapCtx {
        let raw_w = match self.mode {
            LayoutMode::RawText => self.viewport_w - 12.0,
            LayoutMode::Table => {
                let last = self.columns.len().saturating_sub(1);
                let fixed: f32 = self
                    .columns
                    .iter()
                    .take(last)
                    .map(|c| self.col_width(c))
                    .sum();
                self.viewport_w - fixed - 8.0 * last as f32 - 12.0
            }
        };
        let width = (raw_w.max(80.0) / WRAP_WIDTH_BUCKET).floor() * WRAP_WIDTH_BUCKET;
        WrapCtx {
            on: self.wrap,
            width: width.max(WRAP_WIDTH_BUCKET),
            cap: cap.map(|c| c.max(1) as u32),
        }
    }

    /// Whether a scroll to `offset_y` (viewport `viewport_h`, content
    /// `content_h`) should kick off the next Page.
    pub fn wants_more(&self, offset_y: f32, viewport_h: f32, content_h: f32) -> bool {
        matches!(self.state, RunState::Loaded)
            && self.paging == Paging::Idle
            && content_h - (offset_y + viewport_h) < 600.0
    }

    /// What this tab is asking the cluster for right now: its Target, the
    /// Search bar's query string, the Timeframe bounds frozen at the start of
    /// the run, and the sort.
    pub fn query(&self) -> es::Query {
        es::Query {
            target: self.target.clone(),
            query_string: self.query_string.clone(),
            timestamp_field: self.timestamp_field.clone(),
            gte: self.gte.clone(),
            lte: self.lte.clone(),
            sort: self
                .sort
                .iter()
                .map(|key| (key.field.clone(), key.desc))
                .collect(),
        }
    }

    /// How far a run of this tab may page, from the Settings window.
    pub fn limits(&self) -> es::Limits {
        es::Limits::new(self.fetch_size, self.max_results)
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

    /// The render-time [`Layout`] for this tab: its persisted `mode`,
    /// `columns` and `template`, plus the two runtime values (`timestamp_field`
    /// and the UTC preference) assembled on every render.
    /// Fills in `template` from the field list the first time it is needed, if
    /// it has not been set yet. Returns whether it changed (so the caller can
    /// persist the Saved Search).
    pub fn resolve_template(&mut self) -> bool {
        if self.template.is_empty() && !self.all_fields.is_empty() {
            self.template = Layout::default_template(&self.all_fields);
            self.template_draft = self.template.clone();
            true
        } else {
            false
        }
    }

    /// Drops every cached rendered `Line`. Call after clearing or replacing
    /// `hits` wholesale — a positional cache key then points at a different
    /// Hit. (Appending more Hits needs no reset: existing positions are
    /// unchanged and new ones are simply absent until first rendered.)
    pub fn reset_line_cache(&mut self) {
        self.line_cache.get_mut().clear();
    }

    pub fn layout(&self) -> Layout {
        Layout {
            mode: self.mode,
            columns: self.columns.clone(),
            template: self.template.clone(),
            timestamp_field: self.timestamp_field.clone(),
            utc: self.utc,
        }
    }
}
