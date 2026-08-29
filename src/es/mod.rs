//! The only place in Log Lens that speaks HTTP to Elasticsearch.
//!
//! Everything outside this module deals in the typed values defined here
//! ([`Endpoint`], [`ClusterInfo`], ...) and never sees `reqwest`. Requests are
//! built by hand against a fixed handful of REST endpoints, per ADR 0001.

use std::collections::BTreeSet;

use serde::Deserialize;
use serde_json::{Value, json};

/// Everything needed to make one authenticated request to a cluster.
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

fn client(endpoint: &Endpoint) -> Result<reqwest::Client, String> {
    let mut builder = reqwest::Client::builder();
    if endpoint.skip_tls_verify {
        builder = builder.danger_accept_invalid_certs(true);
    }
    builder.build().map_err(|e| e.to_string())
}

fn with_auth(request: reqwest::RequestBuilder, auth: &AuthValue) -> reqwest::RequestBuilder {
    match auth {
        AuthValue::None => request,
        AuthValue::Basic { username, password } => request.basic_auth(username, Some(password)),
        AuthValue::ApiKey { key } => {
            request.header(reqwest::header::AUTHORIZATION, format!("ApiKey {key}"))
        }
    }
}

fn base(url: &str) -> &str {
    url.trim_end_matches('/')
}

/// `GET /` — reports the cluster name and version, or the failure verbatim.
pub async fn ping(endpoint: Endpoint) -> Result<ClusterInfo, String> {
    let client = client(&endpoint)?;
    let response = with_auth(client.get(base(&endpoint.url)), &endpoint.auth)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let status = response.status();
    let body = response.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("HTTP {}: {}", status.as_u16(), body.trim()));
    }

    #[derive(Deserialize)]
    struct Root {
        cluster_name: String,
        version: Version,
    }
    #[derive(Deserialize)]
    struct Version {
        number: String,
    }

    let root: Root = serde_json::from_str(&body)
        .map_err(|e| format!("unexpected response from {}: {e}", endpoint.url))?;
    Ok(ClusterInfo {
        cluster_name: root.cluster_name,
        version: root.version.number,
    })
}

// --- Targets -------------------------------------------------------------

/// `_cat/indices` + `_data_stream` — the names a Target typeahead offers.
/// Best-effort: a failing endpoint contributes nothing rather than erroring.
pub async fn list_targets(endpoint: Endpoint) -> Result<Vec<String>, String> {
    let client = client(&endpoint)?;
    let mut names: BTreeSet<String> = BTreeSet::new();

    let url = format!("{}/_cat/indices?h=index&format=json", base(&endpoint.url));
    if let Ok(response) = with_auth(client.get(&url), &endpoint.auth).send().await {
        if response.status().is_success() {
            if let Ok(rows) = response.json::<Vec<Value>>().await {
                for row in rows {
                    if let Some(index) = row.get("index").and_then(Value::as_str) {
                        if !index.starts_with('.') {
                            names.insert(index.to_string());
                        }
                    }
                }
            }
        }
    }

    let url = format!("{}/_data_stream", base(&endpoint.url));
    if let Ok(response) = with_auth(client.get(&url), &endpoint.auth).send().await {
        if response.status().is_success() {
            #[derive(Deserialize)]
            struct Streams {
                data_streams: Vec<Named>,
            }
            #[derive(Deserialize)]
            struct Named {
                name: String,
            }
            if let Ok(streams) = response.json::<Streams>().await {
                for stream in streams.data_streams {
                    names.insert(stream.name);
                }
            }
        }
    }

    Ok(names.into_iter().collect())
}

// --- Field capabilities -------------------------------------------------

/// What `_field_caps` tells us about a Target's fields.
#[derive(Debug, Clone, Default)]
pub struct FieldCaps {
    /// Every field name, sorted. Any of these may be a Column.
    pub all: Vec<String>,
    /// The subset that can be sorted on (keyword / numeric / date / boolean /
    /// ip — never analysed text).
    pub sortable: Vec<String>,
}

/// Elasticsearch field types Log Lens will sort on.
fn is_sortable_type(ty: &str) -> bool {
    matches!(
        ty,
        "keyword"
            | "constant_keyword"
            | "wildcard"
            | "boolean"
            | "date"
            | "date_nanos"
            | "ip"
            | "version"
            | "long"
            | "integer"
            | "short"
            | "byte"
            | "double"
            | "float"
            | "half_float"
            | "scaled_float"
            | "unsigned_long"
    )
}

/// `GET {target}/_field_caps?fields=*` — the fields a Search form offers.
pub async fn field_caps(endpoint: Endpoint, target: String) -> Result<FieldCaps, String> {
    let client = client(&endpoint)?;
    let url = format!(
        "{}/{}/_field_caps?fields=*",
        base(&endpoint.url),
        target.trim_matches('/')
    );
    let response = with_auth(client.get(&url), &endpoint.auth)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = response.status();
    let text = response.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(extract_error(&text, status.as_u16()));
    }

    #[derive(Deserialize)]
    struct Response {
        fields: serde_json::Map<String, Value>,
    }

    let parsed: Response =
        serde_json::from_str(&text).map_err(|e| format!("unexpected _field_caps response: {e}"))?;

    let mut all = Vec::new();
    let mut sortable = Vec::new();
    for (name, types) in parsed.fields {
        if name.starts_with('_') {
            continue;
        }
        all.push(name.clone());
        let type_names: Vec<&str> = types
            .as_object()
            .map(|m| m.keys().map(String::as_str).collect())
            .unwrap_or_default();
        if !type_names.is_empty() && type_names.iter().all(|t| is_sortable_type(t)) {
            sortable.push(name);
        }
    }
    all.sort();
    sortable.sort();
    Ok(FieldCaps { all, sortable })
}

