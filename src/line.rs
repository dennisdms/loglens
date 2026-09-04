//! A Hit rendered for display: the one seam the table, raw text mode, and
//! GREP all read through. See CONTEXT.md: Layout, Line, Part.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::advance_cache::AdvanceCache;
use crate::es::Hit;
use crate::results::{ROW_H, WRAP_AFFORDANCE_H, WRAP_HARD_MAX, WRAP_LINE_H};

/// How a Result Tab draws its Hits. Both `columns` and `template` are always
/// present regardless of `mode`, so switching modes never discards the
/// other's settings.
#[derive(Debug, Clone, Hash, Serialize, Deserialize)]
pub struct Layout {
    pub mode: LayoutMode,
    pub columns: Vec<String>,
    pub template: String,
    /// Not persisted on `Layout` itself — assembled by the caller from the
    /// Saved Search's `timestamp_field` and `Config.utc_timestamps` before
    /// each render. See `SavedSearch` in config.rs for where these actually
    /// live on disk.
    #[serde(skip)]
    pub timestamp_field: String,
    #[serde(skip)]
    pub utc: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutMode {
    #[default]
    Table,
    RawText,
}

/// One Hit rendered for display.
#[derive(Debug, Clone, Default)]
pub struct Line {
    pub parts: Vec<Part>,
}

/// One addressable piece of a Line: one Column's text under a Columns
/// Layout, or the whole line under a template Layout.
#[derive(Debug, Clone, Default)]
pub struct Part {
    pub text: String,
}

impl Layout {
    /// `%{message}` when the Target has a `message` field, otherwise compact
    /// `_source` JSON (via the reserved `_source` path). Decided once, when
    /// the Layout needs a default — never per line, and never re-derived on
    /// every render.
    pub fn default_template(all_fields: &[String]) -> String {
        if all_fields.iter().any(|f| f == "message") {
            "%{message}".to_string()
        } else {
            "%{_source}".to_string()
        }
    }

    /// A hash of every input [`render`] reads — `mode`, `columns`, `template`,
    /// `timestamp_field`, `utc`. [`LineCache`] keys its entries on this so any
    /// change that would produce a different `Line` invalidates the cache.
    /// Deliberately *not* affected by column pixel widths (`col_widths` on
    /// `ResultTab`): those never reach `render`, so a drag-resize must not bust
    /// the cache.
    pub fn fingerprint(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.hash(&mut hasher);
        hasher.finish()
    }
}

/// The wrap state a [`LineCache`]'s row-height model was last built under.
/// Not part of [`Layout::fingerprint`] — `render` produces the same `Line`
/// text wrapped or not; only the height model and the view layer care.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct WrapCtx {
    /// Wrap long Hit text onto multiple visual rows instead of truncating.
    pub on: bool,
    /// The wrap width in pixels, already bucketed to
    /// [`crate::results::WRAP_WIDTH_BUCKET`] by the caller.
    pub width: f32,
    /// Visual-row cap per Hit; `None` = wrap to full height. A Hit past the
    /// cap is clamped and gets an expand affordance.
    pub cap: Option<u32>,
}

impl WrapCtx {
    fn key(&self) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.on.hash(&mut h);
        self.width.to_bits().hash(&mut h);
        self.cap.hash(&mut h);
        h.finish()
    }
}

/// Byte length and hard-newline count of one Hit's wrap-relevant text — the
/// cheap, render-once basis for estimating how many visual rows an
/// *off-screen* Hit wraps to, without shaping it. On-screen Hits get an exact
/// count from [`AdvanceCache::wrap_rows`] instead (see [`LineCache::get`]).
#[derive(Debug, Clone, Copy, Default)]
struct LineMetric {
    len: u32,
    nl: u32,
}

/// What affordance, if any, a wrapped row carries at its bottom edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Affordance {
    None,
    /// Row is capped; this many more visual rows are hidden.
    Expand(u32),
    /// Row is expanded past the cap; offer to collapse it.
    Collapse,
    /// Row hit [`crate::results::WRAP_HARD_MAX`]; the rest is unreachable
    /// here — point at the Hit detail panel.
    Truncated,
}

