# Follow-ups: scroll performance on wide log lines

**Status:** draft — a punch list for a future session, not a reviewed/approved
plan. Not prioritized beyond the ordering below, which is a recommendation,
not a commitment.
**Origin:** the `advance_cache.rs` / `results_view.rs` change (see git log)
fixed the biggest *measured* cost — full-line shaping of untruncated cell
text — but scrolling `logs-loglens-nginx` still stutters, just less. This
document is the list of what's left, for whoever (human or Claude) picks
this back up.

**Read first:** `docs/wide-line-rendering-resources.md` — the shaping-cache
background (why per-grapheme advance caching works, its limits) that
everything below builds on. Don't re-derive it; it's already measured and
cited there.

## What's already true — don't re-litigate or accidentally revert

- Cell text in **Table mode** is truncated to its column's pixel width
  before being handed to a `text` widget — see `part_widget` and
  `hit_table` in `src/results_view.rs`, backed by
  `AdvanceCache::take_width` in `src/advance_cache.rs`. Measured 110 rows ×
  200px column: **10.5ms → 103µs**.
- **Highlight rules were removed** (item 2). A `Part` is plain text;
  everything renders through a `text` widget — no `rich_text`, no
  `Segment`/`Style`, no `matching` pass.
- **Raw text mode is horizontally virtualized** (item 5). `raw_text_view`
  slices each row to `[scroll_x, scroll_x + viewport_w]` (plus
  `RAW_SLICE_SLACK`) before shaping — the horizontal analogue of the
  vertical `row_window`. A leading spacer of the scrolled-off prefix's exact
  width keeps every glyph at its true position (no visible shift); the
  `scrollable`'s content width is set to `LineCache::max_line_bytes` × one
  monospace advance so the horizontal scrollbar still reaches the end of
  every line. `ResultTab` carries `scroll_x` / `viewport_w` alongside
  `scroll_y` / `viewport_h`, fed by `Message::ResultScrolled`.
- The view layer for a Result Tab (table, raw text, popovers, Format modal)
  lives in `src/results_view.rs` now, as free functions — not
  `impl LogLens` methods in `main.rs`. Extend it there, not back on
  `LogLens`.
- **`line::render` is cached per Hit** (item 3). `hit_table`/`raw_text_view`
  read through `tab.line_cache` (`line::LineCache`), keyed by Hit position,
  invalidated by `Layout::fingerprint()` and `ResultTab::reset_line_cache`.
  Don't add a call to `line::render` in the windowed row loop — go through
  the cache. `format_modal`'s preview is the one deliberate direct caller
  (different draft Layout, off the hot path).
- **Line wrapping + variable row heights are built** (item 6). A per-tab
  `wrap` flag (persisted on the Saved Search) drives a row-height model
  inside `LineCache` — `rows`/`offsets` prefix sums, `wrap_rows` in
  `advance_cache.rs`, a global `Config.wrap_row_cap`. `ResultTab::row_window`
  and the spacer maths read the model, not `ROW_H` arithmetic. With `wrap`
  off the model is a flat `ROW_H` grid and nothing new runs — don't add a
  `ROW_H`-per-row assumption back.
- `AdvanceCache` shapes through iced's *own* global font system
  (`iced::advanced::graphics::text::font_system()`) — don't open a second
  `cosmic_text::FontSystem`; it'd double font-loading cost and risk
  measurements drifting from what's actually drawn.

## 0. Measure before doing anything else below — DONE (harness built)

Everything in this doc past item 1 is a *hypothesis* about where the
remaining time goes, based on isolated microbenchmarks (see the git history
for the `advance_cache.rs`/`results_view.rs` change) — not a profile of the
live app while it's actually stuttering.

**The harness now exists — how to run it: `docs/testing.md`.** `src/perf.rs`
plus `LOGLENS_PERF_SCROLL=1` drives a fixed scroll over a checked-in fixture
(`benches/fixtures/*.json`), no cluster needed, and prints per-frame p50/p90/
p99/max for `update`, `view`, and the windowed row-build loop, plus the
realized frame interval. Runs are comparable, so item 1 (and anything else)
can be A/B'd properly.

The default run is bigger now than when the numbers below were taken:
`LOGLENS_HITS_REPEAT` concatenates the fixture onto itself, default 10 (≈ 8k
rows from `nginx-800.json`), so the same 12s scroll moves the row window 10×
further per frame. The fixture files and the 12s duration are unchanged.

