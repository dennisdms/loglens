# dev — local Elasticsearch for testing Log Lens

A throwaway single-node Elasticsearch plus a generator that keeps a data stream
topped up with synthetic log Hits, so you can point Log Lens at a real cluster.

Not part of the app build. Requires Docker + Docker Compose.

## Start

```sh
cd dev
docker compose up -d
docker compose logs -f generator   # watch it backfill and then trickle
```

First run pulls ~1.5 GB of images. The generator backfills ~60 min of history,
then indexes 20 Hits every 3 s. Tune with `RATE` / `INTERVAL` /
`BACKFILL_MINUTES` in `docker-compose.yml`.

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
   - Target: `logs-loglens` (offered by the typeahead — it comes from
     `_data_stream`, since the backing indices are hidden)
   - Query string: empty, or try `level:ERROR`, `service:payments`,
     `status:>=500`, `duration_ms:>400`
   - Timeframe: Last 15 minutes (default)
   - Columns: `@timestamp`, `message`; add `level`, `service`, `status`,
     `duration_ms` to exercise sorting

Fields available for columns / sorting: `@timestamp`, `message`, `level`,
`service`, `host`, `trace_id`, `duration_ms`, `status`.

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
- The generator writes with the `create` bulk op into data stream
  `logs-loglens`, backed by an index template it installs on startup.