/// Per-Result-Tab cache of rendered [`Line`]s **and** the variable
/// row-height model that drives windowed scrolling, keyed by a Hit's
/// position in `tab.hits`. Every scroll frame would otherwise re-render the
/// whole windowed slice (JSON resolution, multi-KB string cloning, timestamp
/// formatting) — see `docs/plans/wide-line-perf-followups.md` items 3 and 6.
///
/// Positional, not content-keyed: `tab.hits` positions are stable within a
/// loaded result set (paging appends; sort / refresh replace wholesale and
/// call [`LineCache::clear`]). A content hash would cost hashing multi-KB JSON
/// per row per frame and would over-report its hit rate against the
/// scroll-harness's identical-copy fixtures (see
/// `.claude/skills/dev/references/performance-benchmarking.md`).
///
/// Height model: `rows[i]` is the best-known *uncapped* visual row count for
/// Hit `i` — a byte-length estimate for off-screen Hits, upgraded to an exact
/// [`AdvanceCache::wrap_rows`] count the first time the Hit is rendered
/// on-screen. `offsets` is the prefix sum of per-Hit pixel heights, so
/// windowing and the scrollbar extent are O(log n) lookups. With
/// [`WrapCtx::on`] false the model degenerates to a flat
/// [`ROW_H`]-per-row grid (today's behaviour, no render pass).
#[derive(Default)]
pub struct LineCache {
    lines: HashMap<usize, Line>,
    /// [`Layout::fingerprint`] the current entries were rendered under.
    key: u64,
    /// The longest raw-text [`Line`] (in bytes) rendered since the last
    /// [`Layout`] change — a monotonic, O(1)-per-miss estimate of the widest
    /// line, used to size raw text mode's horizontal scrollbar without shaping
    /// every full line. Byte length over-approximates monospace column width
    /// for any non-ASCII content, so it never hides reachable text; it only
    /// grows as longer lines scroll into view, the same way the vertical
    /// extent grows with paging. Zero in Table mode (never updated there).
    max_line_bytes: usize,
    /// [`WrapCtx::key`] the height model below was built under.
    wrap_key: u64,
    /// Wrap-relevant text metrics, dense by Hit position. Empty (and unused)
    /// while wrapping is off; primed by a one-off render pass the first time
    /// it turns on, extended for the tail on paging.
    metric: Vec<LineMetric>,
    /// Best-known uncapped visual row count per Hit position. `1` everywhere
    /// while wrapping is off.
    rows: Vec<u32>,
    /// Positions whose `rows` entry is an exact [`AdvanceCache::wrap_rows`]
    /// count (not an estimate) under the current `wrap_key` — so a row that
    /// stays on screen across frames is measured once, not every frame.
    /// Cleared whenever the estimates are recomputed.
    exact: HashSet<usize>,
    /// Hits the user expanded past [`WrapCtx::cap`].
    expanded: HashSet<usize>,
    /// Prefix sums of per-Hit pixel heights; `offsets[i]` is the top of Hit
    /// `i`, `offsets.last()` the total content height. `len == rows.len() + 1`.
    offsets: Vec<f32>,
    /// `offsets` needs rebuilding (a row count or the expanded set changed).
    dirty: bool,
    /// The context `prepare_heights` last ran with, for the frame's `get` /
    /// accessor calls.
    ctx: WrapCtx,
}

/// Rows kept cached on each side of the live window. Comfortably more than a
/// single frame's scroll delta (even a fast fling), so rows staying on screen
/// across frames stay warm, while a long scroll can't grow the map without
/// bound.
const RETAIN: usize = 64;

/// The visual row count Hit `i` actually renders at, given its uncapped count.
fn disp_rows(full: u32, expanded: bool, ctx: &WrapCtx) -> u32 {
    if !ctx.on {
        return 1;
    }
    match ctx.cap {
        Some(cap) if !expanded => full.min(cap),
        _ => full.min(WRAP_HARD_MAX),
    }
}

fn affordance_of(full: u32, expanded: bool, ctx: &WrapCtx) -> Affordance {
    if !ctx.on {
        return Affordance::None;
    }
    if disp_rows(full, expanded, ctx) >= WRAP_HARD_MAX && full >= WRAP_HARD_MAX {
        return Affordance::Truncated;
    }
    match ctx.cap {
        Some(cap) if full > cap => {
            if expanded {
                Affordance::Collapse
            } else {
                Affordance::Expand(full - cap)
            }
        }
        _ => Affordance::None,
    }
}

fn row_px(full: u32, expanded: bool, ctx: &WrapCtx) -> f32 {
    if !ctx.on {
        return ROW_H;
    }
    let disp = disp_rows(full, expanded, ctx);
    let mut h = ROW_H + disp.saturating_sub(1) as f32 * WRAP_LINE_H;
    if affordance_of(full, expanded, ctx) != Affordance::None {
        h += WRAP_AFFORDANCE_H;
    }
    h
}

/// Estimated uncapped visual rows for an off-screen Hit: whole-text width
/// plus one row lost to each hard newline, an upper bound that runs slightly
/// long and tightens to the exact count once the Hit is rendered on-screen.
fn estimate_rows(m: LineMetric, width: f32, mono_adv: f32) -> u32 {
    if width <= 0.0 {
        return 1;
    }
    let by_width = (m.len as f32 * mono_adv / width).ceil() as u32;
    let rows = by_width.saturating_add(m.nl).max(m.nl.saturating_add(1));
    rows.min(WRAP_HARD_MAX)
}