First numbers, release build, `nginx-800.json` (no repeat), Table mode, this
dev machine (`WINDOW_BUFFER` 40, i.e. pre-item-1 — see item 1 for the
after):

- `perf.frame_interval` p50 16.7ms, p99 17.6ms — **60Hz, no missed frames.**
- `view` p50 1.3ms, `view.hit_table_rows` p50 1.2ms — the row loop is ~94%
  of `view()` and well under a 16.7ms budget.
- Raw text mode over `payloads-150.json` is the opposite: `view` p50 0.3ms
  but `frame_interval` p99 ~37ms, max ~48ms — **frames dropped, and not in
  our code.** The cost is iced shaping the untruncated multi-KB lines
  (layout/draw), i.e. item 5, and a profiler pass is the next step there.

So on this hardware Table-mode scrolling is already smooth; the reproducible
stutter is raw-text mode (item 5), or needs heavier data / slower hardware to
show in Table mode. Confirm on the machine that actually stutters before
picking up items 3–4.

Still open under item 0:

- **Criterion microbenchmarks** for `line::render` / `take_width` (isolated,
  headless, statistical before/after). Needs a `src/lib.rs` split first — the
  modules are only reachable from the `main.rs` bin crate today. Not done.
- **Profiler pass** (`samply record` over the scripted scroll) to attribute
  the raw-text-mode frame time to a specific cosmic-text / wgpu call.

## 1. `WINDOW_BUFFER` was 40 — shrunk to 8 — DONE

`src/results.rs`. Was flagged in the very first pass and never changed. At
the default window size roughly 30 rows are visible; `WINDOW_BUFFER = 40`
above *and* below meant `row_window()` built widgets for up to ~110 rows —
**3.7× the visible count** — every scroll frame, regardless of any other fix
in this list.

Now `8` (~175px of fling slack each way, roughly half the built-row count of
the visible slice). One-line change, no architectural risk.

Measured with the item-0 harness, release build, this dev machine:

| metric (p50)                         | buffer 40 | buffer 8 |             |
| ------------------------------------ | --------- | -------- | ----------- |
| `view`, `nginx-800.json` table       | 1.31ms    | 0.74ms   | **−44%**    |
| `view.hit_table_rows`                | 1.24ms    | 0.70ms   | **−44%**    |
| `view` p99, table                    | ~1.72ms   | ~0.94ms  | −45%        |
| `view`, `payloads-150.json` raw text | ~0.30ms¹  | ~0.17ms  | ~−45%       |
| `frame_interval` p99, raw text       | ~37ms¹    | ~28ms    | still drops |

`frame_interval` in **Table** mode was already a tight 16.7ms/60Hz on this
hardware both before and after — the win is headroom (nearly half of
`view()` reclaimed), not a fixed stutter. **Raw text** mode still drops
frames (item 5): the per-row cost below `view()` is iced shaping the
untruncated multi-KB lines, and 8 is just fewer of them per frame, not a
cure.

¹ raw-text "before" is the same-machine baseline recorded under item 0
above, not a fresh A/B on the reverted constant.

## 2. Highlight rules — DONE (feature removed)

Rather than optimize the matching path, the Highlight rules feature was
removed outright (this session). `Config.rules`, `src/rules.rs`, the
`line::matching` module, `Rule`/`Matcher`/`Op`/`Style`/`Segment`/`Prepared`,
the rules modal and its `Message` variants, and the options-strip button
are all gone. `Part` is now just `text: String`; `line::render(hit,
layout)` no longer takes a `Prepared`; `part_widget` renders a plain `text`
widget only (no `rich_text` branch). The landmine measured here — *N*
redundant lowercasing passes per Hit with *N* enabled rules — is moot: no
rules can exist.

If highlighting comes back, re-derive the fix direction from git history
(the `matching::apply` / `match_ranges` lowercasing) rather than this
paragraph.

## 3. `line::render` reran on every scroll frame — DONE (per-Hit `Line` cache)

Was: every frame, `hit_table`/`raw_text_view` called `line::render` fresh for
the whole windowed slice — JSON field lookup (`resolve`), timestamp
formatting, string building (a several-KB query-string column `.clone()`d per
row, or the whole multi-KB concatenated template line in raw text) — even for
rows also on screen last frame. Strictly upstream of the shaping-cache fix, so
item 1 left it untouched.

