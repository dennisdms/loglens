# Implementation plan: the Line module

**Status:** approved, ready to implement.
**Origin:** architecture review candidate C1, worked through a full grilling
session. Every decision below is load-bearing — it was chosen over stated
alternatives for stated reasons. Do not silently substitute a different
choice because it seems simpler or more idiomatic; if something here turns
out to be wrong once you're in the code, stop and raise it rather than
deviating quietly.

## How to use this document

Work the steps in order. Each is a checkpoint: the code compiles, the tests
pass, and (from step 2 on) the app runs, before you move to the next one.
Steps 1–3 change no on-screen behavior — if the app looks different after
any of them, something has drifted from the plan.

Every subsection that introduces a type or function gives its exact
signature. Match it. Where a decision has a rationale that matters for
correctness (not just style), the rationale is included so you can verify
your implementation actually honors it, not just resembles it.

Terms **Layout**, **Line**, **Part**, **Segment**, **Highlight rule** are
defined in `CONTEXT.md`. Read that section before starting — it's the
vocabulary this document uses throughout.

## Non-negotiable constraints

These cut across every step. Violating one of them is a plan deviation even
if no single step's instructions explicitly repeat it.

- **No new dependencies.** No `regex`, no `serde_with`. Text-pattern matching
  is substring only (case-insensitive). Colors serialize as hex strings via a
  hand-written serde module (~20 lines), not a crate.
