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

**Max Results**:
The maximum number of Hits a Result Tab will load (10,000 by default, set in the
Settings window). On reaching it the tab stops paging and tells the user to
narrow the query. Formerly a fixed "retention cap".
_Avoid_: retention cap, limit, buffer size

**Fetch size**:
How many documents Log Lens pulls per `_search` request while paging (1,000 by
default, capped at 10,000 — Elasticsearch's own per-request ceiling). Set in the
Settings window.
_Avoid_: page size, batch size

### Display

**Menu bar**:
A strip across the top of the window holding application-wide actions, grouped as
menus (File, View). Always present, independent of which Saved Search is open.
`File` opens a dropdown with `Settings`; `View` is still inert.
_Avoid_: toolbar, ribbon

**Settings window**:
A separate OS window (File > Settings) for application-wide configuration, apart
from any Saved Search. One page so far — Elasticsearch — carrying Max Results and
Fetch size. Saving applies the new limits to every open Result Tab.
_Avoid_: preferences pane, settings tab, modal

**Options strip**:
The row of display controls directly above an active Result Tab: sort fields, the
Table/Text mode toggle, the Highlight rules editor, and — in Text mode only — the
Format modal. Shown once a run has loaded.
_Avoid_: toolbar

**Format modal**:
Where a Result Tab's Text-mode template is edited: the `%{field.path}` template
input, a reference list of the fields available in the current search, and a live
preview of the first Hits that re-renders as the template is typed. Opened from
the options strip; only reachable in Text mode.

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

**Layout**:
How a Result Tab draws its Hits: either as Columns (the table) or as a template
(raw text). Both are kept, so switching modes never discards the other's
settings. Belongs to a Saved Search; the render-time value is assembled from it
plus the timestamp field and the UTC preference. The template is edited in the
Format modal, opened from the options strip in Text mode.
_Avoid_: format, view mode, display settings

**Line**:
One Hit rendered for display: an ordered list of Parts. A Columns Layout gives
one Part per Column; a template Layout gives one. The table draws Part _i_ into
column _i_; raw text mode concatenates them; GREP matches against their text.
_Avoid_: row, formatted hit, output

**Part**:
One addressable piece of a Line — a Column's worth of text under a Columns
Layout, or the whole rendered line under a template. Holds one or more Segments.

**Segment**:
A run of text within a Part carrying one style. A Part is a single Segment until
a Highlight rule splits it. Named to stay out of the way of iced's own `Span`.
_Avoid_: span, chunk, token

**Highlight rule**:
A rule that colours a Line. Matches either a field predicate (`level == ERROR`),
which colours the whole Line, or a text pattern, which colours just the Segments
it matches. Ordered: the first matching field predicate wins, and text patterns
layer over it. Global to the application, not per Saved Search. Edited from the
options strip's "Highlight rules" button (both Table and Text mode).
_Avoid_: filter, formatter, theme rule

### Distribution

**Release**:
One published version of Log Lens, carrying an Artifact per Install flavour,
their checksums, and notes describing what changed. Cut by tagging; a Release
is never visible to users until every one of its Artifacts is present.
_Avoid_: build, version (when the set of files is meant), drop

**Artifact**:
One downloadable file belonging to a Release — the Windows installer, the
Windows portable archive, or the Linux archive. Named to a fixed convention
that the Update check depends on, so the names are a compatibility contract
rather than a formatting choice: `LogLens-<version>-<os>-x86_64` carrying the
flavour last — `-setup.exe`, `-portable.zip`, `.tar.gz` — plus one
`SHA256SUMS` covering them.
_Avoid_: asset, package, binary

**Install flavour**:
How a copy of Log Lens got onto the machine. **Installer-managed** — placed by
the Windows installer or the Linux archive's install script, in a directory it
owns and can rewrite. **Portable** — unpacked by hand, owned by nobody. Only an
installer-managed copy can update itself; a portable one is told about a new
Release and left alone. Told apart by an `install-manifest.json` that records
the directory the installer wrote to: a copy is installer-managed only while
that recorded directory is the one it is running from, so a portable copy
sitting beside an installed one is not mistaken for it.
_Avoid_: install type, install mode, channel

**Update check**:
Asking GitHub whether a Release newer than the running version exists. Happens
at most once a day on startup, and on demand from the Help menu. Pre-releases
are never offered.
_Avoid_: version check, poll

**Update** (verb):
Replacing the running copy with a newer Release by running that Release's own
installer, so shortcuts, launcher entries and icons move with the binary
instead of drifting away from it. Never touches the user's Connections,
settings, or stored secrets.
_Avoid_: upgrade, patch, self-update
