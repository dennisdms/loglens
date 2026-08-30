//! A Hit rendered for display: the one seam the table, raw text mode, and
//! GREP all read through. See CONTEXT.md: Layout, Line, Part, Segment.

use iced::Color;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::es::Hit;

/// How a Result Tab draws its Hits. Both `columns` and `template` are always
/// present regardless of `mode`, so switching modes never discards the
/// other's settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
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
    pub segments: Vec<Segment>,
}

impl Part {
    /// The Part's text when it is a single unstyled run — i.e. no Highlight
    /// rule touched it. Lets the caller render it with a plain `text` widget
    /// (cheap `Auto` shaping) instead of `rich_text` (which forces the much
    /// slower `Advanced` shaping on every line). `None` once a rule has split
    /// or recoloured it, where `rich_text` over `segments` is required.
    pub fn plain(&self) -> Option<&str> {
        match self.segments.as_slice() {
            [only] if only.style == Style::default() => Some(&only.text),
            _ => None,
        }
    }
}

/// A run of text carrying one Style. A Part is a single Segment until a
/// Highlight rule splits it.
#[derive(Debug, Clone)]
pub struct Segment {
    pub text: String,
    pub style: Style,
}

impl Segment {
    /// Owned so the resulting Span, and anything built from it, doesn't borrow
    /// this Segment or the Line it came from — Lines are rebuilt on every
    /// render and dropped immediately after.
    pub fn to_span(&self) -> iced::widget::text::Span<'static> {
        iced::widget::text::Span::new(self.text.clone())
            .color_maybe(self.style.fg)
            .background_maybe(self.style.bg)
    }
}

/// What a Highlight rule can set on a Segment. Maps directly onto
/// `iced::widget::text::Span`'s `color` and `highlight` fields — see step 6.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Style {
    #[serde(default, with = "hex_color_opt")]
    pub fg: Option<Color>,
    #[serde(default, with = "hex_color_opt")]
    pub bg: Option<Color>,
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
}

// --- Highlight rules --------------------------------------------------------

/// A Highlight rule. Global to the application (`Config.rules`), never per
/// Saved Search.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    pub name: String,
    pub enabled: bool,
    pub matcher: Matcher,
    pub style: Style,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Matcher {
    /// Colors the whole Line.
    Field { path: String, op: Op, value: String },
    /// Colors only the matched text, within whichever Part it falls in.
    Text { pattern: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    Eq,
    Ne,
    Gt,
    Gte,
    Lt,
    Lte,
}

impl Op {
    pub const ALL: [Op; 6] = [Op::Eq, Op::Ne, Op::Gt, Op::Gte, Op::Lt, Op::Lte];
}

impl std::fmt::Display for Op {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Op::Eq => "==",
            Op::Ne => "!=",
            Op::Gt => ">",
            Op::Gte => ">=",
            Op::Lt => "<",
            Op::Lte => "<=",
        };
        f.write_str(s)
    }
}

/// The rule set, normalized once when it changes rather than re-normalized on
/// every render. Rebuild this only when `Config.rules` changes (a rule added,
/// edited, deleted, reordered, or toggled) — not per Hit, not per frame.
#[derive(Debug, Clone, Default)]
pub struct Prepared {
    rules: Vec<PreparedRule>,
}

#[derive(Debug, Clone)]
struct PreparedRule {
    matcher: PreparedMatcher,
    style: Style,
}

#[derive(Debug, Clone)]
enum PreparedMatcher {
    Field {
        path: String,
        op: Op,
        value: PreparedValue,
    },
    /// Lowercased once, so matching does a lowercase-vs-lowercase compare
    /// without re-lowercasing the pattern on every Hit.
    Text { pattern_lower: String },
}

/// A Field matcher's `value`, pre-parsed as a number where possible so
/// matching doesn't re-attempt the parse on every Hit.
#[derive(Debug, Clone)]
enum PreparedValue {
    Number(f64),
    Text(String),
}

impl Prepared {
    pub fn from_rules(rules: &[Rule]) -> Self {
        let rules = rules
            .iter()
            .filter(|r| r.enabled)
            .map(|r| PreparedRule {
                matcher: match &r.matcher {
                    Matcher::Field { path, op, value } => PreparedMatcher::Field {
                        path: path.clone(),
                        op: *op,
                        value: match value.parse::<f64>() {
                            Ok(n) => PreparedValue::Number(n),
                            Err(_) => PreparedValue::Text(value.to_lowercase()),
                        },
                    },
                    Matcher::Text { pattern } => PreparedMatcher::Text {
                        pattern_lower: pattern.to_lowercase(),
                    },
                },
                style: r.style,
            })
            .collect();
        Prepared { rules }
    }
}