- **`Line` is never cached.** It is recomputed on every render. Only the
  *rules* are prepared once and reused (step 5's `Prepared`).
- **The table's row height stays fixed at `ROW_H` (22.0px).** Raw text mode
  reuses it unchanged. Do not introduce variable row heights or wrapping.
- **No template modifiers, no width, no alignment.** `%{field.path}` is the
  entire grammar. If a rendered line looks ragged, that's expected — the
  table exists for aligned columns; raw text mode is for reading log text as
  it is that isn't a design flaw to fix here.
- **`Layout` is a struct, not an enum.** Switching between Table and RawText
  must never discard the other mode's settings (`columns` or `template`).
- **Rules are global** (`Config.rules`), not per Saved Search. `Layout` is
  per Saved Search (`SavedSearch.mode` / `.columns` / `.template`).
- **No migration function.** Every new field on `SavedSearch` and `Config`
  must be `#[serde(default)]` so old config files load unchanged. If you find
  yourself writing something like `migrate_legacy_layout`, stop — the plan
  was specifically designed to avoid needing one.

---

## Step 1 — Land `src/line.rs` with Columns rendering

**Visible change: none.** This step is a pure extraction; the app must look
and behave identically before and after.

### 1.1 Create the module

New file `src/line.rs`. Add `mod line;` to `src/main.rs`'s module list
(alongside the existing `mod results;` etc., alphabetically).

### 1.2 Public types

```rust
//! A Hit rendered for display: the one seam the table, raw text mode, and
//! GREP all read through. See CONTEXT.md: Layout, Line, Part, Segment.

use iced::Color;
use serde::{Deserialize, Serialize};
use serde_json::Value;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutMode {
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

/// A run of text carrying one Style. A Part is a single Segment until a
/// Highlight rule (step 5) splits it.
#[derive(Debug, Clone)]
pub struct Segment {
    pub text: String,
    pub style: Style,
}

/// What a Highlight rule can set on a Segment. Maps directly onto
/// `iced::widget::text::Span`'s `color` and `highlight` fields — see step 6.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Style {
    pub fg: Option<Color>,
    pub bg: Option<Color>,
}
```

Notes on naming, both deliberate:

- **`Segment`, not `Span`.** `iced::widget::text::Span` is in scope wherever
  this module's output gets rendered (step 6). Naming ours `Segment` avoids
  the collision entirely rather than requiring `as` renames at every call
  site.
- **`Line` derives `Default`, not `Serialize`.** It is never persisted; it's
  a per-render value. Only `Layout` and (step 5) `Rule`/`Style`'s color
  fields are persisted.

`#[serde(skip)]` on `timestamp_field`/`utc` means they don't round-trip
through JSON at all — `Layout::deserialize` will leave them at their `Default`
impl's values (empty string / `false`). That's fine: nothing deserializes a
`Layout` directly. `SavedSearch` (step 4) persists `mode`/`columns`/`template`
as three flat fields and a call site assembles the full `Layout` from those
plus the two runtime values. Don't derive `Deserialize` expecting to load a
whole `Layout` from disk — that isn't how it's stored.

### 1.3 The render entry point

```rust
/// Everything this module exposes. `rules` is accepted starting this step
/// but not yet applied — that lands in step 5. Every Segment gets
/// `Style::default()` until then.
pub fn render(hit: &Hit, layout: &Layout, rules: &Prepared) -> Line
```

For step 1, stub `Prepared` as an empty marker type so the signature is
final from the start and step 5 doesn't have to touch every call site:

```rust
/// Placeholder until step 5. Becomes the prepared, ready-to-match form of
/// `Config.rules`.
#[derive(Debug, Clone, Default)]
pub struct Prepared;
```

`Hit` here is `crate::es::Hit` — import it. `render` only handles
`LayoutMode::Table` in this step; `LayoutMode::RawText` is unreachable until
step 3 (leave a `todo!("step 3")` arm, or an explicit `unimplemented!` — pick
whichever this codebase's existing style favors; check for precedent before
choosing).

### 1.4 Absorb `cell()` and its helpers

Move from `src/results.rs` (currently lines 408–478) into `src/line.rs` as
**private** functions, unchanged in logic:

- `resolve` (dotted-path lookup, falling back to a literal dotted key)
- `render_value` (null/bool/number/string/array/object → display string)
- `format_timestamp_str`, `format_timestamp_millis`, `format_dt`

Do not make any of these `pub`. Per the deletion test: nothing outside this
module needs one cell's text once the table renders through `Part`s — if you
find a call site elsewhere that seems to need one, that's a sign something
is reaching past the interface, not a reason to make the helper public.

The public `cell()` function itself is replaced by the Table arm of
`render()`:

```rust
fn render_table(hit: &Hit, layout: &Layout) -> Line {
    let parts = layout
        .columns
        .iter()
        .map(|col| {
            let text = cell_text(&hit.source, col, &layout.timestamp_field, layout.utc);
            Part { segments: vec![Segment { text, style: Style::default() }] }
        })
        .collect();
    Line { parts }
}
```

(`cell_text` is `cell()` renamed private — same body.)

### 1.5 Tests (inline, `#[cfg(test)] mod tests` at the bottom of `line.rs`)

Write these now, against the Table path only:

- dotted path resolves through nested objects (e.g. `"a.b.c"` against
  `{"a":{"b":{"c":1}}}`)
- dotted path falls back to a literal key containing dots when no nested
  match exists
- missing field → empty string
- null field → empty string
- array of strings joins with `", "`
- object renders as compact JSON
- timestamp field, ISO-8601 string, local time
- timestamp field, ISO-8601 string, UTC
- timestamp field, epoch millis, local and UTC
- non-timestamp field that happens to hold a date-like string is *not*
  reformatted (only the field matching `layout.timestamp_field` gets
  timestamp handling)

These are the first tests in the repository. Match the existing code's
naming conventions for test functions (check `src/config.rs` and
`src/es/mod.rs` — despite having no `#[test]` items today, check whether any
sibling Rust project the author maintains has a convention; if none is
discoverable, use `snake_case` names describing the behavior, e.g.
`missing_field_renders_empty`).

### 1.6 Checkpoint

`cargo build` succeeds. `cargo test` runs and passes the new suite. The app
is untouched — `results::cell` still exists and is still what the table
uses. (Step 2 removes it.) Do not proceed until this compiles cleanly with
no clippy warnings (`cargo clippy --all-targets --all-features -- -D
warnings`, matching the pre-commit hook).

---

## Step 2 — Point the table at `Line`

**Visible change: none.** The table must render pixel-identically.

### 2.1 Update `hit_table`

In `src/main.rs`, the row-building loop inside `hit_table` (currently line
2990, inside the `for (offset, hit) in tab.hits[start..end]...` loop)
currently calls:

```rust
let value = results::cell(&hit.source, col, &tab.timestamp_field, tab.utc);
```

Replace the per-column call with one `line::render` call per Hit (not per
cell — call it once per row, before iterating columns, and index into
`line.parts`):

```rust
let rendered = line::render(hit, &layout_for(tab), &Prepared::default());
// ... inside the per-column map:
let value = rendered.parts[i].segments[0].text.clone();
```

You'll need a `layout_for(tab: &ResultTab) -> line::Layout` helper (a free
function in `main.rs`, or a method — match whatever `ResultTab` already
does for similar assembly, e.g. how `effective_sort()` is a method) that
builds:

```rust
line::Layout {
    mode: line::LayoutMode::Table,
    columns: tab.columns.clone(),
    template: String::new(), // unused in Table mode
    timestamp_field: tab.timestamp_field.clone(),
    utc: tab.utc,
}
```

This step renders every visible row's full `Line` even though it only reads
`segments[0].text` — that's expected and matches the "no caching" constraint.
Don't try to optimize this into reading fields directly; the point of this
step is that the table goes through the same seam raw text mode will use in
step 4.

### 2.2 Delete `results::cell` and its helpers

Remove `cell()`, `resolve()`, `render_value()`, `format_timestamp_str()`,
`format_timestamp_millis()`, `format_dt()` from `src/results.rs` entirely —
they now live only in `line.rs`. `src/results.rs` should afterward contain
only `TimeframeDraft` and `ResultTab` and their impls (plus the constants:
`DETAIL_DEFAULT_H`, `DETAIL_MIN_H`, `DETAIL_MAX_H`, `ROW_H`, `COL_DEFAULT_W`,
`COL_TIMESTAMP_W`, `COL_MIN_W`, `COL_MAX_W`, `RETENTION_CAP`,
`WINDOW_BUFFER`, `RunState`, `TotalHits`, `Paging`).

