//! The only place in Log Lens that speaks Elasticsearch.
//!
//! Everything outside this module deals in the typed values defined here and
//! never sees `reqwest`, a REST path, or a request body. [`Client`] is a
//! connected cluster; [`Query`] is what to look for; [`Run`] is a Search being
//! paged, and owns the whole `search_after` protocol — the cursor, the Page
//! size, and when to stop. Requests are built by hand against a fixed handful
//! of REST endpoints, per ADR 0001; paging follows ADR 0002.

mod run;
mod wire;

use std::sync::Arc;

use serde_json::Value;

pub use run::{Advance, Run, State};

/// Elasticsearch's own ceiling on how many documents one `_search` may return.
pub const FETCH_SIZE_MAX: usize = 10_000;

/// Everything needed to reach a cluster, secrets included.
#[derive(Debug, Clone)]
pub struct Endpoint {
    pub url: String,
    pub auth: AuthValue,
    pub skip_tls_verify: bool,
}

/// Auth scheme plus its resolved secret material.
#[derive(Debug, Clone)]
pub enum AuthValue {
    None,
    Basic { username: String, password: String },
    ApiKey { key: String },
}

/// What `GET /` tells us about a reachable cluster.
#[derive(Debug, Clone)]
pub struct ClusterInfo {
    pub cluster_name: String,
    pub version: String,
}

/// What `_field_caps` tells us about a Target's fields.
#[derive(Debug, Clone, Default)]
pub struct FieldCaps {
    /// Every field name, sorted. Any of these may be a Column.
    pub all: Vec<String>,
    /// The subset that can be sorted on (keyword / numeric / date / boolean /
    /// ip — never analysed text).
    pub sortable: Vec<String>,
}

/// One Hit: its `_source`, plus the opaque values that page past it. Those
/// are private — paging past a Hit is [`Run`]'s business, not a caller's.
#[derive(Debug, Clone)]
pub struct Hit {
    pub source: Value,
    sort: Vec<Value>,
}

#[cfg(test)]
impl Hit {
    /// A Hit with nothing to page past it, for tests that only need a
    /// `_source` to render.
    pub fn detached(source: Value) -> Hit {
        Hit {
            source,
            sort: Vec::new(),
        }
    }
}

/// Everything that decides which Hits a Search returns.
#[derive(Debug, Clone)]
pub struct Query {
    pub target: String,
    /// Empty matches everything.
    pub query_string: String,
    pub timestamp_field: String,
    /// Timeframe bounds — Elasticsearch date-math (`now-15m`) or ISO
    /// timestamps.
    pub gte: String,
    pub lte: String,
    /// Sort keys as `(field, descending)`, highest priority first. Empty sorts
    /// by `timestamp_field` descending; a tiebreaker is always added on top, so
    /// paging is totally ordered either way.
    pub sort: Vec<(String, bool)>,
}

impl Query {
    /// The sort keys to send, with the default applied.
    fn sort_keys(&self) -> Vec<(String, bool)> {
        if self.sort.is_empty() {
            vec![(self.timestamp_field.clone(), true)]
        } else {
            self.sort.clone()
        }
    }
}

/// How far a [`Run`] may go: the Fetch size it pages in, and the Max Results it
/// stops at.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    fetch_size: usize,
    max_results: usize,
}

impl Limits {
    /// `fetch_size` is clamped to [`FETCH_SIZE_MAX`]; both are at least 1.
    pub fn new(fetch_size: usize, max_results: usize) -> Limits {
        Limits {
            fetch_size: fetch_size.clamp(1, FETCH_SIZE_MAX),
            max_results: max_results.max(1),
        }
    }
}

/// Why a call to a cluster did not produce what was asked for.
#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    /// The cluster could not be reached at all — DNS, TLS, connect, timeout.
    Unreachable(String),
    /// The cluster rejected our credentials.
    Unauthorized(String),
    /// No such index, data stream, or pattern.
    NoSuchTarget(String),
    /// The cluster rejected the request itself.
    Rejected(String),
    /// The cluster answered with something we cannot read.
    Malformed(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (Error::Unreachable(message)
        | Error::Unauthorized(message)
        | Error::NoSuchTarget(message)
        | Error::Rejected(message)
        | Error::Malformed(message)) = self;
        f.write_str(message)
    }
}

/// A cluster, connected. Holds one HTTP client, so every call over it reuses
/// the connection pool and the TLS session; cloning is cheap.
#[derive(Clone)]
pub struct Client(Arc<Inner>);

