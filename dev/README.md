# dev — local Elasticsearch for testing Log Lens

A throwaway single-node Elasticsearch plus a generator that keeps several data
streams topped up with synthetic log Hits, so you can point Log Lens at a real
cluster. The streams are deliberately awkward — multi-line messages with both
Linux (LF) and Windows (CRLF) newlines, and messages thousands of characters
long — so the renderer gets exercised, not just the happy path.

Not part of the app build. Requires Docker + Docker Compose.

## Start

```sh
cd dev
docker compose up -d
docker compose logs -f generator   # watch it backfill and then trickle
```

First run pulls ~1.5 GB of images. The generator backfills ~60 min of history,
then indexes 20 Hits every 3 s, spread across all streams. Tune with `RATE` /
`INTERVAL` / `BACKFILL_MINUTES` in `docker-compose.yml`, rename the streams with
`DATA_STREAM_PREFIX`, or generate only some of them with e.g. `STREAMS=java,payloads`.

## The data streams

| Data stream            | Share | What it looks like |
| ---------------------- | ----- | ------------------ |
| `logs-loglens-app`     | ~50%  | one-line service logs; the boring baseline |
| `logs-loglens-nginx`   | ~25%  | combined access logs; ~15% carry a several-KB query string and ~8% a huge user agent, all on a single line |
| `logs-loglens-java`    | ~12%  | multi-line stack traces joined with `\n`, up to ~45 lines, with `Caused by:` / `Suppressed:` sections |
| `logs-loglens-winevent`| ~9%   | Windows event log and PowerShell error text joined with `\r\n`, including blank lines |
| `logs-loglens-payloads`| ~4%   | very long messages: pretty-printed JSON and SQL (LF, hundreds of lines), an HTTP exchange dump (CRLF), and a single unbroken base64 line of 4,000–20,000 characters |

Every doc also carries `message_chars`, `line_count`, and `newline_style`
(`none` / `lf` / `crlf`), so you can find the nasty ones directly:
`message_chars:>5000`, `line_count:>20`, `newline_style:crlf`.

Optional Kibana at http://localhost:5601:

```sh
docker compose --profile kibana up -d
```

## Point Log Lens at it

Run the app (`cargo run`), then in the Elasticsearch tree:

1. **＋** → Add Connection
   - Name: `local`
   - URL: `http://localhost:9200`
   - Auth: **None**
   - Test → should report the cluster name and version.
2. Add a Saved Search on that Connection:
   - Target: one of `logs-loglens-app` / `-nginx` / `-java` / `-winevent` /
     `-payloads`, or `logs-loglens-*` for all of them (the typeahead offers
     them from `_data_stream`, since the backing indices are hidden)
   - Query string: empty, or try `level:ERROR`, `service:payments`,
     `status:>=500`, `duration_ms:>400`, `line_count:>20`,
     `message_chars:>5000`, `newline_style:crlf`
   - Timeframe: Last 15 minutes (default)
   - Columns: `@timestamp`, `message`; add `level`, `service`, `status`,
     `duration_ms`, `line_count`, `message_chars` to exercise sorting

Fields on every doc: `@timestamp`, `message`, `level`, `service`, `host`,
`trace_id`, `line_count`, `message_chars`, `newline_style`. Per-flavour extras:
`duration_ms` / `status` (app, nginx), `client_ip` / `method` / `path` /
`bytes` (nginx), `logger` / `thread` / `exception` (java), `channel` /
`provider` / `event_id` (winevent), `payload_kind` (payloads).

## Stop

```sh
docker compose down          # keep nothing running
docker compose down -v       # also drop the indexed data (there are no volumes,
                             # so a plain `down` already discards everything)
```

## Notes

- Security is disabled, so this only exercises the `Auth::None` path. To test
  Basic auth / API keys / `skip_tls_verify`, set `xpack.security.enabled=true`
  and add credentials — not wired up here.
- The generator writes with the `create` bulk op into the `logs-loglens-*`
  data streams, all backed by one index template it installs on startup. The
  streams are created on first write, so a stream you excluded via `STREAMS`
  simply never appears.
