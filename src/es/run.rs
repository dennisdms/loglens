//! Paging one Search: the `search_after` protocol, the Fetch size / Max
//! Results arithmetic, and the two places Hits come from.

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{Client, Error, Hit, Limits, Query, wire};

/// One execution of a Saved Search: the sequence of Pages from the first
/// `_search` through each `search_after` continuation, up to Max Results.
///
/// Callers hold a Run and ask it for Pages. Where the cursor came from, how
/// big the next Page should be, and when to stop are all its business.
#[derive(Debug, Clone)]
pub struct Run {
    hits_from: Hits,
    /// Hits handed out so far, across every Page.
    loaded: usize,
    /// The `sort` values of the last Hit — where the next Page starts.
    cursor: Option<Vec<Value>>,
    /// Set once the Run stops. Asking again just repeats it.
    end: Option<End>,
}

#[derive(Debug, Clone)]
enum Hits {
    Live {
        client: Client,
        query: Query,
        limits: Limits,
    },
    /// The scroll-perf harness's stand-in: a saved `_search` body on disk, so
    /// a run is byte-identical every time and needs no cluster at all. See
    /// `crate::perf`.
    Fixture { path: PathBuf, repeat: usize },
}

/// Why a Run stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum End {
    Exhausted,
    Capped,
}

/// What one call to [`Run::next_page`] produced.
#[derive(Debug, Clone)]
pub struct Advance {
    /// The Run to keep for the next call. Returned even on failure, so a Page
    /// that failed can be retried.
    pub run: Run,
    /// The Hits this Page carried. Empty when the Page failed, and when the
    /// Run had already stopped.
    pub hits: Vec<Hit>,
    pub state: State,
}

/// Where a Run stands after a Page.
#[derive(Debug, Clone, PartialEq)]
pub enum State {
    /// More Hits may follow; ask again.
    More,
    /// Every Hit matching the Query has been loaded.
    Exhausted,
    /// Max Results reached. The cluster may hold more.
    Capped,
    /// The Page could not be fetched. Retry by asking again.
    Failed(Error),
}

impl From<End> for State {
    fn from(end: End) -> State {
        match end {
            End::Exhausted => State::Exhausted,
            End::Capped => State::Capped,
        }
    }
}

impl Run {
    pub fn live(client: Client, query: Query, limits: Limits) -> Run {
        Run::new(Hits::Live {
            client,
            query,
            limits,
        })
    }

    /// A Run that reads its Hits from a saved `_search` body instead of a
    /// cluster, `repeat` copies of it concatenated. For the scroll-perf
    /// harness only (`crate::perf`).
    pub fn fixture(path: PathBuf, repeat: usize) -> Run {
        Run::new(Hits::Fixture { path, repeat })
    }

    fn new(hits_from: Hits) -> Run {
        Run {
            hits_from,
            loaded: 0,
            cursor: None,
            end: None,
        }
    }

    /// Re-limits a Run already under way, so the Settings window's Max Results
    /// and Fetch size reach Result Tabs that are already open. A Run that has
    /// already stopped stays stopped.
    pub fn relimit(&mut self, new: Limits) {
        if let Hits::Live { limits, .. } = &mut self.hits_from {
            *limits = new;
        }
    }

    /// Fetches the next Page.
    pub async fn next_page(mut self) -> Advance {
        if let Some(end) = self.end {
            return self.stopped(end, Vec::new());
        }

        let asked = match &self.hits_from {
            Hits::Live { limits, .. } => page_size(self.loaded, limits),
            Hits::Fixture { .. } => usize::MAX,
        };
        if asked == 0 {
            return self.stop(End::Capped, Vec::new());
        }

        let fetched = match &self.hits_from {
            Hits::Live { client, query, .. } => {
                client.search(query, asked, self.cursor.as_deref()).await
            }
            Hits::Fixture { path, repeat } => load_fixture(path, *repeat),
        };
        let hits = match fetched {
            Ok(hits) => hits,
            Err(err) => {
                return Advance {
                    run: self,
                    hits: Vec::new(),
                    state: State::Failed(err),
                };
            }
        };

        let got = hits.len();
        self.loaded += got;
        self.cursor = cursor(&hits);

        let end = match &self.hits_from {
            Hits::Fixture { .. } => Some(End::Exhausted),
            Hits::Live { limits, .. } => settle(self.loaded, got, asked, limits)
                // Without sort values there is nothing to page past.
                .or_else(|| self.cursor.is_none().then_some(End::Exhausted)),
        };
        match end {
            Some(end) => self.stop(end, hits),
            None => Advance {
                run: self,
                hits,
                state: State::More,
            },
        }
    }

    fn stop(mut self, end: End, hits: Vec<Hit>) -> Advance {
        self.end = Some(end);
        self.stopped(end, hits)
    }

