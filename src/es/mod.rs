//! The only place in Log Lens that speaks HTTP to Elasticsearch.
//!
//! Everything outside this module deals in the typed values defined here
//! ([`Endpoint`], [`ClusterInfo`], ...) and never sees `reqwest`. Requests are
//! built by hand against a fixed handful of REST endpoints, per ADR 0001.

use serde::Deserialize;

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

fn with_auth(
    request: reqwest::RequestBuilder,
    auth: &AuthValue,
) -> reqwest::RequestBuilder {
    match auth {
        AuthValue::None => request,
        AuthValue::Basic { username, password } => {
            request.basic_auth(username, Some(password))
        }
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