// --- Paged search -------------------------------------------------------

/// One Hit: its `_source` and the `sort` values that page past it.
#[derive(Debug, Clone)]
pub struct Hit {
    pub source: Value,
    /// The Hit's `sort` values — the `search_after` cursor for the next Page.
    pub sort: Vec<Value>,
}

/// One Page of Hits from a `_search` call.
#[derive(Debug, Clone)]
pub struct Page {
    pub hits: Vec<Hit>,
    /// A refreshed PIT id, when the cluster returns one.
    pub pit_id: Option<String>,
}

/// Everything a `_search` call needs besides the PIT id.
#[derive(Debug, Clone)]
pub struct SearchParams {
    /// Empty means "no `query_string` clause" (range-only).
    pub query_string: String,
    pub timestamp_field: String,
    /// Range bounds — Elasticsearch date-math (`now-15m`) or ISO timestamps.
    pub gte: String,
    pub lte: String,
    /// Sort keys as `(field, descending)`, highest priority first. `_shard_doc`
    /// is appended as a tiebreaker. Never empty.
    pub sort: Vec<(String, bool)>,
    pub size: usize,
    /// `None` for the first Page; the previous Hit's `sort` values otherwise.
    pub search_after: Option<Vec<Value>>,
}

/// `POST {target}/_pit?keep_alive=1m` — opens a Point-in-Time.
pub async fn open_pit(endpoint: Endpoint, target: String) -> Result<String, String> {
    let client = client(&endpoint)?;
    let url = format!(
        "{}/{}/_pit?keep_alive=1m",
        base(&endpoint.url),
        target.trim_matches('/')
    );
    let response = with_auth(client.post(&url), &endpoint.auth)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = response.status();
    let text = response.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(extract_error(&text, status.as_u16()));
    }
    #[derive(Deserialize)]
    struct Pit {
        id: String,
    }
    serde_json::from_str::<Pit>(&text)
        .map(|p| p.id)
        .map_err(|e| e.to_string())
}

/// `DELETE /_pit` — releases server-side PIT state. Best-effort.
pub async fn close_pit(endpoint: Endpoint, pit_id: String) -> Result<(), String> {
    let client = client(&endpoint)?;
    let url = format!("{}/_pit", base(&endpoint.url));
    with_auth(
        client.delete(&url).json(&json!({ "id": pit_id })),
        &endpoint.auth,
    )
    .send()
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// `POST /_search` against an open PIT, paging with `search_after` and a
/// `[<sort field>, _shard_doc]` total order.
pub async fn search(
    endpoint: Endpoint,
    pit_id: String,
    params: SearchParams,
) -> Result<Page, String> {
    // `[{ field: dir }, ..., { _shard_doc: <primary dir> }]` — a stable total
    // order so `search_after` can page without gaps or repeats.
    let primary_desc = params.sort.first().map(|(_, desc)| *desc).unwrap_or(true);
    let mut sort: Vec<Value> = params
        .sort
        .iter()
        .map(|(field, desc)| json!({ field.clone(): if *desc { "desc" } else { "asc" } }))
        .collect();
    sort.push(json!({ "_shard_doc": if primary_desc { "desc" } else { "asc" } }));

    let mut bool_query = serde_json::Map::new();
    bool_query.insert(
        "filter".to_string(),
        json!([{
            "range": { (params.timestamp_field.clone()): { "gte": params.gte, "lte": params.lte } }
        }]),
    );
    if !params.query_string.trim().is_empty() {
        bool_query.insert(
            "must".to_string(),
            json!([{ "query_string": { "query": params.query_string } }]),
        );
    }

    let mut body = json!({
        "size": params.size,
        "track_total_hits": false,
        "pit": { "id": pit_id, "keep_alive": "1m" },
        "sort": Value::Array(sort),
        "query": { "bool": Value::Object(bool_query) },
    });
    if let Some(after) = &params.search_after {
        body["search_after"] = json!(after);
    }

    let client = client(&endpoint)?;
    let url = format!("{}/_search", base(&endpoint.url));
    let response = with_auth(client.post(&url).json(&body), &endpoint.auth)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = response.status();
    let text = response.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(extract_error(&text, status.as_u16()));
    }

    #[derive(Deserialize)]
    struct Response {
        hits: HitsEnvelope,
        #[serde(default)]
        pit_id: Option<String>,
    }
    #[derive(Deserialize)]
    struct HitsEnvelope {
        hits: Vec<RawHit>,
    }
    #[derive(Deserialize)]
    struct RawHit {
        #[serde(rename = "_source", default)]
        source: Value,
        #[serde(default)]
        sort: Vec<Value>,
    }

    let parsed: Response =
        serde_json::from_str(&text).map_err(|e| format!("unexpected search response: {e}"))?;
    Ok(Page {
        hits: parsed
            .hits
            .hits
            .into_iter()
            .map(|h| Hit {
                source: h.source,
                sort: h.sort,
            })
            .collect(),
        pit_id: parsed.pit_id,
    })
}

/// Pulls the most specific message out of an Elasticsearch error body,
/// falling back to the raw text so parse errors reach the user verbatim.
fn extract_error(body: &str, status: u16) -> String {
    if let Ok(value) = serde_json::from_str::<Value>(body) {
        let error = &value["error"];
        if let Some(reason) = error["root_cause"][0]["reason"].as_str() {
            return reason.to_string();
        }
        if let Some(reason) = error["reason"].as_str() {
            return reason.to_string();
        }
        if let Some(reason) = error.as_str() {
            return reason.to_string();
        }
    }
    let trimmed = body.trim();
    if trimmed.is_empty() {
        format!("HTTP {status}")
    } else {
        format!("HTTP {status}: {trimmed}")
    }
}
