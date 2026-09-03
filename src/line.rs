//! A Hit rendered for display: the one seam the table, raw text mode, and
//! GREP all read through. See CONTEXT.md: Layout, Line, Part.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::es::Hit;

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

/// Per-Result-Tab cache of rendered [`Line`]s, keyed by a Hit's position in
/// `tab.hits`. Every scroll frame re-renders the whole windowed row slice
/// otherwise (JSON resolution, multi-KB string cloning, timestamp formatting)
/// even though the window barely moves frame to frame — see
/// `docs/plans/wide-line-perf-followups.md` item 3.
///
/// Positional, not content-keyed: `tab.hits` positions are stable within a
/// loaded result set (paging appends; sort / refresh replace wholesale and
/// call [`LineCache::clear`]). A content hash would cost hashing multi-KB JSON
/// per row per frame and would over-report its hit rate against the
/// scroll-harness's identical-copy fixtures (see `docs/testing.md`).
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
}

/// Rows kept cached on each side of the live window. Comfortably more than a
/// single frame's scroll delta (even a fast fling), so rows staying on screen
/// across frames stay warm, while a long scroll can't grow the map without
/// bound.
const RETAIN: usize = 64;

impl LineCache {
    /// Call once per frame before [`get`](Self::get), with the row range about
    /// to be requested. Drops every entry on a [`Layout`] change; otherwise
    /// evicts entries far outside `window` so the map stays bounded.
    pub fn prepare(&mut self, layout: &Layout, window: (usize, usize)) {
        let key = layout.fingerprint();
        if key != self.key {
            self.lines.clear();
            self.max_line_bytes = 0;
            self.key = key;
            return;
        }
        let (start, end) = window;
        let lo = start.saturating_sub(RETAIN);
        let hi = end.saturating_add(RETAIN);
        self.lines.retain(|&index, _| (lo..hi).contains(&index));
    }

    /// The rendered `Line` for `hit` at `index`, rendering and caching it on a
    /// miss. Assumes [`prepare`](Self::prepare) ran this frame with a matching
    /// `layout`.
    pub fn get(&mut self, index: usize, hit: &Hit, layout: &Layout) -> &Line {
        if !self.lines.contains_key(&index) {
            let line = render(hit, layout);
            if layout.mode == LayoutMode::RawText {
                let bytes = line.parts.first().map_or(0, |p| p.text.len());
                self.max_line_bytes = self.max_line_bytes.max(bytes);
            }
            self.lines.insert(index, line);
        }
        &self.lines[&index]
    }

    /// The longest raw-text line, in bytes, rendered since the last [`Layout`]
    /// change. See [`max_line_bytes`](Self::max_line_bytes).
    pub fn max_line_bytes(&self) -> usize {
        self.max_line_bytes
    }

    /// Drops every entry. For the callers that replace or clear `tab.hits`
    /// wholesale, after which a positional key means something different.
    pub fn clear(&mut self) {
        self.lines.clear();
        self.max_line_bytes = 0;
    }
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
        Hit {
            source,
            sort: Vec::new(),
        }
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

    #[test]
    fn line_cache_serves_a_hit_without_re_rendering() {
        let layout = table_layout(&["message"], "@timestamp", false);
        let mut cache = LineCache::default();
        cache.prepare(&layout, (0, 1));

        let first = hit(json!({ "message": "original" }));
        assert_eq!(cache.get(0, &first, &layout).parts[0].text, "original");

        // A different Hit at the same position must not be re-rendered — the
        // cached Line stands until an explicit invalidation.
        let changed = hit(json!({ "message": "changed" }));
        assert_eq!(cache.get(0, &changed, &layout).parts[0].text, "original");
    }

    #[test]
    fn line_cache_prepare_drops_everything_on_layout_change() {
        let a = table_layout(&["message"], "@timestamp", false);
        let b = table_layout(&["level"], "@timestamp", false);
        let mut cache = LineCache::default();

        cache.prepare(&a, (0, 1));
        cache.get(0, &hit(json!({ "message": "m", "level": "INFO" })), &a);
        assert_eq!(cache.lines.len(), 1);

        cache.prepare(&b, (0, 1));
        assert!(cache.lines.is_empty());
        let line = cache.get(0, &hit(json!({ "message": "m", "level": "INFO" })), &b);
        assert_eq!(line.parts[0].text, "INFO");
    }

    #[test]
    fn line_cache_prepare_evicts_rows_far_from_the_window() {
        let layout = raw_layout("%{message}");
        let mut cache = LineCache::default();

        cache.prepare(&layout, (0, 200));
        for i in 0..200 {
            cache.get(i, &hit(json!({ "message": format!("row {i}") })), &layout);
        }
        assert_eq!(cache.lines.len(), 200);

        // Window has scrolled well past the early rows.
        cache.prepare(&layout, (500, 540));
        assert!(cache.lines.is_empty());

        cache.prepare(&layout, (100, 300));
        for i in 100..300 {
            cache.get(i, &hit(json!({ "message": format!("row {i}") })), &layout);
        }
        // Rows within RETAIN (64) of the window [130, 170) survive — i.e.
        // [66, 234) — the rest are evicted.
        cache.prepare(&layout, (130, 170));
        assert!(cache.lines.keys().all(|&i| (66..234).contains(&i)));
        assert!(cache.lines.contains_key(&120));
        assert!(cache.lines.contains_key(&233));
        assert!(!cache.lines.contains_key(&234));
        assert!(!cache.lines.contains_key(&280));
    }

    #[test]
    fn line_cache_tracks_widest_raw_line_and_resets_it() {
        let layout = raw_layout("%{message}");
        let mut cache = LineCache::default();
        cache.prepare(&layout, (0, 3));
        cache.get(0, &hit(json!({ "message": "short" })), &layout);
        cache.get(
            1,
            &hit(json!({ "message": "a much longer line here" })),
            &layout,
        );
        cache.get(2, &hit(json!({ "message": "mid" })), &layout);
        assert_eq!(cache.max_line_bytes(), "a much longer line here".len());

        // A Layout change clears the estimate along with the entries.
        let other = raw_layout("%{level}");
        cache.prepare(&other, (0, 1));
        assert_eq!(cache.max_line_bytes(), 0);

        // Table mode never touches it.
        let table = table_layout(&["message"], "@timestamp", false);
        cache.prepare(&table, (0, 1));
        cache.get(0, &hit(json!({ "message": "anything at all" })), &table);
        assert_eq!(cache.max_line_bytes(), 0);
    }

    #[test]
    fn line_cache_clear_drops_everything() {
        let layout = raw_layout("%{message}");
        let mut cache = LineCache::default();
        cache.prepare(&layout, (0, 2));
        cache.get(0, &hit(json!({ "message": "a" })), &layout);
        cache.get(1, &hit(json!({ "message": "b" })), &layout);
        cache.clear();
        assert!(cache.lines.is_empty());
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