Remove the now-unused `use chrono::{...}` and `use serde_json::Value` from
`results.rs` if nothing else in the file needs them — check before deleting.

### 2.3 Checkpoint

`cargo build`, `cargo clippy -D warnings`, `cargo test` all pass. Run the app
against a real or dockerized Elasticsearch (see the repo's docker setup) and
confirm the table renders identically to before this step — same text, same
formatting, same column widths. This is the step most likely to introduce a
subtle regression (an off-by-one in which Part maps to which column); check
it by eye before moving on.

---

## Step 3 — Parse the template

**Visible change: none.** `LayoutMode::RawText` becomes renderable but
nothing in the UI can select it yet (that's step 4).

### 3.1 Grammar

`%{field.path}` is the entire syntax. Precisely:

- `%{` starts a placeholder, `}` ends it.
- Everything between them is a dotted field path, resolved the same way
  `resolve()` (from step 1) resolves table columns — including its
  literal-dotted-key fallback.
- A missing or null field renders as empty string (consistent with Table
  mode — this was decided explicitly to keep the two modes' "missing data"
  behavior identical).
- An unclosed `%{` (no matching `}` before the template ends) is **not** an
  error — treat the literal `%{` and everything after it as ordinary text.
- **`|` inside a placeholder is invalid** and the whole placeholder — from
  `%{` to the next `}` — is treated as literal text, not as a partially-parsed
  field. This is a deliberate reservation for a future modifier syntax (not
  built here); it's also what catches a pasted Logstash/grok pattern like
  `%{LOGLEVEL:level}` — wait, that uses `:` not `|`. Re-read: grok syntax
  uses `:`, and `:` is a **legal character in a field path** in this grammar
  (nothing forbids it), so `%{LOGLEVEL:level}` parses as a literal attempt to
  resolve a field literally named `LOGLEVEL:level`, which won't exist, so it
  silently renders empty. That silent-empty case is intentional (see 3.1's
  "missing renders empty") and is specifically why step 4 adds edit-time
  validation — don't try to detect grok syntax here in the parser; that's a
  UI concern, not a grammar concern.

### 3.2 Implementation

Write a private parser, e.g.:

```rust
/// One piece of a parsed template: either literal text or a field
/// placeholder to resolve against a Hit.
enum Piece {
    Literal(String),
    Field(String),
}

fn parse_template(template: &str) -> Vec<Piece> { ... }
```

A straightforward single-pass scanner is sufficient — this is not a case
that needs a parser combinator or external crate. Walk the string, buffer
literal text until `%{` is found, then scan to the matching `}` (no nested
`%{` inside a placeholder — the first `}` closes it); if a `|` appears before
that `}`, or no `}` is found at all, treat the buffered `%{...` span as
literal and continue scanning from just past the `%{`.

### 3.3 Render arm

```rust
fn render_raw_text(hit: &Hit, layout: &Layout) -> Line {
    let pieces = parse_template(&layout.template);
    let text: String = pieces
        .into_iter()
        .map(|piece| match piece {
            Piece::Literal(s) => s,
            Piece::Field(path) => cell_text(&hit.source, &path, &layout.timestamp_field, layout.utc),
        })
        .collect();
    Line { parts: vec![Part { segments: vec![Segment { text, style: Style::default() }] }] }
}
```

Wire this into `render()`'s `LayoutMode::RawText` arm, replacing the
`todo!`/`unimplemented!` from step 1.

### 3.4 Default template

```rust
impl Layout {
    /// `%{message}` when the Target has a `message` field, otherwise
    /// compact `_source` JSON. Decided once, when the Layout needs a
    /// default — never per line, and never re-derived on every render.
    pub fn default_template(all_fields: &[String]) -> String {
        if all_fields.iter().any(|f| f == "message") {
            "%{message}".to_string()
        } else {
            // Compact JSON of the whole document. There is no field
            // placeholder for "the whole source" in this grammar — encode
            // it as a sentinel the render path special-cases, OR (simpler,
            // preferred): make this literally the placeholder for a
            // reserved path, e.g. "%{_source}", and have `cell_text`/
            // `resolve` treat the path "_source" as "return the whole
            // Value", not a field lookup. Implement it the second way —
            // it keeps render_raw_text ignorant of this special case.
            "%{_source}".to_string()
        }
    }
}
```

Pick the `"_source"` sentinel approach explicitly called out above, not an
ad hoc branch in `render_raw_text` — it keeps the "one call renders one
template" property intact and means a user who deletes the default and
types their own `%{_source}` gets the same behavior back, which is the
correct, unsurprising outcome.

In `resolve()` / `cell_text()`, add: if `path == "_source"`, return the
Hit's whole `source: &Value` rendered the same way `render_value` renders an
object (compact JSON) — check for this **before** the dotted-path split, so
a real field literally named `_source` (unlikely, but Elasticsearch reserves
leading-underscore names, so this is actually safe) isn't shadowed
incorrectly. Elasticsearch itself reserves `_source` as a meta-field, so
there is no real ambiguity here.

### 3.5 Tests

Add to `line.rs`'s test module:

- literal-only template renders unchanged
- single placeholder resolves
- placeholder adjacent to literal text on both sides
- unclosed `%{` at end of string renders as literal
- `|` inside a placeholder renders the whole `%{...}` span as literal
- a placeholder whose path doesn't resolve renders empty, leaving
  surrounding literal text intact
- `%{_source}` renders compact JSON of the whole `_source`
- `default_template` picks `%{message}` when `"message"` is in the field
  list, and `%{_source}` when it isn't

### 3.6 Checkpoint

`cargo test` passes including the new template suite. No UI change yet.

---

## Step 4 — Raw text mode in the UI

**Visible change: yes.** This is the first step where the seam becomes real
— both adapters (Table, RawText) exist and are reachable.

### 4.1 Persist `Layout` on `SavedSearch`

In `src/config.rs`, add two fields to `SavedSearch` (currently lines
36–56), next to the existing `columns` field:

```rust
/// Table or raw text. Defaults to Table so every existing config loads
/// unchanged.
#[serde(default)]
pub mode: crate::line::LayoutMode,
/// Raw text mode's template. Empty means "not yet set" — the app computes
/// a default from field caps the first time raw text mode is entered; see
/// `Layout::default_template`. An empty string is never rendered directly.
#[serde(default)]
pub template: String,
```

`LayoutMode` needs a `Default` impl (`Table`) for `#[serde(default)]` to
work on the `mode` field — add `#[derive(Default)]` with `#[default]` on the
`Table` variant in `line.rs`, or implement `Default` by hand; match whatever
style `config.rs`'s other enums use (`Auth` uses `#[derive(Default)]` +
`#[default]` — follow that precedent).

**Do not** nest these under a `layout: Layout` sub-object in the serialized
form, even though that mirrors the in-memory type more closely. Flat fields
on `SavedSearch` mean no migration function is needed — every existing
`config.json` on disk already has no `mode`/`template` keys and will pick up
the `#[serde(default)]` values. A nested `layout` object would need the same
`#[serde(default)]` treatment on the whole struct to work identically, but
flat fields are simpler and match how `columns`, `sort`, and
`timestamp_field` are already stored on this same struct — consistency with
sibling fields on the same struct, not just correctness, is why flat wins
here.

### 4.2 Assembling `Layout` for a Result Tab

Wherever `layout_for()` was written in step 2, extend it to read `mode` and
`template` from the tab (which in turn got them from `SavedSearch` when the
tab was opened — check `open_result_tab` in `main.rs` around line 1463 for
where other `SavedSearch` fields are copied onto `ResultTab`, and add `mode`
and `template` fields to `ResultTab` the same way `columns` already is one).

```rust
line::Layout {
    mode: tab.mode,
    columns: tab.columns.clone(),
    template: tab.template.clone(),
    timestamp_field: tab.timestamp_field.clone(),
    utc: tab.utc,
}
```

When a tab is first opened and `saved.template` is empty, resolve it via
`Layout::default_template(&tab.all_fields)` before storing it on the tab —
but only if `all_fields` is already populated at that point; if it isn't
(field caps still loading), leave `template` empty and resolve it lazily the
first time the raw text view is asked to render (or when `ResultFieldsLoaded`
lands — pick whichever integrates more naturally with the existing
`all_fields`-population flow around line 740; do not add a second, separate
loading state for this).

### 4.3 The raw text view

New function in `src/main.rs`, parallel to `hit_table`:

```rust
fn raw_text_view<'a>(&'a self, tab: &'a ResultTab) -> Element<'a, Message>
```

Structure:

- Same windowed-slice approach as `hit_table` (`tab.row_window()`, spacer
  elements above/below the visible slice) — reuse it as-is, do not
  reimplement windowing.
- Each visible Hit becomes one row of fixed height `ROW_H`, containing the
  rendered `Line`'s single Part's text.
- The row's text widget must not wrap (`text::Wrapping::None`, matching
  what table cells already use) and the row's container must allow
  horizontal overflow — check what scrollable/container combination in this
  codebase already handles unclipped horizontal content (the table's
  `scrollable` is vertical-only today; you likely need the row content
  inside a `scrollable` with horizontal direction enabled, or a container
  with `clip(false)` — consult iced 0.14's `scrollable::Direction` API
  before choosing; do not guess at a solution that silently clips or wraps).
