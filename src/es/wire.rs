//! The Elasticsearch wire format: the request bodies `es` sends and the
//! responses it reads back. Nothing here does I/O, so all of it is tested
//! directly.

use serde::Deserialize;
use serde_json::{Value, json};

use super::{ClusterInfo, Error, FieldCaps, Hit, Query};

/// The `bool` query shared by `_search` and `_count`: a timestamp `range`
/// filter, plus a `query_string` `must` clause when one is set.
fn range_bool_query(query: &Query) -> Value {
    let timestamp_field = query.timestamp_field.clone();
    let (gte, lte) = (&query.gte, &query.lte);

    let mut bool_query = serde_json::Map::new();
    bool_query.insert(
        "filter".to_string(),
        json!([{ "range": { timestamp_field: { "gte": gte, "lte": lte } } }]),
    );
    if !query.query_string.trim().is_empty() {
        let query_string = &query.query_string;
        bool_query.insert(
            "must".to_string(),
            json!([{ "query_string": { "query": query_string } }]),
        );
    }
    json!({ "bool": Value::Object(bool_query) })
}

pub(super) fn count_body(query: &Query) -> Value {
    json!({ "query": range_bool_query(query) })
}

pub(super) fn search_body(query: &Query, size: usize, after: Option<&[Value]>) -> Value {
    // `[{ field: dir }, ..., { _doc: <primary dir> }]` — a stable total order
    // so `search_after` can page without gaps or repeats.
    let keys = query.sort_keys();
    let primary_desc = keys.first().map(|(_, desc)| *desc).unwrap_or(true);
    let mut sort: Vec<Value> = keys
        .iter()
        .map(|(field, desc)| json!({ field.clone(): if *desc { "desc" } else { "asc" } }))
        .collect();
    sort.push(json!({ "_doc": if primary_desc { "desc" } else { "asc" } }));

    let mut body = json!({
        "size": size,
        "track_total_hits": false,
        "sort": Value::Array(sort),
        "query": range_bool_query(query),
    });
    if let Some(after) = after {
        body["search_after"] = json!(after);
    }
    body
}

pub(super) fn cluster_info(body: &str) -> Result<ClusterInfo, Error> {
    #[derive(Deserialize)]
    struct Root {
        cluster_name: String,
        version: Version,
    }
    #[derive(Deserialize)]
    struct Version {
        number: String,
    }

    let root: Root = serde_json::from_str(body)
        .map_err(|e| Error::Malformed(format!("unexpected response: {e}")))?;
    Ok(ClusterInfo {
        cluster_name: root.cluster_name,
        version: root.version.number,
    })
}

/// The index names in a `_cat/indices?format=json` body, dot-prefixed system
/// indices dropped. Best-effort: an unreadable body contributes nothing.
pub(super) fn cat_indices(body: &str) -> Vec<String> {
    let Ok(rows) = serde_json::from_str::<Vec<Value>>(body) else {
        return Vec::new();
    };
    rows.iter()
        .filter_map(|row| row.get("index").and_then(Value::as_str))
        .filter(|index| !index.starts_with('.'))
        .map(str::to_string)
        .collect()
}