impl LineCache {
    /// Prime / refresh the row-height model for the whole loaded set. Call
    /// once per frame before [`row_window`](crate::results::ResultTab::row_window)
    /// and the `get` loop. Cheap unless a [`Layout`] change forces a re-render
    /// pass, wrapping just turned on (a one-off render pass to measure every
    /// line), or paging added a tail.
    pub fn prepare_heights(
        &mut self,
        hits: &[Hit],
        layout: &Layout,
        ctx: WrapCtx,
        adv: &AdvanceCache,
    ) {
        let key = layout.fingerprint();
        if key != self.key {
            self.lines.clear();
            self.max_line_bytes = 0;
            self.metric.clear();
            self.rows.clear();
            self.exact.clear();
            self.offsets.clear();
            self.expanded.clear();
            self.wrap_key = 0;
            self.key = key;
            self.dirty = true;
        }

        let wk = ctx.key();

        if !ctx.on {
            // Keep `metric` (still valid for this Layout) so a later on→off→on
            // toggle re-estimates by arithmetic instead of re-rendering.
            if self.rows.len() != hits.len() || self.wrap_key != wk {
                self.rows = vec![1; hits.len()];
                self.expanded.clear();
                self.wrap_key = wk;
                self.dirty = true;
            }
            if self.dirty {
                self.rebuild_offsets(&ctx);
                self.dirty = false;
            }
            self.ctx = ctx;
            return;
        }

        // Wrapping is on: keep a dense metric per Hit. A shrink can only come
        // from an un-`clear`ed replacement — treat it as a reset.
        if self.metric.len() > hits.len() {
            self.metric.clear();
            self.rows.clear();
            self.exact.clear();
            self.expanded.clear();
            self.wrap_key = 0;
            self.dirty = true;
        }
        if self.metric.len() < hits.len() {
            let span = crate::perf::span("view.row_cache.lens");
            for hit in &hits[self.metric.len()..] {
                let line = render(hit, layout);
                let text = wrap_text(&line, layout.mode);
                self.metric.push(LineMetric {
                    len: text.len().min(u32::MAX as usize) as u32,
                    nl: text
                        .bytes()
                        .filter(|&b| b == b'\n')
                        .count()
                        .min(u32::MAX as usize) as u32,
                });
            }
            drop(span);
            self.wrap_key = 0; // new rows need estimating
            self.dirty = true;
        }

        if self.wrap_key != wk {
            self.wrap_key = wk;
            let mono = adv.mono_advance();
            self.rows = self
                .metric
                .iter()
                .map(|m| estimate_rows(*m, ctx.width, mono))
                .collect();
            self.exact.clear();
            self.dirty = true;
        }

        if self.dirty {
            self.rebuild_offsets(&ctx);
            self.dirty = false;
        }
        self.ctx = ctx;
    }

    fn rebuild_offsets(&mut self, ctx: &WrapCtx) {
        self.offsets.clear();
        self.offsets.reserve(self.rows.len() + 1);
        let mut acc = 0.0f32;
        self.offsets.push(0.0);
        for (i, &full) in self.rows.iter().enumerate() {
            acc += row_px(full, self.expanded.contains(&i), ctx);
            self.offsets.push(acc);
        }
    }

    /// Evict rendered `Line`s far outside `window` so the map stays bounded.
    /// Call after [`prepare_heights`](Self::prepare_heights) and the window
    /// calculation, before the `get` loop.
    pub fn prepare_lines(&mut self, window: (usize, usize)) {
        let (start, end) = window;
        let lo = start.saturating_sub(RETAIN);
        let hi = end.saturating_add(RETAIN);
        self.lines.retain(|&index, _| (lo..hi).contains(&index));
    }

    /// The rendered `Line` for `hit` at `index`, rendering and caching it on a
    /// miss. Also upgrades the height model's row count for `index` from the
    /// off-screen estimate to an exact [`AdvanceCache::wrap_rows`] count, since
    /// this Hit is about to be drawn.
    pub fn get(&mut self, index: usize, hit: &Hit, layout: &Layout, adv: &AdvanceCache) -> &Line {
        if !self.lines.contains_key(&index) {
            let line = render(hit, layout);
            if layout.mode == LayoutMode::RawText {
                let bytes = line.parts.first().map_or(0, |p| p.text.len());
                self.max_line_bytes = self.max_line_bytes.max(bytes);
            }
            self.lines.insert(index, line);
        }
        if self.ctx.on && index < self.rows.len() && !self.exact.contains(&index) {
            let exact = {
                let text = wrap_text(&self.lines[&index], layout.mode);
                adv.wrap_rows(text, self.ctx.width, WRAP_HARD_MAX + 1).0
            };
            self.exact.insert(index);
            if exact != self.rows[index] {
                self.rows[index] = exact;
                self.dirty = true; // offsets refreshed next frame
            }
        }
        &self.lines[&index]
    }

