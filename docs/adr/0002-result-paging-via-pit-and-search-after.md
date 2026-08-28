# Result paging via Point-in-Time and search_after

## Context

A Result Tab loads Hits in Pages of 1,000 and appends more as the user scrolls,
up to a Retention cap of 10,000. Documents are being indexed continuously while
the user reads, and the chosen sort field is arbitrary (often not unique).

## Decision

On a Result Tab's first run, open a Point-in-Time (PIT) against the Target and
hold it for the life of the tab. Page with `search_after`, using the sort
`[<chosen field>:<dir>, _shard_doc]` — `_shard_doc` is always appended as a
tiebreaker so paging is total-ordered and deterministic. Close the PIT when the
tab closes. Changing the sort, hitting Refresh, or saving an edit to the Saved
Search discards the PIT and starts a fresh one.

Stop paging hard at 10,000 Hits and show a footer telling the user to narrow the
query. Render the table as a windowed slice (~200 rows) shifted on scroll rather
than as 10,000 live widgets.

## Considered options

- **`from` / `size` paging.** Rejected: capped at `index.max_result_window`
  (10,000) and re-sorts the whole result set on every page — increasingly
  expensive and inconsistent as data is indexed.
- **Scroll API.** Rejected: designed for full exports, not interactive reads;
  heavier server-side state than PIT and awkward to abandon mid-way.
- **No tiebreaker (sort on the user's field alone).** Rejected: `search_after`
  needs a total order; on a non-unique field it would skip or repeat Hits at
  Page boundaries.
- **Sliding 10k window (evict oldest Pages, keep paging).** Rejected for v1:
  scrolling back up past an evicted region needs backward `search_after`, and a
  tab that silently drops the earliest Hits misrepresents what the user is
  looking at. A hard stop is simpler and honest.

## Consequences

- Each open Result Tab holds server-side PIT state; the client must close PITs on
  tab close and refresh `keep_alive` while paging.
- 10,000 Hits is a firm ceiling per tab in v1. Narrowing the Timeframe or query
  string is the only way to see beyond it.