Now cached. `line::LineCache` (`src/line.rs`) holds a `HashMap<usize, Line>`
per `ResultTab` (`tab.line_cache`, a `RefCell` — `view` has `&self`), keyed by
Hit position:

- **Positional, not content-keyed.** `tab.hits` positions are stable within a
  loaded result set — paging only `extend`s; sort / refresh replace wholesale
  and call `ResultTab::reset_line_cache` (three sites in `main.rs`:
  `start_run`'s two `hits.clear()` paths and `apply_page`'s non-append
  branch). A content hash would cost hashing multi-KB JSON per row per frame
  and would over-report its hit rate against the harness's identical-copy
  fixtures.
- **Layout invalidation via `Layout::fingerprint()`** — a hash of every input
  `render` reads (`mode`, `columns`, `template`, `timestamp_field`, `utc`;
  `LineCache::prepare` clears the whole map when it changes). Column pixel
  widths are deliberately *not* in it — they never reach `render`, so a
  drag-resize must not bust the cache.
- **Bounded.** `prepare` evicts entries more than `RETAIN` (64) rows outside
  the live window each frame, so a long scroll can't grow the map without
  bound (~window + 128 `Line`s max).

Measured with the item-0 harness, release build, this dev machine, default
12s scroll × 10 repeat:

| metric (p50)                            | before  | after   |          |
| --------------------------------------- | ------- | ------- | -------- |
| `view.hit_table_rows`, `nginx-800`      | 0.690ms | 0.624ms | **−10%** |
| `view` p99, `nginx-800` table           | ~0.93ms | ~0.90ms | −4%      |
| `view.raw_text_rows`, `payloads-150`    | 0.182ms | 0.079ms | **−57%** |
| `view` p50, `payloads-150` raw text     | 0.210ms | 0.108ms | **−49%** |
| `frame_interval` p99, raw text          | ~36ms   | ~34ms   | still drops |

Table's win is modest — post-item-1 the row loop is dominated by per-cell
truncation + widget building, not `render`. Raw text's is large: one Part, no
truncation, and `render` there rebuilds the full multi-KB line. `frame_interval`
in raw text still blows past 16.7ms — that stutter is iced shaping the
untruncated lines *below* `view()` (item 5), which this doesn't touch.

## 4. iced's built-in paragraph cache can't help during a scroll — SKIPPED

Documented in the earlier design discussion (see conversation / git log)
but never built: iced's `Paragraph::compare` (`iced_core`) skips reshaping
when content is unchanged, but its cache is keyed by *tree position*, not
Hit identity. Since `row_window()`'s `start` shifts with `scroll_y`, the
row at tree position 5 holds a different Hit every frame during a scroll —
so iced reshapes it every time regardless. After item 1's truncation fix
this is a *small* cost per row, but it's nonzero and multiplied by every
row in the (now hopefully smaller, per item 1) window, every frame.

The real fix is a custom `Widget` (not `text`/`rich_text`) holding
`Vec<Paragraph>` keyed by Hit id in `tree::State`, whose `layout()` returns
a size without shaping and whose `draw()` calls `renderer.fill_paragraph`
directly on an already-shaped, cached paragraph. iced 0.14 exposes what's
needed (`Paragraph` trait, `fill_paragraph`) — this was scoped as "Tier 2,
build only if measurement says it's still needed" in the earlier design
pass, and that's still the right call: it's real scope (reimplementing
what `mouse_area` + `container` currently give for free — hit-testing,
click-to-select — around a custom widget). Don't start here; start at
item 0/1.

**Skipped 2026-09-03 after a design/feasibility pass.** The approach is
sound and iced 0.14 supports it fully — the concrete
`iced::advanced::graphics::text::Paragraph` type is nameable (so the shape
cache can be a `RefCell` sibling of `LineCache`, or live in `tree::State`),
`renderer.fill_paragraph` is on the public `advanced::text::Renderer`
trait, and because `view()` is rebuilt every scroll frame here, `layout()`
can shape the window slice with no `draw()`-time interior mutability. But:

