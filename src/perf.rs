//! Opt-in frame-timing instrumentation for the wide-line scroll-performance
//! work. A no-op unless `LOGLENS_PERF=1` (the scripted-scroll harness,
//! `LOGLENS_PERF_SCROLL=1`, turns it on too). See `docs/testing.md` for how to
//! run it and `docs/plans/wide-line-perf-followups.md` item 0 for why.
//!
//! What this measures: the two phases that are *this app's* code — `update()`
//! and `view()`, plus the windowed row-build loop inside `view()` — and, in
//! the scripted-scroll harness, the realized wall-clock interval between
//! scroll frames. That interval stretches past `update + view` exactly when
//! the phases iced runs afterwards (layout, draw, present) can't keep up, so
//! it's the signal for "the cost is below our code". Breaking those
//! iced-internal phases down further is a job for a sampling profiler
//! (`samply record`), not this module.
//!
//! Everything here is gated on [`enabled`], which reads the environment once.
//! When off, [`span`] returns `None` and [`record`] returns immediately, so
//! the instrumentation left in `main.rs` / `results_view.rs` costs a single
//! predictable branch per call.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

/// Whether any perf instrumentation is active this run. Any of the harness
/// environment variables turns it on — including `LOGLENS_HITS`, which also
/// redirects every search to that file, so its presence is treated as a
/// deliberate opt-in the same way `LOGLENS_PERF` is.
pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var_os("LOGLENS_PERF").is_some()
            || scroll_harness()
            || std::env::var_os("LOGLENS_HITS").is_some()
    })
}

/// Whether the scripted-scroll harness should run: open a Saved Search, drive
/// a fixed scroll over it, print timings, and exit. Implies [`enabled`].
pub fn scroll_harness() -> bool {
    std::env::var_os("LOGLENS_PERF_SCROLL").is_some()
}

/// How long the scripted scroll should take, top to bottom, in seconds.
/// `LOGLENS_PERF_SCROLL_SECS`, default 12.
pub fn scroll_secs() -> f32 {
    std::env::var("LOGLENS_PERF_SCROLL_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|s: &f32| *s > 0.0)
        .unwrap_or(12.0)
}

/// A file of Hits to load instead of querying Elasticsearch — a saved
/// `_search` response (`hits.hits[]` with `_source` and `sort`). `LOGLENS_HITS`.
/// Lets the harness run with no cluster and byte-identical input every time.
pub fn fixture_path() -> Option<PathBuf> {
    std::env::var_os("LOGLENS_HITS").map(PathBuf::from)
}

/// Which Saved Search the harness opens: matched against each Saved Search's
/// `id` first, then its `name`. `LOGLENS_PERF_SEARCH`. Unset picks the first
/// Saved Search of the first configured Connection.
pub fn target_search() -> Option<String> {
    std::env::var("LOGLENS_PERF_SEARCH")
        .ok()
        .filter(|s| !s.is_empty())
}

/// A Layout mode to force the opened tab into, overriding the Saved Search's
/// own. `LOGLENS_PERF_MODE=table` (item 0's target — the Hit table) or `text`
/// (raw text mode, item 5). Unset keeps the Saved Search's mode.
pub fn force_mode() -> Option<crate::line::LayoutMode> {
    match std::env::var("LOGLENS_PERF_MODE")
        .ok()?
        .to_ascii_lowercase()
        .as_str()
    {
        "table" => Some(crate::line::LayoutMode::Table),
        "text" | "raw" | "raw_text" => Some(crate::line::LayoutMode::RawText),
        _ => None,
    }
}

static SAMPLES: OnceLock<Mutex<BTreeMap<&'static str, Vec<f32>>>> = OnceLock::new();

fn samples() -> &'static Mutex<BTreeMap<&'static str, Vec<f32>>> {
    SAMPLES.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// Records one timing sample, in milliseconds, under `name`. A cheap no-op
/// when perf instrumentation is off.
pub fn record(name: &'static str, millis: f32) {
    if !enabled() {
        return;
    }
    samples()
        .lock()
        .expect("lock perf samples")
        .entry(name)
        .or_default()
        .push(millis);
}

/// A scoped timer. Records the elapsed wall time under its name when dropped —
/// including on an early `return`, since the guard is dropped on any scope
/// exit. Build one with [`span`]; it is `None` (and records nothing) when perf
/// is off.
pub struct Span {
    name: &'static str,
    start: Instant,
}

impl Drop for Span {
    fn drop(&mut self) {
        record(self.name, self.start.elapsed().as_secs_f32() * 1_000.0);
    }
}

/// Starts a [`Span`] timing `name`, or `None` when perf is off. Bind it to a
/// name (`let _s = perf::span("view");`) so it lives to the end of the scope.
#[must_use]
pub fn span(name: &'static str) -> Option<Span> {
    enabled().then(|| Span {
        name,
        start: Instant::now(),
    })
}

/// Prints a percentile table for every recorded metric to stderr, then clears
/// them. Called once at the end of a scripted-scroll run and on app exit under
/// a plain `LOGLENS_PERF=1`.
pub fn dump() {
    if !enabled() {
        return;
    }
    let mut map = samples().lock().expect("lock perf samples");
    if map.values().all(Vec::is_empty) {
        return;
    }
    eprintln!();
    eprintln!(
        "{:<24} {:>6} {:>10} {:>10} {:>10} {:>10} {:>10}",
        "metric", "n", "p50", "p90", "p99", "max", "mean"
    );
    eprintln!("{}", "-".repeat(24 + 6 + 10 * 5 + 6));
    for (name, xs) in map.iter_mut() {
        if xs.is_empty() {
            continue;
        }
        xs.sort_by(|a, b| a.total_cmp(b));
        let pct = |p: f32| {
            let idx = ((xs.len() as f32 - 1.0) * p).round() as usize;
            xs[idx]
        };
        let mean = xs.iter().sum::<f32>() / xs.len() as f32;
        eprintln!(
            "{:<24} {:>6} {:>8.3}ms {:>8.3}ms {:>8.3}ms {:>8.3}ms {:>8.3}ms",
            name,
            xs.len(),
            pct(0.5),
            pct(0.9),
            pct(0.99),
            xs[xs.len() - 1],
            mean,
        );
    }
    map.clear();
}
