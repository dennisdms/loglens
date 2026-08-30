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
A persisted, named query belonging to one Connection: a Target, a query string, a
timeframe, a set of Columns, and a sort. Opening one runs it and shows the results
in a Result Tab. Its Target, query string, timeframe, Columns and sort can all be
changed from the Search bar while viewing that tab; those changes are saved back
automatically.
_Avoid_: query, view, filter, saved query

**Target**:
The index, data stream, or index pattern a Saved Search runs against (e.g.
`logs-app-prod`, `logs-*`). Set when the Saved Search is created and re-pointed
from the Search bar afterwards, with a typeahead over the Connection's indices
and data streams. Changing it re-runs the Search against the new Target.
_Avoid_: index (when a data stream or pattern may be meant), source

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
One batch of Hits fetched in a single `_search` request. A Result Tab starts
with one Page and appends more as the user scrolls, each one an independent
`search_after` call past the last Hit's sort values.
_Avoid_: batch, chunk, scroll

**Retention cap**:
The maximum number of Hits a Result Tab will load (10,000 by default). On
reaching it the tab stops paging and tells the user to narrow the query.
_Avoid_: limit, max results, buffer size

### Display

**Menu bar**:
A strip across the top of the window holding application-wide actions, grouped as
menus (File, View). Always present, independent of which Saved Search is open.
_Avoid_: toolbar, ribbon

**Search bar**:
The controls for the Saved Search whose Result Tab is currently active, shown
above the tab strip: its Target, query string, timeframe, loaded Hit count, and
Columns. Editing the Target, query string, timeframe or Columns re-runs the
Search. Hidden when no Result Tab is active.
_Avoid_: filter bar, query bar, toolbar

**Result Tab**:
A tab in the main area showing the Hits from one Saved Search as a virtualized
table. Its Target, query string, timeframe and Columns are edited in the Search
bar above it.
_Avoid_: results pane, grid, output

**Search settings**:
The name and timestamp field of a Saved Search — the parts set outside the Search
bar. Edited in a modal for an existing Saved Search; the creation form also
carries the Target (the one setting the modal drops, since the Target is
re-pointed from the Search bar). A new Saved Search's query string, timeframe,
Columns and sort take defaults, tuned from the Search bar afterwards.
_Avoid_: properties, config, options

**Column**:
A field projected out of each Hit into its own table column in a Result Tab.
Chosen in the Search bar.