- **No measured problem to fix.** The item-0 harness on this dev machine
  shows Table mode already at a tight 60Hz, `view.hit_table_rows` p50
  ~0.62ms. Item 4 would attack widget-build churn + per-frame reshaping —
  exactly what item 3's numbers said dominates that sub-ms loop — but as
  speculative headroom, not a fixed dropped-frame stutter. The doc's own
  rule (item 0) is "confirm on the machine that actually stutters first",
  and that hasn't been done for Table mode.
- **First custom `Widget` in the codebase**, ~200–300 lines, re-implementing
  what `mouse_area` + `container` + `row` give for free: click→`HitClicked`
  (cursor-y math), the selection-row background (`fill_quad`), per-cell
  clipping (`fill_paragraph` clip bounds), and column x-offset arithmetic
  that has to line up pixel-for-pixel with the still-widget header row.
  Medium risk on visual parity for a benefit that doesn't show on available
  hardware.

**Item 6 landed without it (this session).** The "wrapping needs a custom
body widget regardless" prediction turned out wrong: `column` of
variable-height rows + a prefix-sum height model in `LineCache` was enough,
and the per-Hit exact-`wrap_rows` memo (`LineCache::exact`) already gives
the "measure once, not per frame" win item 4's shape cache was for. A
`Vec<Paragraph>`-per-Hit shape cache would still cut the *shaping* iced does
below `view()` for wrapped rows — revisit only if a Table-mode frame drop
reproduces in the harness (heavier fixture, or the machine that stutters).

## 5. Raw text mode ("Text" layout mode) — DONE (horizontal virtualization)

Was: `raw_text_view` handed each windowed row's whole (multi-KB) line to a
`text` widget with no truncation — `part_widget(part, None)` — because its
`scrollable` handles horizontal scrolling internally and the app tracked no
`scroll_x` / viewport-width state, so clamping blind would have hidden
content a user could scroll right to reach.

Now virtualized horizontally, mirroring the vertical `row_window`:

- **Tracked offset.** `Message::ResultScrolled` carries `offset_x` /
  `viewport_w`; `ResultTab` stores `scroll_x` / `viewport_w` next to
  `scroll_y` / `viewport_h`. Table mode is vertical-only, so its `scroll_x`
  stays 0 (harmless).
- **Per-row slice.** Each row shapes only
  `[scroll_x, scroll_x + viewport_w + RAW_SLICE_SLACK]` of its line
  (`AdvanceCache::take_width` twice — drop the scrolled-off prefix on a
  grapheme boundary, keep a viewport-plus-slack slice). A leading
  `space` of the prefix's exact shaped width holds every glyph at its true
  position, so the slice never moves a visible pixel as `scroll_x` changes.
- **Scrollbar extent.** Each row is a `container` of fixed width
  `LineCache::max_line_bytes()` × `AdvanceCache::mono_advance()` — the widest
  line seen so far, estimated from byte length (which over-approximates
  monospace column width for any non-ASCII run, so nothing scrollable-to is
  ever clipped). Monotonic within a result set; grows as longer lines scroll
  in, the way the vertical extent grows with paging.
- **`take_width` now locks once per call** instead of per grapheme — the raw
  slice is hundreds of graphemes, and the lock is never contended.

Measured with the item-0 harness, release build, this dev machine, default
12s × 10 repeat, `LOGLENS_PERF_MODE=text`:

| metric                              | before  | after    |             |
| ----------------------------------- | ------- | -------- | ----------- |
| `frame_interval` p50, `payloads-150`| 21.3ms  | 16.68ms  | **60Hz**    |
| `frame_interval` p90, `payloads-150`| 31.5ms  | 16.85ms  | **60Hz**    |
| `frame_interval` p99, `payloads-150`| 34.2ms  | ~18.4ms  | **−46%**    |
| `view` p50, `payloads-150`          | 0.108ms | ~0.52ms  | see below   |
| `frame_interval` p99, `nginx-800`¹  | ~18ms   | ~17.5ms  | already 60Hz|

`view` / `view.raw_text_rows` go *up* (~0.03–0.08ms → ~0.5ms): the
per-row grapheme walk is now in `view()` where the harness sees it, while
the ~15ms it removed — iced shaping the full multi-KB line in
layout/draw *below* `view()` — never showed in the instrumentation, only in
the dropped frames. Net: `payloads-150` raw text goes from a constant
~48Hz stutter to a solid 60Hz. `view()` at ~0.5ms is 3% of the frame
budget.

