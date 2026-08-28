# Elasticsearch access via raw REST over reqwest

## Context

Log Lens's first iteration queries Elasticsearch. The app today is a fully
synchronous iced 0.14 program with no async runtime and no HTTP client, so this
feature has to bring both in.

## Decision

Talk to Elasticsearch with `reqwest` (json + rustls) against a small, fixed set
of REST endpoints (`GET /`, `_cat/indices`, `_data_stream`, `_field_caps`,
`_pit`, `_search`), building request bodies by hand. All of it lives behind an
`es` module that exposes a typed client; nothing outside that module sees HTTP.
Add `tokio` (rt + macros) as the runtime and drive every call through iced's
`Task::perform`.

## Considered options

- **The official `elasticsearch` crate.** Rejected: it has been published only as
  alpha for years, is generated against ES 8 type-by-type, and pulls a large
  dependency tree. We touch six endpoints — the crate's surface is far more than
  we need and its release status is a risk for a shipping app.
- **A blocking HTTP client on a worker thread.** Rejected: iced already has a
  first-class async story via `Task`, and Elasticsearch paging is naturally a
  sequence of awaited requests. A bespoke threading layer would be more code and
  more error-prone than `tokio` + `Task::perform`.

## Consequences

- `tokio` is now a hard dependency of a previously runtime-free app.
- We own the request/response types for each endpoint, including ES version
  differences. This is deliberate: it keeps the dependency surface small and the
  wire format visible.
