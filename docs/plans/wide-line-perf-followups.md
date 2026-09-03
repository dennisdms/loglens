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
  before being handed to `text`/`rich_text` — see `part_widget` and
  `hit_table` in `src/results_view.rs`, backed by
  `AdvanceCache::take_width` in `src/advance_cache.rs`. Measured 110 rows ×
  200px column: **10.5ms → 103µs**.
- **Raw text mode is deliberately still untruncated.** It has real
  horizontal scrolling with no tracked offset; truncating blind would hide
  content a user could otherwise reach. This is item 6 below, not a bug.
- The view layer for a Result Tab (table, raw text, popovers, Format modal)
  lives in `src/results_view.rs` now, as free functions — not
  `impl LogLens` methods in `main.rs`. Extend it there, not back on
  `LogLens`.
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

First numbers, release build, `nginx-800.json`, Table mode, this dev machine
(`WINDOW_BUFFER` 40, i.e. pre-item-1 — see item 1 for the after):

- `perf.frame_interval` p50 16.7ms, p99 17.6ms — **60Hz, no missed frames.**
- `view` p50 1.3ms, `view.hit_table_rows` p50 1.2ms — the row loop is ~94%
  of `view()` and well under a 16.7ms budget.
- Raw text mode over `payloads-150.json` is the opposite: `view` p50 0.3ms
  but `frame_interval` p99 ~37ms, max ~48ms — **frames dropped, and not in
  our code.** The cost is iced shaping the untruncated multi-KB lines
  (layout/draw), i.e. item 5, and a profiler pass is the next step there.

So on this hardware Table-mode scrolling is already smooth; the reproducible
stutter is raw-text mode (item 5), or needs heavier data / rules / slower
hardware to show in Table mode. Confirm on the machine that actually stutters
before picking up items 2–4.

Still open under item 0:

- **Criterion microbenchmarks** for `line::render` / `take_width` / the rules
  path (isolated, headless, statistical before/after). Needs a `src/lib.rs`
  split first — the modules are only reachable from the `main.rs` bin crate
  today. Not done.
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

## 2. Highlight rules will be the next dominant cost, the moment any exist

The current config has `"rules": []`, so this isn't today's problem — but
it's a landmine. Measured earlier (see git history / conversation): with a
single enabled text rule, rendering a 110-row window went from **80µs to
900µs** — an 11× jump. `src/line.rs`, `matching::apply`
(around line 487) and `match_ranges` (around line 603) lowercase the full
haystack **once per rule per Hit**, not once per Hit. With *N* enabled
rules that's *N* redundant lowercasing passes over the same string.

Fix direction: lowercase each Part's text once per Hit (e.g. compute it
once at the top of `apply`, pass the lowered string down to each rule's
matcher) rather than re-lowering inside `match_ranges` for every rule.
Verify with a version of the existing `line::matching::tests` bench-style
comparison (the codebase already has real unit tests here — extend them,
don't just spot-check manually).

## 3. `line::render` itself reruns on every scroll frame, not just shaping

Independent of widget shaping (which item 1's fix already addresses): every
frame, `hit_table`/`raw_text_view` call `line::render` fresh for the whole
windowed slice — JSON field lookup (`resolve`), timestamp formatting
(`format_dt`/`format_timestamp_*`), string building — even for rows that
were *also* visible last frame and haven't changed at all. This is strictly
upstream of the shaping-cache fix, so it survives untouched.

Potential fix: a per-Hit rendered-`Line` cache, keyed by Hit identity plus
something that changes when the *inputs* to `render` change (the active
`Layout` — mode/columns/template/timestamp_field/utc — and the rules
generation). Complications worth thinking through before starting:

- `Hit` (`src/es/mod.rs`) has no stable id today beyond its position in
  `tab.hits`, which is stable *within* a loaded page but needs a cache-key
  story once paging appends more Hits or a refresh replaces them.
- Cache invalidation on `Layout`/rules change needs to be correct, not
  just "usually fires" — a stale cached `Line` after a column add/remove or
  a rule edit would be a visible bug, not just a missed optimization.
- Worth measuring (per item 0) whether this is actually worth the
  complexity before building it — it may turn out item 1 alone, or item 1 +
  4, gets scrolling smooth enough that this isn't needed.

## 4. iced's built-in paragraph cache can't help during a scroll

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

## 5. Raw text mode ("Text" layout mode) is still fully unclamped

Deliberately deferred when the Table-mode fix shipped (see the doc comment
on `raw_text_view` in `src/results_view.rs`). Its `scrollable` handles
horizontal scrolling internally and the app tracks no `scroll_x` /
viewport-width state, so `AdvanceCache::take_width` can't be applied there
without hiding content a user could reach by scrolling right.

To fix properly: add a tracked horizontal offset (extend
`Message::ResultScrolled` with `offset_x`, add a field to `ResultTab`
alongside `scroll_y`/`viewport_h`), then slice each row to
`[offset_x, offset_x + viewport_w)` the same way Table mode slices to
`[0, col_width)`. This is a real feature-sized piece of work, not a
one-liner — it changes what state a scroll event carries.

Relevant today only for a raw-text-mode Saved Search on a stream with long
lines (e.g. the config's `test123` search, or any future one) — not the
`logs-loglens-nginx` Table-mode search this conversation started from.

## 6. Wrapping + variable row heights — the original "eventually" ask

Not implemented at all. This is what kicked off the whole design
discussion (see `docs/wide-line-rendering-resources.md` and the
conversation it came from) but nothing has been built yet:

- A per-Hit wrapped-row-count estimate, computed from `AdvanceCache`
  without shaping (`wrap_rows`-style function, verified in the design
  discussion to exactly match cosmic-text's own `Wrap::Glyph` output —
  200/200 on real data at three viewport widths).
- A height model on `ResultTab` (prefix sums over per-Hit heights) to
  replace the fixed-`ROW_H` assumption baked into `row_window()`,
  `wants_more()`, and the spacer-height math in `hit_table`/
  `raw_text_view`.
- Actually wiring `.wrapping(text::Wrapping::Glyph)` into the render call
  for wrapped rows, and a cap on how many visual rows one Hit can wrap to
  (the worst real line in this project's dev data wraps to 67 rows at
  1200px — needs an expand affordance, not unbounded height).

This interacts with items 3 and 4 above: a render cache and a height cache
are naturally the same shape of per-Hit cache, so if this gets picked up,
worth designing items 3/4/6 together rather than three separate caches.

## 7. Minor: `Prepared::from_rules` rebuilds redundantly per frame

`main_area`, `main_view`, and `options_bar` in `main.rs` each independently
call `line::Prepared::from_rules(&self.config.rules)` when building the
active tab's view. With `rules: []` this is nearly free today, but it's
unforced per-frame allocation. Low priority — compute once per `view()`
call and thread it through, or cache on `LogLens` invalidated on
`config.rules` change. Not worth doing before items 0–2 are tried.