    /// The already-rendered `Line` for `index` — call right after
    /// [`get`](Self::get) for that index, so the borrow checker lets the
    /// immutable metric accessors (`disp_rows`, `affordance`, `row_height`)
    /// be read alongside it. Falls back to a shared empty `Line` if `index`
    /// was somehow never rendered.
    pub fn line(&self, index: usize) -> &Line {
        static EMPTY: std::sync::OnceLock<Line> = std::sync::OnceLock::new();
        self.lines
            .get(&index)
            .unwrap_or_else(|| EMPTY.get_or_init(Line::default))
    }

    /// The longest raw-text line, in bytes, rendered since the last [`Layout`]
    /// change. See [`max_line_bytes`](Self::max_line_bytes).
    pub fn max_line_bytes(&self) -> usize {
        self.max_line_bytes
    }

    /// Total pixel height of every loaded Hit under the current model.
    pub fn content_height(&self) -> f32 {
        self.offsets.last().copied().unwrap_or(0.0)
    }

    /// Pixel offset of the top of Hit `i`.
    pub fn offset(&self, i: usize) -> f32 {
        self.offsets.get(i).copied().unwrap_or(0.0)
    }

    /// Pixel height of Hit `i`'s row.
    pub fn row_height(&self, i: usize) -> f32 {
        match (self.offsets.get(i), self.offsets.get(i + 1)) {
            (Some(&a), Some(&b)) => b - a,
            _ => ROW_H,
        }
    }

    /// The last Hit whose row top is at or above `y` — i.e. the first row a
    /// viewport scrolled to `y` shows.
    pub fn row_at(&self, y: f32) -> usize {
        match self.offsets.partition_point(|&o| o <= y) {
            0 => 0,
            k => k - 1,
        }
    }

    /// Visual rows Hit `i` actually renders (capped / expanded).
    pub fn disp_rows(&self, i: usize) -> u32 {
        disp_rows(
            self.rows.get(i).copied().unwrap_or(1),
            self.expanded.contains(&i),
            &self.ctx,
        )
    }

    /// The affordance Hit `i`'s row carries, if any.
    pub fn affordance(&self, i: usize) -> Affordance {
        affordance_of(
            self.rows.get(i).copied().unwrap_or(1),
            self.expanded.contains(&i),
            &self.ctx,
        )
    }

    /// Toggle Hit `i`'s expanded-past-the-cap state and refresh the model.
    pub fn toggle_expand(&mut self, i: usize) {
        if !self.expanded.remove(&i) {
            self.expanded.insert(i);
        }
        let ctx = self.ctx;
        self.rebuild_offsets(&ctx);
    }

    /// Drops every entry — rendered lines and the whole height model. For the
    /// callers that replace or clear `tab.hits` wholesale, after which a
    /// positional key means something different.
    pub fn clear(&mut self) {
        self.lines.clear();
        self.max_line_bytes = 0;
        self.wrap_key = 0;
        self.metric.clear();
        self.rows.clear();
        self.exact.clear();
        self.expanded.clear();
        self.offsets.clear();
        self.dirty = false;
    }
}

/// The slice of a rendered [`Line`] that wraps: the flexible last column in
/// Table mode, the whole line in raw text mode.
fn wrap_text(line: &Line, mode: LayoutMode) -> &str {
    match mode {
        LayoutMode::Table => line.parts.last(),
        LayoutMode::RawText => line.parts.first(),
    }
    .map_or("", |p| p.text.as_str())
}

// --- Render ----------------------------------------------------------------

/// Everything this module exposes: renders `hit` under `layout`.
pub fn render(hit: &Hit, layout: &Layout) -> Line {
    match layout.mode {
        LayoutMode::Table => render_table(hit, layout),
        LayoutMode::RawText => render_raw_text(hit, layout),
    }
}

fn render_table(hit: &Hit, layout: &Layout) -> Line {
    let parts = layout
        .columns
        .iter()
        .map(|col| Part {
            text: cell_text(&hit.source, col, &layout.timestamp_field, layout.utc),
        })
        .collect();
    Line { parts }
}