/// The stream names in a `_data_stream` body. Best-effort, as [`cat_indices`].
pub(super) fn data_streams(body: &str) -> Vec<String> {
    #[derive(Deserialize)]
    struct Streams {
        data_streams: Vec<Named>,
    }
    #[derive(Deserialize)]
    struct Named {
        name: String,
    }

    serde_json::from_str::<Streams>(body)
        .map(|streams| streams.data_streams.into_iter().map(|s| s.name).collect())
        .unwrap_or_default()
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

pub(super) fn field_caps(body: &str) -> Result<FieldCaps, Error> {
    #[derive(Deserialize)]
    struct Response {
        fields: serde_json::Map<String, Value>,
    }

    let parsed: Response = serde_json::from_str(body)
        .map_err(|e| Error::Malformed(format!("unexpected _field_caps response: {e}")))?;

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

pub(super) fn count(body: &str) -> Result<u64, Error> {
    #[derive(Deserialize)]
    struct CountResponse {
        count: u64,
    }
    serde_json::from_str::<CountResponse>(body)
        .map(|c| c.count)
        .map_err(|e| Error::Malformed(format!("unexpected _count response: {e}")))
}

/// The Hits in a `_search` response, reading `_source` and `sort` off each
/// `hits.hits[]` entry.
pub(super) fn hits(body: &str) -> Result<Vec<Hit>, Error> {
    #[derive(Deserialize)]
    struct Response {
        hits: HitsEnvelope,
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

    let parsed: Response = serde_json::from_str(body)
        .map_err(|e| Error::Malformed(format!("unexpected search response: {e}")))?;
    Ok(parsed
        .hits
        .hits
        .into_iter()
        .map(|h| Hit {
            source: h.source,
            sort: h.sort,
        })
        .collect())
}

/// Classifies a non-2xx response, carrying the most specific message
/// Elasticsearch offered — or the raw body, so a response we can't parse still
/// reaches the user verbatim.
pub(super) fn error(status: u16, body: &str) -> Error {
    let parsed = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    let error = &parsed["error"];
    let message = reason(error).unwrap_or_else(|| {
        let trimmed = body.trim();
        if trimmed.is_empty() {
            format!("HTTP {status}")
        } else {
            format!("HTTP {status}: {trimmed}")
        }
    });

    let missing_target = status == 404
        || error["root_cause"][0]["type"].as_str() == Some("index_not_found_exception")
        || error["type"].as_str() == Some("index_not_found_exception");

    match status {
        401 | 403 => Error::Unauthorized(message),
        _ if missing_target => Error::NoSuchTarget(message),
        _ => Error::Rejected(message),
    }
}

/// The most specific `reason` in an Elasticsearch `error` object.
fn reason(error: &Value) -> Option<String> {
    error["root_cause"][0]["reason"]
        .as_str()
        .or_else(|| error["reason"].as_str())
        .or_else(|| error.as_str())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query() -> Query {
        Query {
            target: "logs-app".to_string(),
            query_string: String::new(),
            timestamp_field: "@timestamp".to_string(),
            gte: "now-15m".to_string(),
            lte: "now".to_string(),
            sort: Vec::new(),
        }
    }

    #[test]
    fn the_sort_ends_with_a_doc_tiebreaker_in_the_primary_direction() {
        let mut q = query();
        q.sort = vec![("level".to_string(), false)];
        let body = search_body(&q, 100, None);
        assert_eq!(body["sort"], json!([{ "level": "asc" }, { "_doc": "asc" }]));
    }

    #[test]
    fn an_unset_sort_falls_back_to_the_timestamp_field_descending() {
        let body = search_body(&query(), 100, None);
        assert_eq!(
            body["sort"],
            json!([{ "@timestamp": "desc" }, { "_doc": "desc" }])
        );
    }

    #[test]
    fn every_sort_key_is_kept_in_priority_order() {
        let mut q = query();
        q.sort = vec![
            ("level".to_string(), true),
            ("host".to_string(), false),
            ("@timestamp".to_string(), true),
        ];
        let body = search_body(&q, 100, None);
        assert_eq!(
            body["sort"],
            json!([
                { "level": "desc" },
                { "host": "asc" },
                { "@timestamp": "desc" },
                { "_doc": "desc" },
            ])
        );
    }

    #[test]
    fn a_blank_query_string_leaves_the_bool_query_with_only_the_range_filter() {
        let body = search_body(&query(), 100, None);
        let bool_query = &body["query"]["bool"];
        assert!(bool_query.get("must").is_none());
        assert_eq!(
            bool_query["filter"],
            json!([{ "range": { "@timestamp": { "gte": "now-15m", "lte": "now" } } }])
        );
    }

    #[test]
    fn a_query_string_of_only_whitespace_counts_as_blank() {
        let mut q = query();
        q.query_string = "   ".to_string();
        assert!(
            search_body(&q, 100, None)["query"]["bool"]
                .get("must")
                .is_none()
        );
    }

    #[test]
    fn a_query_string_becomes_a_must_clause_beside_the_range_filter() {
        let mut q = query();
        q.query_string = "level:ERROR".to_string();
        let body = search_body(&q, 100, None);
        assert_eq!(
            body["query"]["bool"]["must"],
            json!([{ "query_string": { "query": "level:ERROR" } }])
        );
    }

    #[test]
    fn the_range_filter_uses_the_configured_timestamp_field() {
        let mut q = query();
        q.timestamp_field = "event.ingested".to_string();
        let body = count_body(&q);
        assert_eq!(
            body["query"]["bool"]["filter"],
            json!([{ "range": { "event.ingested": { "gte": "now-15m", "lte": "now" } } }])
        );
    }

    #[test]
    fn search_after_is_absent_on_the_first_page_and_present_on_a_continuation() {
        let first = search_body(&query(), 100, None);
        assert!(first.get("search_after").is_none());

        let cursor = vec![json!(1_700_000_000_000_u64), json!(42)];
        let next = search_body(&query(), 100, Some(&cursor));
        assert_eq!(next["search_after"], json!([1_700_000_000_000_u64, 42]));
    }

    #[test]
    fn a_search_never_asks_the_cluster_to_track_the_total() {
        assert_eq!(
            search_body(&query(), 100, None)["track_total_hits"],
            json!(false)
        );
    }

    #[test]
    fn a_count_body_carries_the_query_without_sort_or_paging() {
        let body = count_body(&query());
        assert!(body.get("sort").is_none());
        assert!(body.get("size").is_none());
        assert!(body.get("search_after").is_none());
    }

    #[test]
    fn hits_carry_their_source_and_sort_values() {
        let body = json!({
            "hits": { "hits": [
                { "_source": { "message": "one" }, "sort": [1, 2] },
                { "_source": { "message": "two" }, "sort": [3, 4] },
            ]}
        })
        .to_string();
        let hits = hits(&body).expect("parse");
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].source["message"], json!("one"));
        assert_eq!(hits[1].sort, vec![json!(3), json!(4)]);
    }

    #[test]
    fn a_hit_with_neither_source_nor_sort_still_parses() {
        let body = json!({ "hits": { "hits": [{}] } }).to_string();
        let hits = hits(&body).expect("parse");
        assert_eq!(hits.len(), 1);
        assert!(hits[0].sort.is_empty());
    }

    #[test]
    fn the_checked_in_fixture_parses_into_hits() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/benches/fixtures/nginx-800.json"
        );
        let body = std::fs::read_to_string(path).expect("read fixture");
        let hits = hits(&body).expect("parse");
        assert_eq!(hits.len(), 800);
        assert!(hits.iter().all(|h| !h.sort.is_empty()));
    }

    #[test]
    fn a_body_that_is_not_a_search_response_is_malformed() {
        assert!(matches!(hits("not json"), Err(Error::Malformed(_))));
    }

    #[test]
    fn only_fields_whose_every_type_is_sortable_are_offered_for_sorting() {
        let body = json!({
            "fields": {
                "@timestamp": { "date": {} },
                "message": { "text": {} },
                "host": { "keyword": {}, "text": {} },
                "_id": { "_id": {} },
            }
        })
        .to_string();
        let caps = field_caps(&body).expect("parse");
        assert_eq!(caps.all, vec!["@timestamp", "host", "message"]);
        assert_eq!(caps.sortable, vec!["@timestamp"]);
    }

    #[test]
    fn the_most_specific_root_cause_reason_wins_over_the_outer_one() {
        let body = json!({
            "error": {
                "root_cause": [{ "type": "parsing_exception", "reason": "the real reason" }],
                "reason": "the outer reason",
            }
        })
        .to_string();
        assert_eq!(error(400, &body).to_string(), "the real reason");
    }

    #[test]
    fn an_error_with_no_root_cause_falls_back_to_the_outer_reason() {
        let body = json!({ "error": { "reason": "the outer reason" } }).to_string();
        assert_eq!(error(400, &body).to_string(), "the outer reason");
    }

    #[test]
    fn an_unparseable_error_body_reaches_the_user_verbatim() {
        assert_eq!(
            error(502, "  bad gateway  ").to_string(),
            "HTTP 502: bad gateway"
        );
        assert_eq!(error(502, "   ").to_string(), "HTTP 502");
    }

    #[test]
    fn rejected_credentials_are_told_apart_from_a_rejected_query() {
        assert!(matches!(error(401, "{}"), Error::Unauthorized(_)));
        assert!(matches!(error(403, "{}"), Error::Unauthorized(_)));
        assert!(matches!(error(400, "{}"), Error::Rejected(_)));
    }

    #[test]
    fn a_missing_index_is_told_apart_from_any_other_rejection() {
        let body = json!({
            "error": {
                "root_cause": [{
                    "type": "index_not_found_exception",
                    "reason": "no such index [nope]",
                }],
            }
        })
        .to_string();
        match error(404, &body) {
            Error::NoSuchTarget(message) => assert_eq!(message, "no such index [nope]"),
            other => panic!("expected NoSuchTarget, got {other:?}"),
        }
        // The exception type decides it, not just the status.
        assert!(matches!(error(400, &body), Error::NoSuchTarget(_)));
        // ...and a bare 404 counts even with nothing to read in the body.
        assert!(matches!(error(404, ""), Error::NoSuchTarget(_)));
    }

    #[test]
    fn dot_prefixed_system_indices_are_not_offered_as_targets() {
        let body = json!([
            { "index": "logs-app" },
            { "index": ".kibana_1" },
            { "index": "logs-web" },
        ])
        .to_string();
        assert_eq!(cat_indices(&body), vec!["logs-app", "logs-web"]);
    }

    #[test]
    fn an_unreadable_target_listing_contributes_nothing() {
        assert!(cat_indices("not json").is_empty());
        assert!(data_streams("not json").is_empty());
    }

    #[test]
    fn data_streams_are_read_by_name() {
        let body = json!({ "data_streams": [{ "name": "logs-ds" }] }).to_string();
        assert_eq!(data_streams(&body), vec!["logs-ds"]);
    }
}
