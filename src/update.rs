//! The Update check: asking GitHub whether a Release newer than the running
//! build exists.
//!
//! This module only ever *notices* a new Release and hands it back. Downloading
//! it, verifying it against the Release's `SHA256SUMS`, and running its
//! installer are a separate concern and live elsewhere; nothing here touches the
//! filesystem or the running installation.
//!
//! The pieces that can be got wrong — reading the JSON, reading a version out of
//! a tag, deciding whether enough time has passed, and deciding whether a
//! failure is allowed to be seen — are all pure functions with tests. Only
//! [`check`] itself talks to the network.

use chrono::{DateTime, TimeDelta, Utc};
use serde::Deserialize;

/// The version this build reports for comparison purposes.
///
/// Deliberately *not* `crate::VERSION`: that one has the short commit hash
/// appended for display (`0.1.0 (a1b2c3d)`) and would never parse as a version
/// triple. What a Release's tag has to be compared against is the bare crate
/// version.
pub const RUNNING_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The Release GitHub considers current for this repository.
///
/// `/releases/latest` excludes pre-releases and drafts server-side, which is why
/// there is no client-side filtering anywhere below: a `v*-rc*` / `v*-beta*` tag
/// is published with `--prerelease` (see `.github/workflows/release.yml`) and is
/// therefore invisible to this endpoint for free. Filtering pre-releases here as
/// well would be a second, weaker copy of a rule GitHub already enforces — and
/// the copy that gets it wrong.
const LATEST_RELEASE_URL: &str = "https://api.github.com/repos/dennisdms/loglens/releases/latest";

/// The project's repository, shown in the About dialog.
pub const REPOSITORY_URL: &str = "https://github.com/dennisdms/loglens";

/// GitHub rejects API requests that arrive without a `User-Agent`, so this is
/// not decoration.
const USER_AGENT: &str = concat!("LogLens/", env!("CARGO_PKG_VERSION"));

/// How long a background Update check waits before asking again.
const CHECK_INTERVAL: TimeDelta = TimeDelta::hours(24);

/// A whole request has this long to complete. Without it a proxy that accepts
/// the connection and then says nothing leaves a manual check reporting
/// "Checking for updates…" for the rest of the session.
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// One published Release, reduced to what Log Lens needs from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// The version, with the tag's leading `v` stripped: `0.2.0`.
    pub version: String,
    /// GitHub's generated release notes, shown in the banner. Empty when the
    /// Release carries none.
    pub notes: String,
    /// The Release's page, where a copy that cannot update itself is sent.
    pub html_url: String,
    /// Every Artifact attached to the Release.
    ///
    /// Nothing reads these yet — choosing the Artifact for this Install flavour
    /// and verifying it against `SHA256SUMS` is the next step of the pipeline.
    /// They are parsed here because this is the one place that knows the shape
    /// of GitHub's response, and they are part of what this module promises to
    /// hand back.
    #[allow(
        dead_code,
        reason = "read by the Update-applying path, not yet written"
    )]
    pub assets: Vec<Asset>,
}

/// One downloadable file belonging to a Release.
#[allow(
    dead_code,
    reason = "read by the Update-applying path, not yet written"
)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    /// The Artifact's file name, which the naming convention makes a contract:
    /// `LogLens-<version>-<os>-x86_64` plus the flavour suffix.
    pub name: String,
    pub download_url: String,
}

/// Why an Update check ran. This is what the silent/loud failure split hangs
/// off, so that it is a decision the code states rather than an accident of
/// which call site happened to invoke the check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// A day had passed, so a check ran on startup. Nobody asked for it.
    Background,
    /// The user chose `Help > Check for updates…` and is waiting for an answer.
    Manual,
}

/// What the application should show once a check has finished.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// A newer Release exists: show the banner.
    Found(Release),
    /// The running version is the newest one. Only worth saying to someone who
    /// asked.
    UpToDate,
    /// The check failed and the user is owed the reason.
    Failed(String),
    /// Say nothing at all.
    Silent,
}

