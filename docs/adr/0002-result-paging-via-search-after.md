# Result paging via search_after

## Context

A Result Tab loads Hits in Pages of 1,000 and appends more as the user scrolls,
up to a Retention cap of 10,000. Documents are being indexed continuously while
the user reads, and the chosen sort field is arbitrary (often not unique).

## Decision

Page with `search_after` against the Target's `_search` endpoint, using the sort
`[<chosen field>:<dir>, _doc]` — `_doc` is always appended as a tiebreaker so
paging is total-ordered and won't skip or repeat Hits at Page boundaries. Each
Page is an independent `_search` call; there is no server-side cursor to open,
refresh, or release. Changing the sort, editing the query string or Timeframe in
the Search bar, hitting Refresh, or saving an edit to the Saved Search just
re-runs from the first Page.

Stop paging hard at 10,000 Hits and show a footer telling the user to narrow the
query. Render the table as a windowed slice (~200 rows) shifted on scroll rather
than as 10,000 live widgets.

## Considered options

- **`from` / `size` paging.** Rejected: capped at `index.max_result_window`
  (10,000) and re-sorts the whole result set on every page — increasingly
  expensive and inconsistent as data is indexed.
- **Scroll API.** Rejected: designed for full exports, not interactive reads;
  heavy server-side state and awkward to abandon mid-way.
- **Point-in-Time (PIT) + `search_after`.** Gives a frozen view so paging can't
  drift as data is indexed, and unlocks the `_shard_doc` tiebreaker. Rejected:
  every open Result Tab would hold server-side PIT state that the client has to
  close on tab close and keep alive while paging, and the Search bar makes the
  query and Timeframe editable, so an exploratory session churns through a PIT
  per change. For an interactive log reader the small chance of a boundary
  Hit shifting between Pages isn't worth that bookkeeping.
- **No tiebreaker (sort on the user's field alone).** Rejected: `search_after`
  needs a total order; on a non-unique field it would skip or repeat Hits at
  Page boundaries.
- **Sliding 10k window (evict oldest Pages, keep paging).** Rejected for v1:
  scrolling back up past an evicted region needs backward `search_after`, and a
  tab that silently drops the earliest Hits misrepresents what the user is
  looking at. A hard stop is simpler and honest.

## Consequences

- No server-side state per tab: closing a tab or connection just drops the
  in-memory Hits.
- Paging is not snapshot-isolated. Documents indexed into the Timeframe's range
  while the user pages can shift Hits near a Page boundary, occasionally causing
  a Hit to be skipped or shown twice. Acceptable for an interactive reader.
- 10,000 Hits is a firm ceiling per tab in v1. Narrowing the Timeframe or query
  string is the only way to see beyond it.