// --- Render ----------------------------------------------------------------

/// Everything this module exposes: renders `hit` under `layout`, then applies
/// `rules` to the freshly-built Line.
pub fn render(hit: &Hit, layout: &Layout, rules: &Prepared) -> Line {
    let mut line = match layout.mode {
        LayoutMode::Table => render_table(hit, layout),
        LayoutMode::RawText => render_raw_text(hit, layout),
    };
    matching::apply(&mut line, hit, rules);
    line
}

fn render_table(hit: &Hit, layout: &Layout) -> Line {
    let parts = layout
        .columns
        .iter()
        .map(|col| {
            let text = cell_text(&hit.source, col, &layout.timestamp_field, layout.utc);
            Part {
                segments: vec![Segment {
                    text,
                    style: Style::default(),
                }],
            }
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
        parts: vec![Part {
            segments: vec![Segment {
                text,
                style: Style::default(),
            }],
        }],
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

// --- Hex colour serde ----------------------------------------------------

mod hex_color {
    use iced::Color;
    use serde::{Serialize, Serializer};

    pub fn serialize<S: Serializer>(color: &Color, s: S) -> Result<S::Ok, S::Error> {
        let [r, g, b, _a] = color.into_rgba8();
        format!("#{r:02x}{g:02x}{b:02x}").serialize(s)
    }

    /// `#rrggbb` → `Color`. `None` for anything else (bad prefix, wrong
    /// length, non-hex digits).
    pub fn parse_hex(s: &str) -> Option<Color> {
        let s = s.strip_prefix('#')?;
        if s.len() != 6 {
            return None;
        }
        let r = u8::from_str_radix(&s[0..2], 16).ok()?;
        let g = u8::from_str_radix(&s[2..4], 16).ok()?;
        let b = u8::from_str_radix(&s[4..6], 16).ok()?;
        Some(Color::from_rgb8(r, g, b))
    }
}

mod hex_color_opt {
    use super::hex_color;
    use iced::Color;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(c: &Option<Color>, s: S) -> Result<S::Ok, S::Error> {
        match c {
            Some(c) => hex_color::serialize(c, s),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Color>, D::Error> {
        Option::<String>::deserialize(d)?
            .map(|s| {
                hex_color::parse_hex(&s)
                    .ok_or_else(|| serde::de::Error::custom(format!("invalid color: {s}")))
            })
            .transpose()
    }
}

/// `#rrggbb` → `Color`; `None` for anything malformed. Used by the Highlight
/// rules modal's colour inputs (step 6).
pub fn parse_hex(s: &str) -> Option<Color> {
    hex_color::parse_hex(s)
}

// --- Matching ------------------------------------------------------------

mod matching {
    use super::*;

    /// Applies `prepared` to a freshly-rendered `Line` in place. Called once
    /// per Line, after the Table/RawText render arm has built it.
    ///
    /// Precedence: the *first* Field-matcher rule (in rule order) whose
    /// predicate matches the Hit sets the style for every Segment in the
    /// Line. Then, in rule order, every Text-matcher rule's pattern is
    /// searched for in each Segment's text and matching runs are split out
    /// into their own Segment with that rule's style layered on. Where two
    /// Text rules' matches would overlap, the earlier rule owns the
    /// overlapping text: once a rule has claimed a span as its own Segment,
    /// later rules do not re-split it.
    pub fn apply(line: &mut Line, hit: &Hit, prepared: &Prepared) {
        if prepared.rules.is_empty() {
            return;
        }

        // 1. First matching Field rule colours the whole Line.
        for rule in &prepared.rules {
            if let PreparedMatcher::Field { path, op, value } = &rule.matcher
                && field_predicate(hit, path, *op, value)
            {
                for part in &mut line.parts {
                    for seg in &mut part.segments {
                        seg.style = rule.style;
                    }
                }
                break;
            }
        }

        // 2. Text rules, in order. `claimed` tracks Segments a prior Text
        //    rule has already split out and restyled, so a later rule scans
        //    only what is left.
        for part in &mut line.parts {
            let mut claimed = vec![false; part.segments.len()];
            for rule in &prepared.rules {
                let PreparedMatcher::Text { pattern_lower } = &rule.matcher else {
                    continue;
                };
                if pattern_lower.is_empty() {
                    continue;
                }
                let mut next_segments: Vec<Segment> = Vec::new();
                let mut next_claimed: Vec<bool> = Vec::new();
                for (seg, was_claimed) in
                    std::mem::take(&mut part.segments).into_iter().zip(claimed)
                {
                    if was_claimed {
                        next_segments.push(seg);
                        next_claimed.push(true);
                        continue;
                    }
                    match split_segment(&seg, pattern_lower, &rule.style) {
                        Some(pieces) => {
                            for (piece, matched) in pieces {
                                next_segments.push(piece);
                                next_claimed.push(matched);
                            }
                        }
                        None => {
                            next_segments.push(seg);
                            next_claimed.push(false);
                        }
                    }
                }
                part.segments = next_segments;
                claimed = next_claimed;
            }
        }
    }

    /// Splits `seg` on every non-overlapping case-insensitive occurrence of
    /// `needle` (already lowercased). Matched pieces carry `seg.style` with
    /// the rule's `fg`/`bg` layered over it. `None` when there is no match.
    fn split_segment(
        seg: &Segment,
        needle: &str,
        rule_style: &Style,
    ) -> Option<Vec<(Segment, bool)>> {
        let ranges = match_ranges(&seg.text, needle);
        if ranges.is_empty() {
            return None;
        }
        let mut matched_style = seg.style;
        if rule_style.fg.is_some() {
            matched_style.fg = rule_style.fg;
        }
        if rule_style.bg.is_some() {
            matched_style.bg = rule_style.bg;
        }

        let mut out: Vec<(Segment, bool)> = Vec::new();
        let mut cursor = 0;
        for (start, end) in ranges {
            if start > cursor {
                out.push((
                    Segment {
                        text: seg.text[cursor..start].to_string(),
                        style: seg.style,
                    },
                    false,
                ));
            }
            out.push((
                Segment {
                    text: seg.text[start..end].to_string(),
                    style: matched_style,
                },
                true,
            ));
            cursor = end;
        }
        if cursor < seg.text.len() {
            out.push((
                Segment {
                    text: seg.text[cursor..].to_string(),
                    style: seg.style,
                },
                false,
            ));
        }
        Some(out)
    }

    /// Byte ranges in `haystack` of every non-overlapping case-insensitive
    /// match of `needle` (already lowercased). O(n*m), which is fine at
    /// log-line lengths.
    fn match_ranges(haystack: &str, needle: &str) -> Vec<(usize, usize)> {
        let mut ranges = Vec::new();
        if needle.is_empty() {
            return ranges;
        }
        let needle_chars: Vec<char> = needle.chars().collect();
        let indices: Vec<(usize, char)> = haystack.char_indices().collect();
        let mut i = 0;
        while i < indices.len() {
            let mut j = 0; // index into needle_chars
            let mut k = i; // index into indices
            let mut ok = true;
            while j < needle_chars.len() {
                let Some(&(_, hc)) = indices.get(k) else {
                    ok = false;
                    break;
                };
                for lc in hc.to_lowercase() {
                    if needle_chars.get(j) != Some(&lc) {
                        ok = false;
                        break;
                    }
                    j += 1;
                }
                if !ok {
                    break;
                }
                k += 1;
            }
            if ok && j == needle_chars.len() {
                let start = indices[i].0;
                let end = indices.get(k).map(|&(b, _)| b).unwrap_or(haystack.len());
                ranges.push((start, end));
                i = k.max(i + 1);
            } else {
                i += 1;
            }
        }
        ranges
    }

    fn field_predicate(hit: &Hit, path: &str, op: Op, value: &PreparedValue) -> bool {
        let resolved = resolve(&hit.source, path);
        match op {
            Op::Eq | Op::Ne => {
                let lhs = resolved
                    .map(render_value)
                    .unwrap_or_default()
                    .to_lowercase();
                let rhs = match value {
                    PreparedValue::Number(n) => n.to_string(),
                    PreparedValue::Text(t) => t.clone(),
                };
                let equal = lhs == rhs;
                if matches!(op, Op::Eq) { equal } else { !equal }
            }
            Op::Gt | Op::Gte | Op::Lt | Op::Lte => {
                let PreparedValue::Number(threshold) = value else {
                    return false;
                };
                let Some(lhs) = resolved.and_then(value_as_f64) else {
                    return false;
                };
                match op {
                    Op::Gt => lhs > *threshold,
                    Op::Gte => lhs >= *threshold,
                    Op::Lt => lhs < *threshold,
                    Op::Lte => lhs <= *threshold,
                    _ => unreachable!(),
                }
            }
        }
    }

    fn value_as_f64(v: &Value) -> Option<f64> {
        v.as_f64()
            .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
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

        fn fg(hex: &str) -> Style {
            Style {
                fg: parse_hex(hex),
                bg: None,
            }
        }

        fn bg(hex: &str) -> Style {
            Style {
                fg: None,
                bg: parse_hex(hex),
            }
        }

        fn one_part(text: &str) -> Line {
            Line {
                parts: vec![Part {
                    segments: vec![Segment {
                        text: text.to_string(),
                        style: Style::default(),
                    }],
                }],
            }
        }

        fn field_rule(path: &str, op: Op, value: &str, style: Style) -> Rule {
            Rule {
                name: "f".to_string(),
                enabled: true,
                matcher: Matcher::Field {
                    path: path.to_string(),
                    op,
                    value: value.to_string(),
                },
                style,
            }
        }

        fn text_rule(pattern: &str, style: Style) -> Rule {
            Rule {
                name: "t".to_string(),
                enabled: true,
                matcher: Matcher::Text {
                    pattern: pattern.to_string(),
                },
                style,
            }
        }

        #[test]
        fn field_eq_case_insensitive_colours_whole_line() {
            let prepared =
                Prepared::from_rules(&[field_rule("level", Op::Eq, "error", fg("#ff0000"))]);
            let mut line = Line {
                parts: vec![
                    Part {
                        segments: vec![Segment {
                            text: "a".into(),
                            style: Style::default(),
                        }],
                    },
                    Part {
                        segments: vec![Segment {
                            text: "b".into(),
                            style: Style::default(),
                        }],
                    },
                ],
            };
            matching::apply(&mut line, &hit(json!({ "level": "ERROR" })), &prepared);
            for part in &line.parts {
                for seg in &part.segments {
                    assert_eq!(seg.style, fg("#ff0000"));
                }
            }
        }

        #[test]
        fn field_gte_numeric_threshold() {
            let prepared =
                Prepared::from_rules(&[field_rule("status", Op::Gte, "500", fg("#ff0000"))]);

            let mut hi = one_part("x");
            matching::apply(&mut hi, &hit(json!({ "status": 500 })), &prepared);
            assert_eq!(hi.parts[0].segments[0].style, fg("#ff0000"));

            let mut lo = one_part("x");
            matching::apply(&mut lo, &hit(json!({ "status": 499 })), &prepared);
            assert_eq!(lo.parts[0].segments[0].style, Style::default());
        }

        #[test]
        fn field_gte_non_numeric_value_never_matches() {
            let prepared =
                Prepared::from_rules(&[field_rule("status", Op::Gte, "oops", fg("#ff0000"))]);
            let mut line = one_part("x");
            matching::apply(&mut line, &hit(json!({ "status": 9000 })), &prepared);
            assert_eq!(line.parts[0].segments[0].style, Style::default());
        }

        #[test]
        fn text_rule_splits_exactly_the_matched_run() {
            let prepared = Prepared::from_rules(&[text_rule("Timeout", bg("#ffff00"))]);
            let mut line = one_part("a timeout here");
            matching::apply(&mut line, &hit(json!({})), &prepared);
            let segs = &line.parts[0].segments;
            assert_eq!(
                segs.iter().map(|s| s.text.as_str()).collect::<Vec<_>>(),
                vec!["a ", "timeout", " here"]
            );
            assert_eq!(segs[0].style, Style::default());
            assert_eq!(segs[1].style, bg("#ffff00"));
            assert_eq!(segs[2].style, Style::default());
        }

        #[test]
        fn overlapping_text_rules_earlier_wins_the_overlap() {
            // "abcabc": rule 1 matches "bca", rule 2 matches "cab". Rule 1
            // claims [1,4); rule 2 then only sees "a" + "bc" and finds no
            // full "cab", so only rule 1's style lands.
            let prepared = Prepared::from_rules(&[
                text_rule("bca", fg("#111111")),
                text_rule("cab", fg("#222222")),
            ]);
            let mut line = one_part("abcabc");
            matching::apply(&mut line, &hit(json!({})), &prepared);
            let segs = &line.parts[0].segments;
            assert_eq!(
                segs.iter().map(|s| s.text.as_str()).collect::<Vec<_>>(),
                vec!["a", "bca", "bc"]
            );
            assert_eq!(segs[1].style, fg("#111111"));
            assert_eq!(segs[0].style, Style::default());
            assert_eq!(segs[2].style, Style::default());
        }

        #[test]
        fn disabled_rule_never_matches() {
            let mut rule = field_rule("level", Op::Eq, "error", fg("#ff0000"));
            rule.enabled = false;
            let prepared = Prepared::from_rules(&[rule]);
            let mut line = one_part("x");
            matching::apply(&mut line, &hit(json!({ "level": "error" })), &prepared);
            assert_eq!(line.parts[0].segments[0].style, Style::default());
        }

        #[test]
        fn no_rule_matches_keeps_default_style() {
            let prepared = Prepared::from_rules(&[
                field_rule("level", Op::Eq, "warn", fg("#ff0000")),
                text_rule("absent", bg("#00ff00")),
            ]);
            let mut line = one_part("nothing here");
            matching::apply(&mut line, &hit(json!({ "level": "info" })), &prepared);
            for seg in &line.parts[0].segments {
                assert_eq!(seg.style, Style::default());
            }
        }

        #[test]
        fn field_then_text_compose() {
            let prepared = Prepared::from_rules(&[
                field_rule("level", Op::Eq, "error", fg("#ff0000")),
                text_rule("timeout", bg("#ffff00")),
            ]);
            let mut line = one_part("request timeout reached");
            matching::apply(&mut line, &hit(json!({ "level": "error" })), &prepared);
            let segs = &line.parts[0].segments;
            assert_eq!(
                segs.iter().map(|s| s.text.as_str()).collect::<Vec<_>>(),
                vec!["request ", "timeout", " reached"]
            );
            // Field's fg survives on every piece; the matched piece also
            // picks up the Text rule's bg.
            assert_eq!(segs[0].style, fg("#ff0000"));
            assert_eq!(
                segs[1].style,
                Style {
                    fg: parse_hex("#ff0000"),
                    bg: parse_hex("#ffff00"),
                }
            );
            assert_eq!(segs[2].style, fg("#ff0000"));
        }
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
        line.parts
            .iter()
            .map(|p| {
                p.segments
                    .iter()
                    .map(|s| s.text.clone())
                    .collect::<String>()
            })
            .collect()
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
        let line = render(&hit(source), &layout, &Prepared::default());
        assert_eq!(only_text(&line), vec!["INFO", "hello"]);
    }

    // --- template parsing ---

    #[test]
    fn literal_only_template_renders_unchanged() {
        assert_eq!(
            parse_template("plain text"),
            vec![Piece::Literal("plain text".to_string())]
        );
        let line = render(
            &hit(json!({})),
            &raw_layout("plain text"),
            &Prepared::default(),
        );
        assert_eq!(only_text(&line), vec!["plain text"]);
    }

    #[test]
    fn single_placeholder_resolves() {
        let line = render(
            &hit(json!({ "message": "hi" })),
            &raw_layout("%{message}"),
            &Prepared::default(),
        );
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
            &Prepared::default(),
        );
        assert_eq!(only_text(&line), vec!["[] tail"]);
    }

    #[test]
    fn source_placeholder_renders_compact_json() {
        let line = render(
            &hit(json!({ "a": 1, "b": "x" })),
            &raw_layout("%{_source}"),
            &Prepared::default(),
        );
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

    // --- hex colour serde ---

    #[test]
    fn style_colour_round_trips_through_json() {
        let style = Style {
            fg: parse_hex("#e06c6c"),
            bg: parse_hex("#1a1a1a"),
        };
        let json = serde_json::to_string(&style).unwrap();
        let back: Style = serde_json::from_str(&json).unwrap();
        assert_eq!(style, back);
    }

    #[test]
    fn style_defaults_when_colours_absent() {
        let back: Style = serde_json::from_str("{}").unwrap();
        assert_eq!(back, Style::default());
    }

    #[test]
    fn malformed_hex_is_rejected_not_panicking() {
        assert!(parse_hex("not-a-color").is_none());
        assert!(parse_hex("#zz0000").is_none());
        assert!(parse_hex("#12345").is_none());
        let bad = serde_json::from_str::<Style>("{\"fg\":\"#12345\"}");
        assert!(bad.is_err());
    }
}