/// Whether a background Update check is due, given when the last one ran.
///
/// A check that has never run is due. So is one whose recorded time is in the
/// *future*: that means the clock moved backwards (a laptop resuming with a bad
/// RTC, a timezone-confused system clock being corrected), and treating it as
/// "recent" would silence the check for up to a day past a moment that never
/// happens.
pub fn is_due(last_check: Option<DateTime<Utc>>, now: DateTime<Utc>) -> bool {
    let Some(last_check) = last_check else {
        return true;
    };
    let elapsed = now.signed_duration_since(last_check);
    elapsed > CHECK_INTERVAL || elapsed < TimeDelta::zero()
}

/// Decides what a finished check is allowed to show.
///
/// The asymmetry is the point. These checks run on machines behind corporate
/// proxies, on planes, and on office IPs sharing GitHub's 60-requests-an-hour
/// unauthenticated budget, so a failure is a normal event rather than a
/// noteworthy one — and an error box nobody asked for, on startup, for a
/// non-event, is worse than no update check at all. A user who chose
/// `Check for updates…` is in the opposite position: silence there looks like a
/// broken menu item.
pub fn outcome(trigger: Trigger, result: Result<Option<Release>, String>) -> Outcome {
    match (trigger, result) {
        // A hit is worth showing however the check came to run.
        (_, Ok(Some(release))) => Outcome::Found(release),
        (Trigger::Manual, Ok(None)) => Outcome::UpToDate,
        (Trigger::Manual, Err(err)) => Outcome::Failed(err),
        (Trigger::Background, _) => Outcome::Silent,
    }
}

/// Asks GitHub for the latest Release and returns it only if it is newer than
/// the running build.
///
/// `Ok(None)` means the check succeeded and there is nothing to offer. Errors
/// are already phrased for a human, because a manual check shows them verbatim.
pub async fn check() -> Result<Option<Release>, String> {
    let body = fetch_latest().await?;
    let release = parse_latest(&body)?;
    newer_than(release, RUNNING_VERSION)
}

/// The one function here that touches the network.
async fn fetch_latest() -> Result<String, String> {
    let client = reqwest::Client::builder()
        .timeout(TIMEOUT)
        .build()
        .map_err(|e| e.to_string())?;

    let response = client
        .get(LATEST_RELEASE_URL)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let status = response.status();
    // A repository with no published Release answers 404. That is a plain
    // fact about the project, not a fault, and "HTTP 404" would send whoever
    // read it looking for a broken URL.
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err("no releases have been published yet".to_string());
    }
    let body = response.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("HTTP {}: {}", status.as_u16(), body.trim()));
    }
    Ok(body)
}

/// Reads one Release out of GitHub's `releases/latest` response.
fn parse_latest(body: &str) -> Result<Release, String> {
    #[derive(Deserialize)]
    struct RawRelease {
        tag_name: String,
        /// Null on a Release published with no notes at all.
        #[serde(default)]
        body: Option<String>,
        html_url: String,
        #[serde(default)]
        assets: Vec<RawAsset>,
    }
    #[derive(Deserialize)]
    struct RawAsset {
        name: String,
        browser_download_url: String,
    }

    let raw: RawRelease =
        serde_json::from_str(body).map_err(|e| format!("unexpected response from GitHub: {e}"))?;

    Ok(Release {
        // The tag carries a leading `v` (`v0.2.0`); the version does not.
        version: raw.tag_name.trim_start_matches('v').to_string(),
        notes: raw.body.unwrap_or_default(),
        html_url: raw.html_url,
        assets: raw
            .assets
            .into_iter()
            .map(|a| Asset {
                name: a.name,
                download_url: a.browser_download_url,
            })
            .collect(),
    })
}

/// Keeps `release` only when its version is strictly greater than `running`.
///
/// A version that cannot be read is an error rather than a shrug: telling a user
/// they are up to date because a tag was unreadable would be a lie, and the one
/// person who can act on it is whoever cut the malformed tag.
fn newer_than(release: Release, running: &str) -> Result<Option<Release>, String> {
    let latest = parse_version(&release.version).ok_or_else(|| {
        format!(
            "could not read a version from release tag \"{}\"",
            release.version
        )
    })?;
    let running = parse_version(running)
        .ok_or_else(|| format!("could not read this build's own version, \"{running}\""))?;
    Ok((latest > running).then_some(release))
}