fn render_raw_text(hit: &Hit, layout: &Layout) -> Line {
    let pieces = parse_template(&layout.template);
    let text: String = pieces
        .into_iter()
        .map(|piece| match piece {
            Piece::Literal(s) => s,
            Piece::Field(path) => {
                cell_text(&hit.source, &path, &layout.timestamp_field, layout.utc)
            }
        })
        .collect();
    Line {
        parts: vec![Part { text }],
    }
}

// --- Template parsing ------------------------------------------------------

/// One piece of a parsed template: either literal text or a field
/// placeholder to resolve against a Hit.
#[derive(Debug, Clone, PartialEq)]
enum Piece {
    Literal(String),
    Field(String),
}

/// Splits a `%{field.path}` template into literal and placeholder pieces.
///
/// `%{` opens a placeholder and the first `}` closes it. An unclosed `%{`, or
/// a `|` before the closing `}`, makes the whole `%{...` span literal text
/// (`|` is reserved for a future modifier syntax). A missing/null field
/// renders empty — handled by `cell_text`, not here.
fn parse_template(template: &str) -> Vec<Piece> {
    let mut pieces: Vec<Piece> = Vec::new();
    let mut literal = String::new();
    let bytes = template.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
            let rest = &template[i + 2..];
            match rest.find(['}', '|']) {
                Some(rel) if rest.as_bytes()[rel] == b'}' => {
                    let path = &rest[..rel];
                    if !literal.is_empty() {
                        pieces.push(Piece::Literal(std::mem::take(&mut literal)));
                    }
                    pieces.push(Piece::Field(path.to_string()));
                    i += 2 + rel + 1;
                    continue;
                }
                _ => {
                    // `|` before `}`, or no `}` at all: treat `%{` as literal
                    // and resume scanning just past it.
                    literal.push_str("%{");
                    i += 2;
                    continue;
                }
            }
        }
        let ch = template[i..].chars().next().unwrap();
        literal.push(ch);
        i += ch.len_utf8();
    }
    if !literal.is_empty() {
        pieces.push(Piece::Literal(literal));
    }
    pieces
}

/// The `%{field.path}` placeholders in `template` (excluding the reserved
/// `_source`) that are not present in `all_fields`. Drives the Search bar's
/// live template validation warning; never blocks submission.
pub fn unknown_template_fields(template: &str, all_fields: &[String]) -> Vec<String> {
    let mut unknown = Vec::new();
    for piece in parse_template(template) {
        if let Piece::Field(path) = piece
            && path != "_source"
            && !all_fields.iter().any(|f| f == &path)
            && !unknown.contains(&path)
        {
            unknown.push(path);
        }
    }
    unknown
}

// --- Cell / field resolution (moved from results.rs) ----------------------

/// The display string for one Hit / field pair.
///
/// Dotted paths resolve through nested objects (falling back to a literal
/// dotted key); arrays join with `, `; objects render as compact JSON; missing
/// or null fields are blank. The field matching `timestamp_field` is formatted
/// as a local (or UTC) datetime. The reserved path `_source` renders the whole
/// document as compact JSON.
fn cell_text(source: &Value, path: &str, timestamp_field: &str, utc: bool) -> String {
    if path == "_source" {
        return render_value(source);
    }
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
    use chrono::DateTime;
    match DateTime::parse_from_rfc3339(raw) {
        Ok(dt) => format_dt(dt.with_timezone(&chrono::Utc), utc),
        Err(_) => raw.to_string(),
    }
}

fn format_timestamp_millis(millis: i64, utc: bool) -> String {
    use chrono::TimeZone;
    match chrono::Utc.timestamp_millis_opt(millis).single() {
        Some(dt) => format_dt(dt, utc),
        None => millis.to_string(),
    }
}

