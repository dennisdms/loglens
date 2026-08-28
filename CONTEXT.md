# Log Lens

A cross-platform desktop IDE for browsing logs, built in Rust with iced. Its first
iteration is built around querying Elasticsearch.

## Language

### Log sources

**Elasticsearch**:
The root node of the left-hand tree. A fixed container that holds every
Connection the user has configured.

**Connection**:
A named Elasticsearch endpoint plus the credentials and TLS settings needed to
reach it. Owns a set of Saved Searches. Persisted between runs; secrets are held
separately from the rest of its configuration.
_Avoid_: cluster, server, host, profile

**Saved Search**:
A persisted, named query belonging to one Connection: a target, a query string, a
timeframe, and a sort. Opening one runs it and shows the results in a Result Tab.
_Avoid_: query, view, filter, saved query

**Target**:
The index, data stream, or index pattern a Saved Search runs against (e.g.
`logs-app-prod`, `logs-*`). Chosen when the Saved Search is created.
_Avoid_: index (when a data stream or pattern may be meant), source

**Sample Logs**:
The other top-level tree root: bundled fake log files used to exercise the UI
before real local-file browsing exists. Unrelated to Elasticsearch.

### Querying

**Search** (verb):
Executing a Saved Search's query against its Connection.

**Query string**:
The Lucene-syntax expression a user types into a Saved Search, passed through to
Elasticsearch's `query_string` query. An empty query string matches everything.
_Avoid_: filter, search text, DSL

**Timeframe**:
The time window a Saved Search restricts results to — either relative (e.g. "last
15 minutes") or an absolute start/end. Applied as a range filter on a timestamp
field.
_Avoid_: date range, period, interval

**Hit**:
One matching Elasticsearch document. Rendered as one row in a Result Tab.
_Avoid_: record, entry, result, log line

**Page**:
One batch of Hits fetched in a single request. A Result Tab starts with one Page
and appends more as the user scrolls.
_Avoid_: batch, chunk, scroll

**Point-in-Time**:
A frozen view of the Target that a Result Tab holds open for the life of the tab,
so that paging through Hits stays consistent even as new documents are indexed.
Abbreviated PIT.
_Avoid_: snapshot, cursor, scroll context

**Retention cap**:
The maximum number of Hits a Result Tab will load (10,000 by default). On
reaching it the tab stops paging and tells the user to narrow the query.
_Avoid_: limit, max results, buffer size

### Display

**Result Tab**:
A tab in the main area showing the Hits from one Saved Search as a virtualized
table. A distinct kind of tab from the file tabs used for Sample Logs.
_Avoid_: results pane, grid, output

**Column**:
A field projected out of each Hit into its own table column in a Result Tab.