/// Reads `MAJOR.MINOR.PATCH` — with or without the tag's leading `v` — into a
/// tuple that compares in the right order.
///
/// This is not a semantic-version parser and does not need to be. Every string
/// it sees is either this crate's own version or a tag from
/// `/releases/latest`, which never carries a pre-release, so the only cases
/// that remain are three numbers and rubbish. A `semver` dependency to
/// implement `<` over three integers would be all cost.
fn parse_version(version: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version.trim().trim_start_matches('v').split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    // Anything trailing — a fourth component, a `-rc.1` suffix that survived
    // somehow — means this is not a version triple, so refuse it rather than
    // silently comparing a prefix of it.
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A trimmed-down copy of a real `releases/latest` response: the four fields
    /// that are read, and one that is not, to prove unknown keys are ignored.
    const LATEST_JSON: &str = r###"{
        "tag_name": "v0.2.0",
        "name": "0.2.0",
        "draft": false,
        "prerelease": false,
        "html_url": "https://github.com/dennisdms/loglens/releases/tag/v0.2.0",
        "body": "## What's Changed\n* Faster paging",
        "assets": [
            {
                "name": "LogLens-0.2.0-linux-x86_64.tar.gz",
                "browser_download_url": "https://github.com/dennisdms/loglens/releases/download/v0.2.0/LogLens-0.2.0-linux-x86_64.tar.gz"
            },
            {
                "name": "SHA256SUMS",
                "browser_download_url": "https://github.com/dennisdms/loglens/releases/download/v0.2.0/SHA256SUMS"
            }
        ]
    }"###;

    fn release(version: &str) -> Release {
        Release {
            version: version.to_string(),
            notes: String::new(),
            html_url: String::new(),
            assets: Vec::new(),
        }
    }

    #[test]
    fn a_plain_version_parses_into_its_three_numbers() {
        assert_eq!(parse_version("0.2.13"), Some((0, 2, 13)));
    }

    #[test]
    fn a_tag_parses_the_same_as_the_version_it_carries() {
        assert_eq!(parse_version("v1.4.0"), parse_version("1.4.0"));
    }

    #[test]
    fn a_version_that_is_not_three_numbers_is_refused() {
        for garbage in [
            "",
            "v",
            "0.2",
            "0.2.0.1",
            "0.2.x",
            "latest",
            "0.2.0-rc.1",
            "-1.0.0",
        ] {
            assert_eq!(parse_version(garbage), None, "{garbage:?} should not parse");
        }
    }

    #[test]
    fn each_component_outranks_the_ones_after_it() {
        assert!(parse_version("1.0.0") > parse_version("0.99.99"));
        assert!(parse_version("0.3.0") > parse_version("0.2.99"));
        assert!(parse_version("0.2.10") > parse_version("0.2.9"));
    }

    #[test]
    fn a_newer_release_is_offered() {
        let found = newer_than(release("0.2.0"), "0.1.0").expect("both versions parse");
        assert_eq!(found.map(|r| r.version), Some("0.2.0".to_string()));
    }

    #[test]
    fn the_running_version_is_not_offered_to_itself() {
        assert_eq!(newer_than(release("0.1.0"), "0.1.0"), Ok(None));
    }

    #[test]
    fn an_older_release_is_not_offered() {
        assert_eq!(newer_than(release("0.1.0"), "0.2.0"), Ok(None));
    }

    #[test]
    fn a_tag_that_does_not_parse_is_an_error_rather_than_silence() {
        let err = newer_than(release("nightly"), "0.1.0").expect_err("an unreadable tag");
        assert!(err.contains("nightly"), "{err}");
    }

    #[test]
    fn a_release_response_yields_its_version_notes_url_and_assets() {
        let release = parse_latest(LATEST_JSON).expect("a well-formed response");
        assert_eq!(release.version, "0.2.0");
        assert!(release.notes.contains("Faster paging"), "{}", release.notes);
        assert_eq!(
            release.html_url,
            "https://github.com/dennisdms/loglens/releases/tag/v0.2.0"
        );
        assert_eq!(release.assets.len(), 2);
        assert_eq!(release.assets[0].name, "LogLens-0.2.0-linux-x86_64.tar.gz");
        assert!(
            release.assets[1].download_url.ends_with("/SHA256SUMS"),
            "{}",
            release.assets[1].download_url
        );
    }

    #[test]
    fn a_release_with_no_notes_parses_with_empty_notes() {
        let json = r#"{"tag_name":"v0.2.0","html_url":"https://example.invalid","body":null}"#;
        let release = parse_latest(json).expect("a release without notes");
        assert_eq!(release.notes, "");
        assert!(release.assets.is_empty());
    }

    #[test]
    fn a_tag_with_no_leading_v_parses_as_the_version_it_is() {
        // Log Lens tags as `v0.2.0` (see `.github/workflows/release.yml`), but
        // the `v` is a convention rather than a rule and the parse must not
        // depend on it.
        let json = r#"{"tag_name":"0.2.0","html_url":"https://example.invalid","body":""}"#;
        assert_eq!(parse_latest(json).expect("a bare tag").version, "0.2.0");
    }

    #[test]
    fn a_response_that_is_not_a_release_is_an_error() {
        // In order: a rate-limit body, a captive portal's login page, an empty
        // body, and JSON of the right shape but the wrong type.
        for body in [
            r#"{"message":"API rate limit exceeded","documentation_url":"https://docs.github.com"}"#,
            "<html><body>Sign in to continue</body></html>",
            "",
            r#"{"tag_name":42}"#,
        ] {
            let err = parse_latest(body).expect_err("should not parse as a release");
            assert!(err.starts_with("unexpected response from GitHub"), "{err}");
        }
    }

    #[test]
    fn a_check_that_has_never_run_is_due() {
        assert!(is_due(None, Utc::now()));
    }

    #[test]
    fn a_check_from_within_the_last_day_is_not_due() {
        let now = Utc::now();
        assert!(!is_due(Some(now - TimeDelta::hours(23)), now));
        assert!(!is_due(Some(now), now));
    }

    #[test]
    fn a_check_from_exactly_a_day_ago_is_not_yet_due() {
        // The interval is "more than 24 hours", so the boundary itself waits.
        // Pinned because this is where an off-by-one would turn a once-a-day
        // check into a once-a-launch one for anyone with a habit.
        let now = Utc::now();
        assert!(!is_due(Some(now - TimeDelta::hours(24)), now));
    }

    #[test]
    fn a_check_from_over_a_day_ago_is_due() {
        let now = Utc::now();
        assert!(is_due(Some(now - TimeDelta::hours(25)), now));
        assert!(is_due(Some(now - TimeDelta::days(400)), now));
    }

    #[test]
    fn a_check_recorded_in_the_future_is_due() {
        let now = Utc::now();
        assert!(is_due(Some(now + TimeDelta::days(7)), now));
    }

    #[test]
    fn a_hit_is_shown_however_the_check_was_triggered() {
        for trigger in [Trigger::Background, Trigger::Manual] {
            assert_eq!(
                outcome(trigger, Ok(Some(release("0.2.0")))),
                Outcome::Found(release("0.2.0")),
            );
        }
    }

    #[test]
    fn a_failed_background_check_says_nothing() {
        assert_eq!(
            outcome(Trigger::Background, Err("dns error".to_string())),
            Outcome::Silent,
        );
    }

    #[test]
    fn a_background_check_finding_nothing_says_nothing() {
        assert_eq!(outcome(Trigger::Background, Ok(None)), Outcome::Silent);
    }

    #[test]
    fn a_failed_manual_check_reports_the_reason() {
        assert_eq!(
            outcome(Trigger::Manual, Err("dns error".to_string())),
            Outcome::Failed("dns error".to_string()),
        );
    }

    #[test]
    fn a_manual_check_finding_nothing_still_answers() {
        assert_eq!(outcome(Trigger::Manual, Ok(None)), Outcome::UpToDate);
    }
}
