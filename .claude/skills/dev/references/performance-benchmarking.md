# Performance benchmarking

Drives a real window through a fixed scroll and prints per-frame timings.
Needs a display, ~12s per run.

## Procedure

Always A/B — a single run means nothing on its own.

1. Build and run on the code **before** your change; keep the output.
2. Apply the change, rebuild, run the identical command.
3. Compare p50 / p99 / max. Same machine, same fixture, nothing else running.

```sh
cargo build --release

LOGLENS_PERF_SCROLL=1 \
LOGLENS_HITS=benches/fixtures/nginx-800.json \
LOGLENS_PERF_MODE=table \
./target/release/loglens
```

## Environment variables

| var | effect |
|---|---|
| `LOGLENS_PERF=1` | timing only — scroll by hand, numbers print on window close |
| `LOGLENS_PERF_SCROLL=1` | run the scripted scroll (implies `LOGLENS_PERF`) |
| `LOGLENS_PERF_SCROLL_SECS=N` | scroll duration (default 12) |
| `LOGLENS_HITS=<path>` | load a saved `_search` response instead of querying a cluster |
| `LOGLENS_HITS_REPEAT=N` | repeat the fixture `N` times (default 10) to stand in for a larger result set |
| `LOGLENS_PERF_SEARCH=<id or name>` | which Saved Search to open (default: first configured) |
| `LOGLENS_PERF_MODE=table\|text` | force the tab's Layout mode |
| `LOGLENS_PERF_WRAP=1` | force line wrapping on |

## Fixtures

`benches/fixtures/*.json` are saved responses checked into git, so runs are
byte-identical. Always pass one — without `LOGLENS_HITS` it queries a live
cluster and the result set drifts between runs. `nginx-800.json` is the
Table-mode case; `payloads-150.json` is the raw-text worst case, run it with
`LOGLENS_PERF_MODE=text`.

## Reporting results

Paste both tables, baseline first, naming the fixture, mode, and machine.
`perf.frame_interval` decides it: near 16.7ms with a tight p99 is 60Hz; p99 or
max well above it means dropped frames. Call a change a win only when p99 and
max improve — p50 alone hides stutter.