struct Inner {
    http: reqwest::Client,
    /// The Connection's URL, trailing slash trimmed.
    base: String,
    auth: AuthValue,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // A Client reaches `Message`, which is `Debug`. Auth stays out of it.
        f.debug_struct("Client")
            .field("base", &self.0.base)
            .finish()
    }
}

impl Client {
    pub fn connect(endpoint: Endpoint) -> Result<Client, Error> {
        let mut builder = reqwest::Client::builder();
        if endpoint.skip_tls_verify {
            builder = builder.danger_accept_invalid_certs(true);
        }
        let http = builder
            .build()
            .map_err(|e| Error::Unreachable(e.to_string()))?;
        Ok(Client(Arc::new(Inner {
            http,
            base: endpoint.url.trim().trim_end_matches('/').to_string(),
            auth: endpoint.auth,
        })))
    }

    /// `GET /` — the cluster's name and version. Also the Connection form's
    /// Test: reaching it at all is the answer.
    pub async fn ping(&self) -> Result<ClusterInfo, Error> {
        let body = self.send(self.get("")).await?;
        wire::cluster_info(&body)
    }

    /// The index and data stream names a Target typeahead offers, sorted and
    /// deduplicated. Best-effort by design: an endpoint the cluster does not
    /// serve, or answers unreadably, contributes nothing rather than failing
    /// the lot, so a partial list still reaches the user.
    pub async fn targets(&self) -> Vec<String> {
        let mut names: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

        if let Ok(body) = self
            .send(self.get("_cat/indices?h=index&format=json"))
            .await
        {
            names.extend(wire::cat_indices(&body));
        }
        if let Ok(body) = self.send(self.get("_data_stream")).await {
            names.extend(wire::data_streams(&body));
        }
        names.into_iter().collect()
    }

    /// The fields a Target carries. Doubles as an existence probe: a Target
    /// that is not there comes back [`Error::NoSuchTarget`].
    pub async fn fields(&self, target: &str) -> Result<FieldCaps, Error> {
        let path = format!("{}/_field_caps?fields=*", segment(target));
        let body = self.send(self.get(&path)).await?;
        wire::field_caps(&body)
    }

    /// How many Hits match, independent of paging and Max Results.
    pub async fn total(&self, query: &Query) -> Result<u64, Error> {
        let path = format!("{}/_count", segment(&query.target));
        let body = self
            .send(self.post(&path, &wire::count_body(query)))
            .await?;
        wire::count(&body)
    }

    /// One Page. Private to `es`: callers page through a [`Run`], which owns
    /// the cursor and the size.
    async fn search(
        &self,
        query: &Query,
        size: usize,
        after: Option<&[Value]>,
    ) -> Result<Vec<Hit>, Error> {
        let path = format!("{}/_search", segment(&query.target));
        let body = wire::search_body(query, size, after);
        let body = self.send(self.post(&path, &body)).await?;
        wire::hits(&body)
    }

    fn get(&self, path: &str) -> reqwest::RequestBuilder {
        self.with_auth(self.0.http.get(self.url(path)))
    }

    fn post(&self, path: &str, body: &Value) -> reqwest::RequestBuilder {
        self.with_auth(self.0.http.post(self.url(path)).json(body))
    }

    fn url(&self, path: &str) -> String {
        if path.is_empty() {
            self.0.base.clone()
        } else {
            format!("{}/{path}", self.0.base)
        }
    }

    fn with_auth(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.0.auth {
            AuthValue::None => request,
            AuthValue::Basic { username, password } => request.basic_auth(username, Some(password)),
            AuthValue::ApiKey { key } => {
                request.header(reqwest::header::AUTHORIZATION, format!("ApiKey {key}"))
            }
        }
    }

    /// Sends a request and returns its body, mapping transport failures and
    /// anything non-2xx onto [`Error`].
    async fn send(&self, request: reqwest::RequestBuilder) -> Result<String, Error> {
        let response = request
            .send()
            .await
            .map_err(|e| Error::Unreachable(e.to_string()))?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| Error::Unreachable(e.to_string()))?;
        if !status.is_success() {
            return Err(wire::error(status.as_u16(), &body));
        }
        Ok(body)
    }
}

/// A Target as one path segment: no leading or trailing slash to double up on
/// the base URL's.
fn segment(target: &str) -> &str {
    target.trim().trim_matches('/')
}