¹ `nginx-800` forced to text mode was already 60Hz on this hardware before
the change (its lines are wide but the machine kept up); the value is that
it no longer *depends* on the machine keeping up with full-line shaping.

Known minor artifacts (both mirror existing vertical behaviour, neither a
stutter): the horizontal scrollbar thumb recalibrates downward as longer
lines are discovered on the first scroll-through (monotonic, stabilises
after one pass); `viewport_w` defaults to 1200 until the first scroll event
(like `viewport_h`'s 600).

Still not done under item 5: the scripted-scroll harness only drives
vertical motion, so a horizontal sweep is a manual check
(`LOGLENS_PERF_MODE=text`, scroll right, confirm the full line is reachable
and nothing clips early).

## 6. Wrapping + variable row heights — DONE (per-tab Wrap toggle)

Built this session. A per-tab **Wrap** toggle in the options strip (off by
default, persisted on the Saved Search as `wrap: bool`), honoured by both
Table mode (the flexible last column wraps; fixed columns still truncate to
one line) and Text mode (the whole line wraps; item 5's horizontal
virtualization is bypassed while wrap is on).

- **`AdvanceCache::wrap_rows(text, width, max_rows)`** — visual row count
  under glyph wrapping from summed grapheme advances, no shaping. A
  `#[cfg(test)]` parity test drives a real `cosmic_text` buffer with
  `Wrap::Glyph` over four sample lines × three widths and asserts an exact
  match — the "verified in the design discussion" claim is now a test.
- **Row-height model folded into `LineCache`** (items 3/4/6 share one
  per-Hit cache, as the doc recommended). `rows[i]` is the uncapped visual
  row count — a cheap byte-length estimate (`len × mono_advance / width` plus
  one row per hard `\n`) for off-screen Hits, upgraded to an exact
  `wrap_rows` count the first time the Hit is drawn and then memoised in an
  `exact` set so a row that stays on screen is measured once, not per frame.
  `offsets` is the prefix sum of per-Hit pixel heights; `row_window`,
  `wants_more`, the spacers and the `PerfTick` content height all read it.
  With Wrap off the model is a flat `ROW_H` grid built with no render pass —
  the default path is unchanged.
- **One O(n) render pass** (`view.row_cache.lens`) the first time Wrap turns
  on, to measure every loaded line's byte length. ~8–12ms for
  `payloads-150` / `nginx-800` at the harness's 10× repeat; once per result
  set, kept across an off→on→off→on toggle.
- **Row cap:** global `Config.wrap_row_cap: Option<usize>` (Settings →
  Display, default `Some(8)`, blank = no cap). A Hit past the cap is clamped
  and gets an inline **"＋ N more lines"** button (`Message::ResultHitExpand`
  → a per-tab `expanded` set on the cache); **"－ collapse"** when expanded.
  `WRAP_HARD_MAX = 400` visual rows bounds shaping even expanded / uncapped,
  past which the row shows a "line truncated — open Hit detail" note.

Measured with the item-0 harness, release build, this dev machine, default
12s × 10 repeat, `LOGLENS_PERF_WRAP=1`:

| metric (p50)                          | before (no wrap) | wrap on |
| ------------------------------------- | ---------------- | ------- |
| `frame_interval`, `payloads-150` text | 16.7ms           | 16.67ms — **60Hz** |
| `view.raw_text_rows`, `payloads-150`  | ~0.5ms           | ~1.2ms  |
| `frame_interval`, `nginx-800` table   | 16.7ms           | 16.68ms — **60Hz** |
| `view.hit_table_rows`, `nginx-800`    | ~0.62ms          | ~0.54ms |
| `view.row_cache.lens` (one-off)       | —                | 8–12ms  |

Known artifacts (both mirror existing accepted behaviour): the vertical
scrollbar recalibrates slightly as off-screen estimates are replaced by
exact counts on the first scroll-through (monotonic-ish, like item 5's
horizontal extent); the wrapped column is laid out at a width bucketed to
`WRAP_WIDTH_BUCKET` (8px) so it can be a hair narrower than the true fill
width. The scripted-scroll harness only drives vertical motion, so wrapped
rendering fidelity (cap, expand, no clipped last line) is a manual check.
