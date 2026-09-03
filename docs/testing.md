# Testing

Two things to run: the fast test suite, and — for the wide-line render work
only — a scroll-performance harness that drives a real window.

## Test suite — `cargo test`

Unit + integration tests, ~90 of them, in `#[cfg(test)]` modules next to the
code they cover. Fast, deterministic, no display or Elasticsearch needed.

`cargo fmt --check` and `cargo clippy --all-targets -- -D warnings` run
alongside it in CI and in the `.cargo-husky` pre-commit hook. Run all three
before committing:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

## Scroll-performance harness — `LOGLENS_PERF_SCROLL=1`

The measurement half of the wide-line render work
(`docs/plans/wide-line-perf-followups.md` item 0). It drives a real window
through a fixed, repeatable scroll over a Result Tab and prints per-frame
timings, so a change can be A/B'd against a stable baseline.

Not part of `cargo test`: it needs a display, opens a window, and takes ~12s.
Run it by hand.

```sh
cargo build --release

LOGLENS_PERF_SCROLL=1 \
LOGLENS_HITS=benches/fixtures/nginx-800.json \
LOGLENS_PERF_MODE=table \
./target/release/loglens
```

A window opens, scrolls itself top to bottom, prints a table to stderr, and
exits. By default the fixture is loaded **10×** (`LOGLENS_HITS_REPEAT`) — ~8k
rows from the 800-row file — so the 12s scroll covers 10× the content at 10×
the per-frame velocity, jumping the row window far enough each frame to
actually stress the renderer. The 12s stays long on purpose: A/B'ing a
change needs a fat sample behind p99 / max. Set `LOGLENS_HITS_REPEAT=1` for
the file as-is.

### Environment variables

| var | effect |
|---|---|
| `LOGLENS_PERF=1` | timing only, no scripted scroll — scroll by hand, numbers print on window close |
| `LOGLENS_PERF_SCROLL=1` | run the scripted scroll (implies `LOGLENS_PERF`) |
| `LOGLENS_PERF_SCROLL_SECS=N` | scroll duration, top to bottom (default 12) |
| `LOGLENS_HITS=<path>` | load Hits from a saved `_search` response instead of querying a cluster |
| `LOGLENS_HITS_REPEAT=N` | concatenate that fixture onto itself `N` times (default 10) — a small checked-in file standing in for an `N`× larger result set. `1` = the file as-is |
| `LOGLENS_PERF_SEARCH=<id or name>` | which Saved Search to open (default: the first one configured) |
| `LOGLENS_PERF_MODE=table\|text` | force the tab's Layout mode, overriding the Saved Search's |

Without `LOGLENS_HITS` it runs a real query, so a Saved Search must exist and
the cluster must be reachable; use an **absolute** timeframe and stop the
`generator` container (see `dev/`) so the result set does not drift between
runs.

`LOGLENS_PERF_MODE` matters: item 0 is about **Table** mode
(`view.hit_table_rows`); raw text mode is item 5 (`view.raw_text_rows`).

### Fixtures

`benches/fixtures/*.json` are saved `_search` responses (`hits.hits[]` with
`_source` and `sort`) captured once from the dev cluster and checked into git,
so a run is byte-identical every time and needs no cluster.

They're kept small and representative rather than large — each already spans
the full spread of line lengths and special characters its stream produces.
To benchmark against a bigger result set, don't grow the file: the loader
concatenates it onto itself `LOGLENS_HITS_REPEAT` times (default 10). The
copies are **identical**, though — 10× `nginx-800.json` is 800 distinct
Hits, not 8000. `AdvanceCache` is unaffected (it keys per grapheme cluster),
but if a future per-Hit render / height cache (followups items 3/4/6) is
keyed by Hit *content* rather than position, it will look better here than
against a real result set the same size. Regenerate a genuinely larger
fixture from the dev cluster if that ever matters.

- `nginx-800.json` — 800 `logs-loglens-nginx` Hits; ~15% carry a several-KB
  query string, ~8% a huge user agent. The Table-mode case item 0 is about.
- `payloads-150.json` — 150 `logs-loglens-payloads` Hits; the 3–30 KB
  base64 / JSON / SQL monsters. The raw-text-mode (item 5) worst case.

Regenerate (dev cluster from `dev/` must be up):

```sh
curl -s -XPOST 'localhost:9200/logs-loglens-nginx/_search?filter_path=hits.hits._source,hits.hits.sort' \
  -H 'content-type: application/json' \
  -d '{"size":800,"sort":[{"@timestamp":"desc"},{"_doc":"asc"}]}' \
  -o benches/fixtures/nginx-800.json
```

### Reading the output

```
metric                        n        p50        p90        p99        max       mean
--------------------------------------------------------------------------------------
perf.frame_interval         721   16.677ms   16.823ms   19.234ms   34.107ms   16.522ms
update                     1451    0.001ms    0.001ms    0.002ms    0.043ms    0.001ms
view                       1447    0.800ms    0.935ms    1.190ms    2.136ms    0.811ms
view.hit_table_rows        1444    0.753ms    0.882ms    1.091ms    2.095ms    0.764ms
```

(`nginx-800.json` × 10, Table mode, default 12s scroll, this dev machine —
Table mode stays 60Hz here even at ~8k rows; the reproducible stutter is
raw-text mode, item 5.)

- **`perf.frame_interval`** — wall time between rendered frames. Near 16.7ms
  with a tight p99 means 60Hz with no missed frames. p99 / max well above the
  refresh interval means dropped frames = visible stutter.
- **`view`** — one full `view()` tree rebuild. **`view.hit_table_rows`** (or
  `view.raw_text_rows`) — just the windowed per-Hit render + widget-build
  loop, the part the wide-line work touches.
- **`update`** — message handling. Expected to be noise.

The decision this drives:

- `view` alone over budget → the cost is our code → plan items 1 / 3 / 7,
  and a profiler run isolates which.
- `view` small but `frame_interval` blowing past 16.7ms → the cost is
  **below** `view()` — iced layout / draw / present of what `view()` handed
  down. Item 1 (smaller window = fewer primitives) is the lever, or it needs
  the item-4 custom widget or raw-text clamping (item 5).

`view` / `update` run roughly twice per `frame_interval`: the frame tick
drives one, and the scrollable's own `on_scroll` settle drives another — the
same double-pump a real drag causes. The per-call numbers are what matter.

### Profiler pass

Once the timing says which layer, run a profiler over the same scripted scroll:

```sh
cargo install samply   # once
LOGLENS_PERF_SCROLL=1 LOGLENS_HITS=benches/fixtures/nginx-800.json LOGLENS_PERF_MODE=table \
  samply record ./target/release/loglens
```

Opens the Firefox profiler UI — the only tool that sees inside iced's layout /
draw and wgpu, e.g. to go from "`frame_interval` p99 is 40ms but `view` is
0.3ms" to "38ms of it is glyph shaping in cosmic-text on the untruncated
raw-text lines".

## Related

- `src/perf.rs` — the instrumentation itself.
- `docs/wide-line-rendering-resources.md` — why the wide-line cost exists.
- `docs/plans/wide-line-perf-followups.md` — the ranked optimization list.