    fn stopped(self, end: End, hits: Vec<Hit>) -> Advance {
        Advance {
            run: self,
            hits,
            state: end.into(),
        }
    }
}

/// How many documents to ask for next: a whole Fetch size, or just the
/// allowance left under Max Results.
fn page_size(loaded: usize, limits: &Limits) -> usize {
    limits
        .max_results
        .saturating_sub(loaded)
        .min(limits.fetch_size)
}

/// Where a Run stands once a Page of `got` Hits has landed, bringing the total
/// to `loaded`. A Page shorter than what was asked for means the cluster had
/// nothing more to give.
fn settle(loaded: usize, got: usize, asked: usize, limits: &Limits) -> Option<End> {
    if loaded >= limits.max_results {
        Some(End::Capped)
    } else if got < asked {
        Some(End::Exhausted)
    } else {
        None
    }
}

/// The `search_after` values that page past the last Hit.
fn cursor(hits: &[Hit]) -> Option<Vec<Value>> {
    hits.last()
        .map(|hit| hit.sort.clone())
        .filter(|sort| !sort.is_empty())
}

fn load_fixture(path: &Path, repeat: usize) -> Result<Vec<Hit>, Error> {
    // Not a cluster, but the harness reports it the same way.
    let body = std::fs::read_to_string(path).map_err(|e| Error::Unreachable(e.to_string()))?;
    let hits = wire::hits(&body)?;
    if repeat <= 1 {
        return Ok(hits);
    }
    let mut repeated = Vec::with_capacity(hits.len() * repeat);
    for _ in 0..repeat {
        repeated.extend(hits.iter().cloned());
    }
    Ok(repeated)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits(fetch_size: usize, max_results: usize) -> Limits {
        Limits::new(fetch_size, max_results)
    }

    fn hit(sort: Vec<Value>) -> Hit {
        Hit {
            source: Value::Null,
            sort,
        }
    }

    #[test]
    fn a_full_fetch_size_is_asked_for_while_the_allowance_is_wide_enough() {
        assert_eq!(page_size(0, &limits(1000, 10_000)), 1000);
        assert_eq!(page_size(8000, &limits(1000, 10_000)), 1000);
    }

    #[test]
    fn the_last_page_shrinks_to_the_allowance_left_under_max_results() {
        assert_eq!(page_size(9700, &limits(1000, 10_000)), 300);
    }

    #[test]
    fn nothing_is_asked_for_once_max_results_is_reached() {
        assert_eq!(page_size(10_000, &limits(1000, 10_000)), 0);
        assert_eq!(page_size(12_000, &limits(1000, 10_000)), 0);
    }

    #[test]
    fn a_full_page_short_of_max_results_leaves_the_run_open() {
        assert_eq!(settle(1000, 1000, 1000, &limits(1000, 10_000)), None);
    }

    #[test]
    fn a_short_page_exhausts_the_run() {
        assert_eq!(
            settle(400, 400, 1000, &limits(1000, 10_000)),
            Some(End::Exhausted)
        );
    }

    #[test]
    fn a_page_that_lands_exactly_on_max_results_caps_rather_than_exhausts() {
        assert_eq!(
            settle(10_000, 1000, 1000, &limits(1000, 10_000)),
            Some(End::Capped)
        );
    }

    #[test]
    fn a_shrunk_last_page_that_comes_back_full_caps_rather_than_exhausts() {
        // Asked for the 300 left under the cap and got all 300: capped, even
        // though 300 is short of the Fetch size.
        assert_eq!(
            settle(10_000, 300, 300, &limits(1000, 10_000)),
            Some(End::Capped)
        );
    }

    #[test]
    fn the_cursor_is_the_last_hits_sort_values() {
        let hits = vec![hit(vec![Value::from(1)]), hit(vec![Value::from(2)])];
        assert_eq!(cursor(&hits), Some(vec![Value::from(2)]));
    }

    #[test]
    fn there_is_no_cursor_without_sort_values_to_page_past() {
        assert_eq!(cursor(&[]), None);
        assert_eq!(cursor(&[hit(Vec::new())]), None);
    }

    #[tokio::test]
    async fn a_fixture_run_hands_over_every_hit_at_once_and_then_stops() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/benches/fixtures/nginx-800.json"
        );
        let advance = Run::fixture(PathBuf::from(path), 2).next_page().await;
        assert_eq!(advance.hits.len(), 1600);
        assert_eq!(advance.state, State::Exhausted);

        let advance = advance.run.next_page().await;
        assert!(advance.hits.is_empty());
        assert_eq!(advance.state, State::Exhausted);
    }

    #[tokio::test]
    async fn a_missing_fixture_fails_the_page_and_hands_the_run_back() {
        let advance = Run::fixture(PathBuf::from("/nope/missing.json"), 1)
            .next_page()
            .await;
        assert!(matches!(advance.state, State::Failed(_)));
        assert!(advance.hits.is_empty());
    }
}