fn format_dt(dt: chrono::DateTime<chrono::Utc>, utc: bool) -> String {
    use chrono::Local;
    const FMT: &str = "%Y-%m-%d %H:%M:%S%.3f";
    if utc {
        format!("{} UTC", dt.format(FMT))
    } else {
        dt.with_timezone(&Local).format(FMT).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn hit(source: Value) -> Hit {
        Hit::detached(source)
    }

    fn table_layout(columns: &[&str], timestamp_field: &str, utc: bool) -> Layout {
        Layout {
            mode: LayoutMode::Table,
            columns: columns.iter().map(|c| c.to_string()).collect(),
            template: String::new(),
            timestamp_field: timestamp_field.to_string(),
            utc,
        }
    }

    fn raw_layout(template: &str) -> Layout {
        Layout {
            mode: LayoutMode::RawText,
            columns: Vec::new(),
            template: template.to_string(),
            timestamp_field: "@timestamp".to_string(),
            utc: false,
        }
    }

    fn only_text(line: &Line) -> Vec<String> {
        line.parts.iter().map(|p| p.text.clone()).collect()
    }

    fn cell(source: &Value, path: &str) -> String {
        cell_text(source, path, "@timestamp", false)
    }

    #[test]
    fn dotted_path_resolves_through_nested_objects() {
        let source = json!({ "a": { "b": { "c": 1 } } });
        assert_eq!(cell(&source, "a.b.c"), "1");
    }

    #[test]
    fn dotted_path_falls_back_to_literal_dotted_key() {
        let source = json!({ "a.b.c": "flat" });
        assert_eq!(cell(&source, "a.b.c"), "flat");
    }

    #[test]
    fn missing_field_renders_empty() {
        let source = json!({ "a": 1 });
        assert_eq!(cell(&source, "b"), "");
    }

    #[test]
    fn null_field_renders_empty() {
        let source = json!({ "a": null });
        assert_eq!(cell(&source, "a"), "");
    }

    #[test]
    fn array_of_strings_joins_with_comma_space() {
        let source = json!({ "tags": ["x", "y", "z"] });
        assert_eq!(cell(&source, "tags"), "x, y, z");
    }

    #[test]
    fn object_renders_as_compact_json() {
        let source = json!({ "obj": { "k": 1, "n": "v" } });
        assert_eq!(cell(&source, "obj"), r#"{"k":1,"n":"v"}"#);
    }

    #[test]
    fn timestamp_iso_string_local_time() {
        let source = json!({ "@timestamp": "2024-01-02T03:04:05.678Z" });
        let out = cell_text(&source, "@timestamp", "@timestamp", false);
        assert!(!out.ends_with(" UTC"));
        assert!(!out.contains('T'));
        assert!(out.starts_with("202"));
    }

    #[test]
    fn timestamp_iso_string_utc() {
        let source = json!({ "@timestamp": "2024-01-02T03:04:05.678Z" });
        let out = cell_text(&source, "@timestamp", "@timestamp", true);
        assert_eq!(out, "2024-01-02 03:04:05.678 UTC");
    }

    #[test]
    fn timestamp_epoch_millis_local_and_utc() {
        let source = json!({ "@timestamp": 1_704_164_645_678_i64 });
        let utc = cell_text(&source, "@timestamp", "@timestamp", true);
        assert_eq!(utc, "2024-01-02 03:04:05.678 UTC");
        let local = cell_text(&source, "@timestamp", "@timestamp", false);
        assert!(!local.ends_with(" UTC"));
        assert!(local.starts_with("202"));
    }

    #[test]
    fn non_timestamp_date_like_string_is_not_reformatted() {
        let source = json!({ "created": "2024-01-02T03:04:05.678Z" });
        assert_eq!(cell(&source, "created"), "2024-01-02T03:04:05.678Z");
    }

    #[test]
    fn render_table_maps_one_part_per_column() {
        let source = json!({ "level": "INFO", "message": "hello" });
        let layout = table_layout(&["level", "message"], "@timestamp", false);
        let line = render(&hit(source), &layout);
        assert_eq!(only_text(&line), vec!["INFO", "hello"]);
    }

    // --- template parsing ---

    #[test]
    fn literal_only_template_renders_unchanged() {
        assert_eq!(
            parse_template("plain text"),
            vec![Piece::Literal("plain text".to_string())]
        );
        let line = render(&hit(json!({})), &raw_layout("plain text"));
        assert_eq!(only_text(&line), vec!["plain text"]);
    }

    #[test]
    fn single_placeholder_resolves() {
        let line = render(&hit(json!({ "message": "hi" })), &raw_layout("%{message}"));
        assert_eq!(only_text(&line), vec!["hi"]);
    }

    #[test]
    fn placeholder_between_literals() {
        assert_eq!(
            parse_template("[%{level}] done"),
            vec![
                Piece::Literal("[".to_string()),
                Piece::Field("level".to_string()),
                Piece::Literal("] done".to_string()),
            ]
        );
    }

    #[test]
    fn unclosed_placeholder_at_end_is_literal() {
        assert_eq!(
            parse_template("start %{oops"),
            vec![Piece::Literal("start %{oops".to_string())]
        );
    }

    #[test]
    fn pipe_in_placeholder_makes_whole_span_literal() {
        assert_eq!(
            parse_template("a %{x|y} b"),
            vec![Piece::Literal("a %{x|y} b".to_string())]
        );
    }

    #[test]
    fn unresolved_placeholder_renders_empty_keeping_literals() {
        let line = render(
            &hit(json!({ "message": "hi" })),
            &raw_layout("[%{nope}] tail"),
        );
        assert_eq!(only_text(&line), vec!["[] tail"]);
    }

    #[test]
    fn source_placeholder_renders_compact_json() {
        let line = render(&hit(json!({ "a": 1, "b": "x" })), &raw_layout("%{_source}"));
        assert_eq!(only_text(&line), vec![r#"{"a":1,"b":"x"}"#]);
    }

    #[test]
    fn default_template_picks_message_or_source() {
        assert_eq!(
            Layout::default_template(&["a".to_string(), "message".to_string()]),
            "%{message}"
        );
        assert_eq!(Layout::default_template(&["a".to_string()]), "%{_source}");
        assert_eq!(Layout::default_template(&[]), "%{_source}");
    }

    // --- LineCache ---

    fn adv() -> AdvanceCache {
        AdvanceCache::new(iced::Font::MONOSPACE, 12.0)
    }

    /// Wrapping off — the height model degenerates to a flat ROW_H grid and
    /// `prepare_heights` never inspects Hit content.
    const WRAP_OFF: WrapCtx = WrapCtx {
        on: false,
        width: 0.0,
        cap: None,
    };

    fn wrap_on(width: f32, cap: Option<u32>) -> WrapCtx {
        WrapCtx {
            on: true,
            width,
            cap,
        }
    }

    #[test]
    fn line_cache_serves_a_hit_without_re_rendering() {
        let layout = table_layout(&["message"], "@timestamp", false);
        let adv = adv();
        let mut cache = LineCache::default();
        let hits = [
            hit(json!({ "message": "original" })),
            hit(json!({ "message": "changed" })),
        ];
        cache.prepare_heights(&hits, &layout, WRAP_OFF, &adv);
        cache.prepare_lines((0, 1));

        assert_eq!(
            cache.get(0, &hits[0], &layout, &adv).parts[0].text,
            "original"
        );
        // A different Hit at the same position must not be re-rendered.
        assert_eq!(
            cache.get(0, &hits[1], &layout, &adv).parts[0].text,
            "original"
        );
    }

    #[test]
    fn line_cache_prepare_drops_everything_on_layout_change() {
        let a = table_layout(&["message"], "@timestamp", false);
        let b = table_layout(&["level"], "@timestamp", false);
        let adv = adv();
        let mut cache = LineCache::default();
        let hits = [hit(json!({ "message": "m", "level": "INFO" }))];

        cache.prepare_heights(&hits, &a, WRAP_OFF, &adv);
        cache.prepare_lines((0, 1));
        cache.get(0, &hits[0], &a, &adv);
        assert_eq!(cache.lines.len(), 1);

        cache.prepare_heights(&hits, &b, WRAP_OFF, &adv);
        assert!(cache.lines.is_empty());
        let line = cache.get(0, &hits[0], &b, &adv);
        assert_eq!(line.parts[0].text, "INFO");
    }

    #[test]
    fn line_cache_prepare_evicts_rows_far_from_the_window() {
        let layout = raw_layout("%{message}");
        let adv = adv();
        let mut cache = LineCache::default();
        let hits: Vec<Hit> = (0..600)
            .map(|i| hit(json!({ "message": format!("row {i}") })))
            .collect();

        cache.prepare_heights(&hits, &layout, WRAP_OFF, &adv);
        cache.prepare_lines((0, 200));
        for (i, h) in hits.iter().enumerate().take(200) {
            cache.get(i, h, &layout, &adv);
        }
        assert_eq!(cache.lines.len(), 200);

        cache.prepare_lines((500, 540));
        assert!(cache.lines.is_empty());

        cache.prepare_lines((100, 300));
        for (i, h) in hits.iter().enumerate().take(300).skip(100) {
            cache.get(i, h, &layout, &adv);
        }
        // Rows within RETAIN (64) of the window [130, 170) survive.
        cache.prepare_lines((130, 170));
        assert!(cache.lines.keys().all(|&i| (66..234).contains(&i)));
        assert!(cache.lines.contains_key(&120));
        assert!(cache.lines.contains_key(&233));
        assert!(!cache.lines.contains_key(&234));
    }

    #[test]
    fn line_cache_tracks_widest_raw_line_and_resets_it() {
        let layout = raw_layout("%{message}");
        let adv = adv();
        let mut cache = LineCache::default();
        let hits = [
            hit(json!({ "message": "short" })),
            hit(json!({ "message": "a much longer line here" })),
            hit(json!({ "message": "mid" })),
        ];
        cache.prepare_heights(&hits, &layout, WRAP_OFF, &adv);
        cache.prepare_lines((0, 3));
        for (i, h) in hits.iter().enumerate() {
            cache.get(i, h, &layout, &adv);
        }
        assert_eq!(cache.max_line_bytes(), "a much longer line here".len());

        let other = raw_layout("%{level}");
        cache.prepare_heights(&hits, &other, WRAP_OFF, &adv);
        assert_eq!(cache.max_line_bytes(), 0);
    }

    #[test]
    fn line_cache_clear_drops_everything() {
        let layout = raw_layout("%{message}");
        let adv = adv();
        let mut cache = LineCache::default();
        let hits = [
            hit(json!({ "message": "a" })),
            hit(json!({ "message": "b" })),
        ];
        cache.prepare_heights(&hits, &layout, wrap_on(200.0, Some(8)), &adv);
        cache.prepare_lines((0, 2));
        cache.get(0, &hits[0], &layout, &adv);
        cache.clear();
        assert!(cache.lines.is_empty());
        assert!(cache.rows.is_empty());
        assert_eq!(cache.content_height(), 0.0);
    }

    #[test]
    fn height_model_is_a_flat_grid_when_wrapping_is_off() {
        let layout = raw_layout("%{message}");
        let adv = adv();
        let mut cache = LineCache::default();
        let hits: Vec<Hit> = (0..10)
            .map(|_| hit(json!({ "message": "x".repeat(5000) })))
            .collect();
        cache.prepare_heights(&hits, &layout, WRAP_OFF, &adv);
        assert_eq!(cache.content_height(), 10.0 * ROW_H);
        assert_eq!(cache.offset(3), 3.0 * ROW_H);
        assert_eq!(cache.row_at(2.5 * ROW_H), 2);
        assert_eq!(cache.disp_rows(0), 1);
    }

    #[test]
    fn height_model_grows_rows_and_offsets_when_wrapping() {
        let layout = raw_layout("%{message}");
        let adv = adv();
        let mut cache = LineCache::default();
        let hits = [
            hit(json!({ "message": "short" })),
            hit(json!({ "message": "y".repeat(4000) })),
            hit(json!({ "message": "short" })),
        ];
        let ctx = wrap_on(200.0, None);
        cache.prepare_heights(&hits, &layout, ctx, &adv);
        cache.prepare_lines((0, 3));
        for (i, h) in hits.iter().enumerate() {
            cache.get(i, h, &layout, &adv);
        }
        // rebuild offsets with the now-exact middle-row count
        cache.prepare_heights(&hits, &layout, ctx, &adv);

        assert_eq!(cache.disp_rows(0), 1);
        assert!(cache.disp_rows(1) > 10);
        assert_eq!(cache.row_height(0), ROW_H);
        assert!(cache.row_height(1) > ROW_H + 10.0 * WRAP_LINE_H);
        // total = row0 + row1 + row2
        let total = cache.row_height(0) + cache.row_height(1) + cache.row_height(2);
        assert!((cache.content_height() - total).abs() < 0.01);
    }

    #[test]
    fn height_model_caps_rows_and_offers_expand() {
        let layout = raw_layout("%{message}");
        let adv = adv();
        let mut cache = LineCache::default();
        let hits = [hit(json!({ "message": "z".repeat(4000) }))];
        let ctx = wrap_on(200.0, Some(5));
        cache.prepare_heights(&hits, &layout, ctx, &adv);
        cache.prepare_lines((0, 1));
        cache.get(0, &hits[0], &layout, &adv);
        cache.prepare_heights(&hits, &layout, ctx, &adv);

        assert_eq!(cache.disp_rows(0), 5);
        assert!(matches!(cache.affordance(0), Affordance::Expand(_)));

        cache.toggle_expand(0);
        assert!(cache.disp_rows(0) > 5);
        assert_eq!(cache.affordance(0), Affordance::Collapse);

        cache.toggle_expand(0);
        assert_eq!(cache.disp_rows(0), 5);
    }

    #[test]
    fn layout_fingerprint_tracks_every_render_input() {
        let base = Layout {
            mode: LayoutMode::Table,
            columns: vec!["message".to_string()],
            template: "%{message}".to_string(),
            timestamp_field: "@timestamp".to_string(),
            utc: false,
        };
        let fp = base.fingerprint();
        assert_eq!(base.clone().fingerprint(), fp);

        let mut m = base.clone();
        m.mode = LayoutMode::RawText;
        assert_ne!(m.fingerprint(), fp);

        let mut m = base.clone();
        m.columns.push("level".to_string());
        assert_ne!(m.fingerprint(), fp);

        let mut m = base.clone();
        m.template = "%{level}".to_string();
        assert_ne!(m.fingerprint(), fp);

        let mut m = base.clone();
        m.timestamp_field = "ts".to_string();
        assert_ne!(m.fingerprint(), fp);

        let mut m = base.clone();
        m.utc = true;
        assert_ne!(m.fingerprint(), fp);
    }
}