- No column headers, no per-column resize grips, no header menu — none of
  that machinery applies to raw text mode.
- Row selection / click-to-open-detail behavior should carry over
  unchanged: clicking a row still calls `Message::HitClicked` and opens the
  same Hit-detail panel that Table mode uses (this is a "non-decision" from
  the grilling session — nothing about raw text mode implies the detail
  panel should behave differently).

Wire it into `result_view` (`src/main.rs` ~line 2609): where it currently
unconditionally calls `self.hit_table(tab)`, branch on `tab.mode`:

```rust
_ if tab.table_visible() => match tab.mode {
    line::LayoutMode::Table => self.hit_table(tab),
    line::LayoutMode::RawText => self.raw_text_view(tab),
},
```

### 4.4 The mode toggle

In the options strip (`options_bar`, `src/main.rs` ~line 2159, which
currently returns `self.result_sort_bar(tab)` directly), add a control next
to the existing "Sort fields" button — a two-state toggle or a small
segmented control for Table / Raw text. Follow the visual language of the
existing "Sort fields" button (`result_sort_bar`, ~line 2691) rather than
inventing a new control style. New `Message` variant:

```rust
ResultLayoutMode(u64, line::LayoutMode),
```

handled in `update` by setting `tab.mode`, calling
`self.sync_saved_from_result(run_id)` (existing helper, persists +
re-saves config — do not re-run the search; switching display mode doesn't
need a new Elasticsearch query, only a re-render, which happens automatically
since it's driven by `tab.mode` in the view).

### 4.5 The template input and its validation

When `tab.mode == RawText`, the Search bar area (or a new small strip below
it — match whichever placement reads more naturally once you see it
rendered; the options strip itself is for view-option *toggles*, this is a
free-text input, so it likely belongs where the query-string input already
lives, conditionally shown) needs a text input bound to `tab.template`, with
a `Message::ResultTemplateDraft(u64, String)` /
`Message::ResultTemplateSubmit(u64)` pair mirroring the existing
`ResultQueryDraft`/`ResultQuerySubmit` pattern exactly (draft field, commit
on Enter, `sync_saved_from_result` + no re-run needed since this doesn't
change the query).

**Validation, shown beside the input, not blocking submission:**

- Parse the draft with `parse_template` (step 3).
- Collect every `Piece::Field(path)` where `path != "_source"`.
- For each, check membership in `tab.all_fields`.
- If any are absent, show a warning (not an error — styling consistent with
  how `tab.target_error` is shown as a dismissible notice elsewhere, but
  this is inline beside the input, not in the bottom info bar) listing the
  unrecognized path(s), e.g. `Unknown field: "LOGLEVEL:level"`.
- This check re-runs on every keystroke against the draft (cheap: field
  list membership check over a handful of placeholders), not only on
  submit.

Do not block Enter/submit on validation failing — a pattern Target (e.g.
`logs-*`) can legitimately lack a field in some matching indices even
though the template is correct, which is exactly why this is a warning, not
a hard error (decided explicitly in the grilling session).

### 4.6 Tests

UI wiring isn't unit-testable in this codebase's current style (no existing
precedent for testing `view` functions) — skip tests for 4.3/4.4/4.5's
wiring itself. Do add one more `line.rs` test if not already covered:
`default_template` behavior when called with an empty vs non-empty
`all_fields` (already listed in 3.5 — confirm it's there before skipping).

### 4.7 Checkpoint

Run the app. Open a Saved Search, flip to raw text mode, confirm:
correct default template, editable template with live validation, no
row-height regression in Table mode, switching back to Table mode preserves
`columns` exactly as they were. Switching *to* raw text and back to Table
must not have mutated `columns`, and switching to raw text and typing a
template must not mutate `columns` either — this is the concrete test of
the "Layout is a struct, not an enum" decision; if you find columns
disappearing after a mode round-trip, something has regressed that
decision.

---

## Step 5 — Highlight rules: model and matching

**Visible change: none yet.** Rules can be constructed and matched in code,
but nothing in the UI creates one (that's step 6).

### 5.1 Hex color serde

`iced::Color` doesn't implement `Serialize`/`Deserialize`. Write a private
serde module in `line.rs`:

```rust
mod hex_color {
    use iced::Color;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(color: &Color, s: S) -> Result<S::Ok, S::Error> {
        let [r, g, b, _a] = color.into_rgba8();
        format!("#{r:02x}{g:02x}{b:02x}").serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Color, D::Error> {
        let s = String::deserialize(d)?;
        parse_hex(&s).ok_or_else(|| serde::de::Error::custom(format!("invalid color: {s}")))
    }

    pub fn parse_hex(s: &str) -> Option<Color> {
        let s = s.strip_prefix('#')?;
        if s.len() != 6 { return None; }
        let r = u8::from_str_radix(&s[0..2], 16).ok()?;
        let g = u8::from_str_radix(&s[2..4], 16).ok()?;
        let b = u8::from_str_radix(&s[4..6], 16).ok()?;
        Some(Color::from_rgb8(r, g, b))
    }
}
```

`Style.fg`/`Style.bg` are `Option<Color>` — `#[serde(with = "hex_color")]`
does not directly support `Option<T>`. Write a second small module (or a
generic pair of functions parameterized appropriately) specifically for
`Option<Color>`:

```rust
mod hex_color_opt {
    use super::hex_color;
    use iced::Color;
    use serde::{Deserializer, Serializer};

    pub fn serialize<S: Serializer>(c: &Option<Color>, s: S) -> Result<S::Ok, S::Error> {
        match c {
            Some(c) => hex_color::serialize(c, s),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Color>, D::Error> {
        Option::<String>::deserialize(d)?
            .map(|s| hex_color::parse_hex(&s).ok_or_else(|| serde::de::Error::custom(format!("invalid color: {s}"))))
            .transpose()
    }
}
```

Adjust `Style` to use it:

```rust
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Style {
    #[serde(default, with = "hex_color_opt")]
    pub fg: Option<Color>,
    #[serde(default, with = "hex_color_opt")]
    pub bg: Option<Color>,
}
```

Test this module directly: round-trip a known color through
serialize→deserialize, and confirm malformed input (`"not-a-color"`, `"#zz"`,
`"#12345"` — wrong length) returns an error rather than panicking.

### 5.2 Rule types

```rust
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
```

This mirrors `Timeframe`'s and `Auth`'s existing `#[serde(tag = "kind", ...)]`
pattern in `config.rs` — match that style exactly, including
`rename_all = "snake_case"`, for consistency with the rest of the config
schema.

### 5.3 `Config` field

In `src/config.rs`, add to `Config` (currently lines 10–18), beside
`utc_timestamps`:

```rust
/// Highlight rules, applied globally across every Result Tab.
#[serde(default)]
pub rules: Vec<crate::line::Rule>,
```

Empty `Vec` is the correct default — old configs load with no rules, no
migration needed.

### 5.4 `Prepared`

Replace the step-1 placeholder:

```rust
/// The rule set, normalized once when it changes rather than re-normalized
/// on every render. Rebuild this only when `Config.rules` changes (a rule
/// added, edited, deleted, reordered, or toggled) — not per Hit, not per
/// frame.
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
    Field { path: String, op: Op, value: PreparedValue },
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
```

Disabled rules are filtered out here, once, rather than checked on every
match attempt — `enabled` never needs to be inspected again downstream of
`from_rules`.

### 5.5 Matching, as a private submodule

```rust
mod matching {
    use super::*;

    /// Applies `prepared` to a freshly-rendered `Line` in place. Called once
    /// per Line, after the Table/RawText render arm has built it.
    ///
    /// Precedence (settled in the design session, do not change without
    /// re-confirming): the *first* Field-matcher rule (in rule order) whose
    /// predicate matches the Hit sets the style for every Segment in the
    /// Line. Then, in rule order, every Text-matcher rule's pattern is
    /// searched for in each Segment's text and matching runs are split out
    /// into their own Segment with that rule's style. Where two Text rules'
    /// matches would overlap, the earlier rule (lower index in `rules`)
    /// owns the overlapping text — do not attempt to layer both rules'
    /// styles on the same text.
    pub fn apply(line: &mut Line, hit: &Hit, prepared: &Prepared) { ... }
}
```

Implementation sketch (fill in fully, this is the part with the most actual
logic in the module — write real tests against it, listed below, before
considering it done):

1. Iterate `prepared.rules`, find the first `PreparedMatcher::Field` whose
   predicate evaluates true against `hit.source` (resolve `path` the same
   way `cell_text`/`resolve` does; compare per `op` — numeric compare if
   both the resolved field and `PreparedValue` are numeric, otherwise
   case-insensitive string compare, per decision 5.3 from the design
   session). If found, set every Segment in every Part of `line` to that
   rule's `style`.
2. Then, for each `PreparedMatcher::Text` rule in order, for each Part in
   `line`, for each existing Segment in that Part: find all non-overlapping
   case-insensitive occurrences of `pattern_lower` in the Segment's text,
   split the Segment into (unmatched, matched, unmatched, ...) pieces, and
   apply the rule's `style` only to the matched pieces. **Do this against
   the Segment's *current* text, which may already be a sub-piece from a
   previous Text rule's split** — this is how "earlier rule owns the
   overlap" falls out naturally: once step N's rule has claimed a span of
   text as its own Segment, step N+1's rule searches only within whatever
   Segments remain, and a Segment that a prior rule already claimed and
   restyled is not re-split by a later rule. (Confirm this is actually true
   of your split algorithm with the "overlapping rules" test below — if
   your implementation instead re-scans the *original* full text at every
   rule and only resolves conflicts afterward, that is a different,
   more complex algorithm than what's specified; use the simpler
   scan-the-current-segments approach.)
3. Numeric comparison for `Op::Gt`/`Gte`/`Lt`/`Lte` against a
   `PreparedValue::Text` (i.e., the rule's value didn't parse as a number)
   should not match anything — a magnitude comparison against a
   non-numeric value is always false, not an error and not a panic.
4. `Op::Eq`/`Op::Ne` on two `PreparedValue::Text` values compares the
   resolved field's string form (via the same rendering `render_value`
   already does — do not require an exact `Value::String` match; a numeric
   field compared with `Eq` against a text value should still compare by
   string, case-insensitively) against `value.to_lowercase()` — actually,
   simplify: resolve the field's value, render it to a string the same way
   `render_value` would, lowercase both sides, compare.

### 5.6 Wire `Prepared` into callers

`render()`'s signature already takes `&Prepared` since step 1 — update its
body (both the Table and RawText arms) to call `matching::apply(&mut line,
hit, rules)` after building `line`, before returning it.

Every call site that currently passes `&Prepared::default()` (from step 2's
`hit_table`, step 4's `raw_text_view`) needs to instead pass a real
`Prepared` built from `self.config.rules`. Build it once per `view()` call
(or cache it on `LogLens` and rebuild only when `config.rules` changes —
either is acceptable; building it fresh per `view()` call is simpler and
`Prepared::from_rules` is cheap enough at the rule-list sizes this app will
see that premature caching isn't warranted here — but do not rebuild it
once *per Hit* inside the render loop, that would defeat the entire purpose
of `Prepared` existing separately from `Rule`).

### 5.7 Tests

In `line.rs`'s test module (or a nested `mod matching { mod tests { ... } }`
colocated with the `matching` module — match whichever nesting style reads
more naturally once written):

- `Field` rule with `Eq`, case-insensitive, matches and sets whole-Line style
- `Field` rule with `Gte` on a numeric field, matches and doesn't match
  correctly on either side of the threshold
- `Field` rule with `Gte` where the rule's value doesn't parse as a number
  — never matches
- `Text` rule matches a substring case-insensitively and splits exactly the
  matched run into its own Segment
- two `Text` rules with overlapping patterns — confirm the earlier rule's
  style wins on the overlapping text and the later rule's style applies only
  to its non-overlapping match, if any
- a disabled rule (`enabled: false`) never matches, even if the predicate
  would otherwise be true
- when no rule matches, every Segment keeps `Style::default()`
- a `Field` rule matches before any `Text` rule is applied, and a `Text`
  rule can still further split a Segment that a `Field` rule already
  recolored (confirm the two rule kinds compose rather than one clobbering
  the other's work entirely)

### 5.8 Checkpoint

`cargo test` passes including the full matching suite. No UI change yet —
`Config.rules` is always empty because nothing can populate it (step 6).

---

## Step 6 — Rules on screen, and the authoring modal

**Visible change: yes.** This is the step that makes highlighting real.

### 6.1 Segment → iced Span conversion

```rust
impl Segment {
    /// Owned so the resulting Span, and anything built from it, doesn't
    /// borrow this Segment or the Line it came from. `Line`s are rebuilt on
    /// every render and dropped immediately after use — the Element handed
    /// to iced must own its text independently of that.
    pub fn to_span(&self) -> iced::widget::text::Span<'static> {
        iced::widget::text::Span::new(self.text.clone())
            .color_maybe(self.style.fg)
            // `highlight` takes a `text::Highlight { background, border }`,
            // not a bare Color — construct one with a zero-width border if
            // only a background color is set. Check `text::Highlight`'s
            // actual field names in iced_core before writing this; do not
            // guess at the border field's shape.
    }
}
```

Verify `iced_core::text::Span`'s exact builder methods before writing this
(`color`, `color_maybe`, and whatever sets `highlight` — grep the
`iced_core` and `iced_widget` source under `~/.cargo/registry` for the
installed `0.14.0`/`0.14.2` versions rather than assuming an API from
memory or from a different iced version). `color_maybe(Option<Color>)`
exists — confirmed during the design session; the `highlight` setter's
exact name was not.

### 6.2 Update `hit_table`

Change each cell's content from a plain `text(value)` to a `rich_text`
built from the corresponding Part's Segments:

```rust
let spans: Vec<_> = rendered.parts[i].segments.iter().map(Segment::to_span).collect();
iced::widget::rich_text(spans)
    .size(12.0)
    .font(Font::MONOSPACE)
```

matching the existing cell text's size/font (currently set on the plain
`text()` call around line 2985 — carry those settings over exactly, don't
drop them).

### 6.3 Update `raw_text_view`

Same conversion, applied to the single Part's Segments per row.

### 6.4 The Highlight rules modal

New menu item under the existing `menu_bar()`'s View menu (check
`menu_bar`, `src/main.rs` ~line 2140, for the existing menu structure and
match its pattern) — "Highlight rules…". Opens a new modal, following the
existing modal patterns (`connection_form_modal`, `search_settings_modal`
around lines 3285/2566 — use `modal_card` the same way they do, and stack it
in `view()`'s `layers` the same way `connection_form` and `search_settings`
are conditionally pushed).

State: a new `Option<RulesForm>` field on `LogLens` (parallel to
`connection_form`/`search_settings`), where `RulesForm` holds a working
copy of `config.rules` plus whatever transient editing state a single
rule-being-edited needs (name, matcher kind + fields, style color pickers).
Design this form's exact shape yourself, following the existing
`ConnectionForm`/`SearchForm` structs in `src/connection.rs`/`src/search.rs`
as the precedent for how this codebase shapes an editable form backing a
modal — don't invent a structurally different pattern for this one form.

Minimum required UI:

- List of existing rules, each showing name, enabled checkbox, a delete
  button, up/down reorder controls (rule order is meaningful — see 5.5's
  precedence rule).
- An add/edit sub-form: name, matcher kind toggle (Field / Text), the
  matcher's fields (path + op + value, or pattern), foreground color
  picker, background color picker.
- Save persists the whole `rules: Vec<Rule>` onto `self.config.rules` and
  calls `config::save(&self.config)` (matching how every other form's Save
  button persists — see `save_connection_form`, `save_search_settings` for
  the pattern), then closes the modal. No Result Tab needs re-running when
  rules change — re-rendering already happens automatically on the next
  `view()` call once `Config.rules` differs, per step 5.6.
- Cancel discards the working copy and closes without touching
  `self.config`.

Color picker: iced 0.14 doesn't ship a color-picker widget. A minimal
acceptable v1 is a hex text input (`"#e06c6c"`) validated against
`hex_color::parse_hex` on input, with a small swatch preview
(`container` filled with the parsed `Color`, or `PANEL_ALT` if unparsed).
Do not pull in a color-picker crate — this falls under the "no new
dependencies" constraint.

### 6.5 Checkpoint — end to end

Create a rule: `level == ERROR` → red foreground. Open a Result Tab against
an index with a `level` field. Confirm:

- Rows matching `level: "ERROR"` (or `"error"`, case-insensitively) show red
  text, in Table mode.
- Switching that same tab to raw text mode shows the same rows in red,
  proving both adapters go through the same `Prepared` rule set.
- Adding a second rule, a `Text` matcher on `"timeout"`, colors just the
  word `timeout` within any line, including within an already-red `ERROR`
  line (confirms Field and Text rules compose, per 5.7's last test).
- Restarting the app preserves the rule (confirms 5.1/5.3's persistence).

---

## Step 7 — Make the suite load-bearing

**Visible change: none** (to the app; visible to the commit workflow).

### 7.1 Edit the pre-commit hook

`.cargo-husky/hooks/pre-commit` currently runs, in order:

```sh
echo '+ cargo fmt --all -- --check'
cargo fmt --all -- --check

echo '+ cargo clippy --all-targets --all-features -- -D warnings'
cargo clippy --all-targets --all-features -- -D warnings
```

Add a third block after clippy:

```sh
echo '+ cargo test'
cargo test
```

The file has a `set -e` at the top already — no further error-handling
changes needed; `cargo test` failing will already abort the commit under
that.

### 7.2 Reinstall

Per the file's own header comment, cargo-husky reinstalls the hook from
`.cargo-husky/hooks/pre-commit` on the next `cargo build`/`cargo test` — run
one to regenerate `.git/hooks/pre-commit`, then diff it against the source
file to confirm the new `cargo test` block made it through.

### 7.3 Checkpoint

Temporarily break a test (e.g. flip an assertion), attempt a commit, confirm
it's blocked with the failing test's output. Revert the break, confirm a
commit succeeds.

---

## Explicit non-goals

Do not implement any of the following as part of this plan, even if it
seems like a natural extension while you're in the code. Each was raised
and deliberately deferred during the design session; pulling one in here
would be scope drift, not thoroughness:

- **GREP** itself (a search-and-collect UI over rendered `Line` text). This
  plan only guarantees the seam GREP will read through; the GREP window's
  own shape depends on decisions in candidates C2/C3, not yet made.
- **Regex patterns.** Text-matcher rules are substring-only.
- **Per-Saved-Search rules.** Rules are global, full stop, for this plan.
- **Template modifiers** (width, alignment, date formatting). The grammar
  is exactly `%{field.path}`, nothing else, though `|` is reserved inside a
  placeholder for a future modifier syntax.
- **Caching a rendered `Line`.** Every render recomputes it from scratch.
- **A real color-picker widget or new dependency of any kind.**
- **Migrating old configs.** Nothing here needs it; if you think you've
  found a case that does, that's a signal to stop and re-check against
  `#[serde(default)]` before writing migration code.

## If something doesn't fit

If, while implementing, you find a spot where this plan's instructions
don't match what the code actually looks like (a function has moved, a line
number is off, an iced API differs from what's described), that's an
expected consequence of a plan being written before the code changes under
it — fix the mismatch using this plan's *intent*, not by inventing a new
design. If the mismatch is large enough that the intent itself is unclear,
stop and ask rather than guessing.
