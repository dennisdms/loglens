//! Updates: noticing that a newer Release exists, and replacing this copy of
//! Log Lens with it.
//!
//! Three questions, in order, and each one can say no:
//!
//! 1. **Is there a newer Release?** [`check`] asks GitHub, and [`outcome`]
//!    decides whether the answer may be shown — a background check that failed
//!    says nothing at all.
//! 2. **May this copy replace itself?** [`flavour`] compares the directory the
//!    Install flavour marker records against the one this process is running
//!    from. Anything short of a match is [`Flavour::Portable`], which is told
//!    about the Release and otherwise left alone.
//! 3. **Is this really the Release's Artifact?** [`apply`] downloads it and the
//!    Release's `SHA256SUMS`, and nothing reaches the disk — let alone runs —
//!    until the two agree.
//!
//! The pieces that can be got wrong — reading the JSON, reading a version out of
//! a tag, deciding whether enough time has passed, deciding whether a failure is
//! allowed to be seen, reading a marker, picking an Artifact out of a Release,
//! and reading a hash out of `SHA256SUMS` — are all pure functions with tests.
//! [`check`] and [`apply`] are the only things here that talk to the network.

use std::path::{Path, PathBuf};
use std::process::Command;

use chrono::{DateTime, TimeDelta, Utc};
use serde::Deserialize;
use sha2::{Digest, Sha256};

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
    /// Every Artifact attached to the Release, including `SHA256SUMS`.
    ///
    /// [`apply`] picks this platform's Artifact out of these by name and
    /// verifies it against the `SHA256SUMS` beside it.
    pub assets: Vec<Asset>,
}

/// One downloadable file belonging to a Release.
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

/// The check's own network call. [`fetch`] is the Update's.
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

// --- Install flavour -------------------------------------------------------

/// The marker both installers write to record where they installed to.
const MARKER_NAME: &str = "install-manifest.json";

/// How this copy of Log Lens got onto the machine, and therefore whether it is
/// allowed to replace itself.
///
/// The distinction is not cosmetic. Running a Release's installer from a
/// Portable copy would install a *second* copy into the directory the
/// installer owns — `%LOCALAPPDATA%\Programs\Log Lens`, `~/.local/bin` —
/// while the user carried on running the old one from the USB stick or the
/// folder they unpacked it into. The Update would appear to succeed and
/// nothing about the running app would ever change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Flavour {
    /// Placed by the Windows installer or by the Linux archive's `install.sh`,
    /// into a directory it owns and can rewrite.
    ///
    /// Carries the path of the running binary, captured when the flavour is
    /// decided. It is captured that early on purpose: `install.sh` replaces
    /// the binary by renaming a new file over it, which unlinks the inode this
    /// process is still executing, and from that moment `current_exe()` reads
    /// back as `…/loglens (deleted)` — a path that cannot be re-executed.
    InstallerManaged { exe: PathBuf },
    /// Unpacked by hand and owned by nobody: the Windows portable zip, a copy
    /// on a USB stick, a `target/release` build.
    Portable,
}

impl Flavour {
    /// The binary to hand over to after an Update, or `None` for a copy that
    /// must not update itself at all.
    pub fn installed_exe(&self) -> Option<&Path> {
        match self {
            Self::InstallerManaged { exe } => Some(exe),
            Self::Portable => None,
        }
    }
}

/// Reads the Install flavour marker and decides what this copy is.
///
/// Call once, at startup, and keep the answer: it is read from the filesystem,
/// and the binary's own path stops being readable partway through a Linux
/// Update (see [`Flavour::InstallerManaged`]).
pub fn flavour() -> Flavour {
    // No readable path for this process means nothing to compare an install
    // directory against, so there is no way to be sure — which is Portable.
    let Ok(exe) = std::env::current_exe() else {
        return Flavour::Portable;
    };
    let Some(exe_dir) = exe.parent() else {
        return Flavour::Portable;
    };
    let Some(marker) = marker_path(exe_dir) else {
        return Flavour::Portable;
    };
    if is_installer_managed(&marker, exe_dir) {
        Flavour::InstallerManaged { exe }
    } else {
        Flavour::Portable
    }
}

/// Where each installer leaves the marker.
///
/// The asymmetry between the two is deliberate and has to be honoured on both
/// sides. Inno Setup writes it into `{app}`, beside `loglens.exe`, because
/// that directory is the install and goes away with the uninstall — which also
/// means a Windows portable copy cannot see an installed copy's marker at all.
/// `install.sh` has no such directory to use: `~/.local/bin` is the user's own
/// bin directory and holds other programs, so the marker goes where the rest
/// of Log Lens's own data does, `dirs::data_dir()/loglens/`, and it is the
/// directory comparison below that stops a portable copy claiming it.
fn marker_path(exe_dir: &Path) -> Option<PathBuf> {
    if cfg!(windows) {
        Some(exe_dir.join(MARKER_NAME))
    } else {
        dirs::data_dir().map(|dir| dir.join("loglens").join(MARKER_NAME))
    }
}

/// Whether the marker at `marker` says an installer put this copy in
/// `exe_dir`.
///
/// Comparing the recorded directory against the running one is the whole
/// safety property, and a marker that merely *exists* is not enough: a
/// portable copy run on a Linux machine that also has Log Lens installed finds
/// the installed copy's marker in `~/.local/share/loglens/` and would
/// otherwise conclude it may rewrite itself in place. Anything short of an
/// exact match — no file, an unreadable one, one that is not JSON, one
/// recording another flavour, one recording another directory — is Portable.
fn is_installer_managed(marker: &Path, exe_dir: &Path) -> bool {
    /// The two fields that decide this, out of the three
    /// `packaging/linux/install.sh` and the `WriteInstallManifest` procedure
    /// in `packaging/windows/loglens.iss` write. The third, `version`, is
    /// there for whoever opens the file: the running build knows its own
    /// version and has no reason to believe this one.
    #[derive(Deserialize)]
    struct Marker {
        flavour: String,
        install_dir: String,
    }

    let Ok(text) = std::fs::read_to_string(marker) else {
        return false;
    };
    let Ok(recorded) = serde_json::from_str::<Marker>(&text) else {
        return false;
    };
    recorded.flavour == "installer" && same_directory(Path::new(&recorded.install_dir), exe_dir)
}

/// Whether two paths name the same directory.
///
/// Canonicalised first, so that a symlinked home, a trailing separator, or
/// Windows's case-insensitivity cannot demote a real installation to Portable.
/// When either path cannot be resolved the textual comparison is the answer:
/// a recorded directory that no longer exists is not the one this process is
/// running from anyway.
fn same_directory(recorded: &Path, running: &Path) -> bool {
    match (recorded.canonicalize(), running.canonicalize()) {
        (Ok(recorded), Ok(running)) => recorded == running,
        _ => recorded == running,
    }
}

// --- Choosing and verifying the Artifact -----------------------------------

/// The Release Artifact carrying every Artifact's SHA-256, generated by
/// `sha256sum` in `.github/workflows/release.yml`.
const CHECKSUMS_NAME: &str = "SHA256SUMS";

/// The Artifact this build installs from, for `version`.
///
/// Both names are built on every platform rather than behind `#[cfg]`: they
/// are a compatibility contract (`docs/plans/d1-distribution-pipeline.md`,
/// 4.4) that a rename breaks for every already-installed copy, so both halves
/// of it are covered by the tests wherever those run.
fn artifact_name(version: &str) -> String {
    if cfg!(windows) {
        windows_artifact(version)
    } else {
        linux_artifact(version)
    }
}

fn windows_artifact(version: &str) -> String {
    format!("LogLens-{version}-windows-x86_64-setup.exe")
}

fn linux_artifact(version: &str) -> String {
    format!("LogLens-{version}-linux-x86_64.tar.gz")
}

/// The directory a Linux archive unpacks into. One top-level directory, no
/// tarbombs — the release workflow builds it with `tar -C dist`.
fn linux_archive_dir(version: &str) -> String {
    format!("LogLens-{version}-linux-x86_64")
}

/// The Release's Artifact of that name.
///
/// A Release that is missing one is a broken Release rather than a broken
/// build, which is what the message says. The workflow publishes a draft and
/// flips it public only once every Artifact has uploaded precisely so that
/// this cannot normally happen.
fn find_asset<'a>(assets: &'a [Asset], name: &str) -> Result<&'a Asset, String> {
    assets
        .iter()
        .find(|asset| asset.name == name)
        .ok_or_else(|| format!("this release has no {name}"))
}

/// The SHA-256 `SHA256SUMS` records for `name`, lowercased.
fn expected_hash(sums: &str, name: &str) -> Result<String, String> {
    sums.lines()
        .filter_map(checksum_line)
        .find(|(_, file)| *file == name)
        .map(|(hash, _)| hash.to_ascii_lowercase())
        .ok_or_else(|| format!("{CHECKSUMS_NAME} does not list {name}"))
}

/// One line of `sha256sum` output: 64 hex digits, a two-character separator,
/// and a bare file name.
///
/// The second separator character is the mode flag — a space for text mode,
/// `*` for binary. The workflow generates the file from inside `dist/`, so no
/// directory prefix ever appears in the names and none is stripped here; a
/// name that arrived with one would simply not match the Artifact being looked
/// for, which is the safe direction to be wrong in.
///
/// Anything that is not that shape is `None` and skipped rather than fatal:
/// a blank line, a `#` comment, or a future extra header must not stop a
/// well-formed line further down being found.
fn checksum_line(line: &str) -> Option<(&str, &str)> {
    let (hash, rest) = line.trim_end().split_once(' ')?;
    if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let name = rest.strip_prefix([' ', '*'])?;
    (!name.is_empty()).then_some((hash, name))
}

/// Checks a downloaded Artifact against the hash `SHA256SUMS` recorded for it.
///
/// With nothing code-signed this is the only integrity check there is. It does
/// not defend against someone who controls the Release — they would control
/// `SHA256SUMS` too — but it catches the failures that actually happen: a
/// truncated download, a corrupted one, a proxy that served something else.
///
/// A mismatch is a **failed download**. It is never retried silently and the
/// bytes are never run: the one thing worse than an Update that fails is an
/// Update that executes something nobody can account for.
fn verify(bytes: &[u8], expected: &str, name: &str) -> Result<(), String> {
    let actual = sha256_hex(bytes);
    if actual.eq_ignore_ascii_case(expected) {
        return Ok(());
    }
    Err(format!(
        "{name} does not match the checksum in {CHECKSUMS_NAME} \
         (expected {expected}, got {actual}); the download was corrupted or \
         altered in transit"
    ))
}

fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut hex, byte| {
            // Writing into a String cannot fail.
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

// --- The temp directory ----------------------------------------------------

/// Names every directory an Update downloads into, so that one left behind can
/// be recognised and swept up later.
const TEMP_PREFIX: &str = "loglens-update-";

/// A directory under the system temp directory that removes itself, and
/// everything in it, when it is dropped.
///
/// Every failure path in [`apply`] drops one of these on the way out, which is
/// how a download that turned out to be corrupt, or one whose installer would
/// not start, leaves nothing behind.
struct TempDir {
    path: PathBuf,
    keep: std::cell::Cell<bool>,
}

impl TempDir {
    fn new() -> Result<Self, String> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        let path =
            std::env::temp_dir().join(format!("{TEMP_PREFIX}{}-{nanos:x}", std::process::id()));
        // `create_dir`, not `create_dir_all`: on a shared `/tmp` the atomic
        // "make it, and fail if anything is already there" is what keeps this
        // from adopting a directory — or a symlink — somebody else planted.
        std::fs::create_dir(&path)
            .map_err(|e| format!("could not create {}: {e}", path.display()))?;
        Ok(Self {
            path,
            keep: std::cell::Cell::new(false),
        })
    }

    fn path(&self) -> &Path {
        &self.path
    }

    /// Leaves the directory on disk. For the one case that needs it: the
    /// Windows installer is running *from* this directory when the app is
    /// closed, so deleting it would be deleting the program mid-install.
    /// [`clean_stale_downloads`] collects it at the next start instead.
    fn keep(&self) {
        self.keep.set(true);
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        if !self.keep.get() {
            // Best effort. A directory that cannot be removed is swept at the
            // next start, and failing to tidy up is not worth a message.
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

/// Removes download directories a previous run could not.
///
/// There is exactly one way to leave one behind: the Windows path spawns the
/// installer out of its temp directory and is then closed by the Restart
/// Manager, with no moment left in which to delete the file being executed.
/// Call once at startup, before any Update can create one of its own.
///
/// Best effort throughout. A second Log Lens updating at this exact moment
/// would have its download swept from under it — the window is seconds long,
/// on Windows the file is locked while it runs so the removal fails harmlessly
/// anyway, and the alternative is an installer left in `%TEMP%` forever.
pub fn clean_stale_downloads() {
    clean_stale_downloads_in(&std::env::temp_dir());
}

fn clean_stale_downloads_in(root: &Path) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().starts_with(TEMP_PREFIX) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

// --- Applying --------------------------------------------------------------

/// A whole Artifact has this long to download. Deliberately far longer than
/// the [`TIMEOUT`] on the check: this is tens of megabytes over whatever
/// connection the user has, and a hotel wifi that takes four minutes has still
/// succeeded.
const DOWNLOAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

/// What is left for the running process to do once an Update has been carried
/// as far as a background task can carry it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Applied {
    /// The new version is installed and this process must hand over to the
    /// binary at this path — the Linux path, where `install.sh` has already
    /// replaced the binary and nothing else is going to restart the app.
    HandOver(PathBuf),
    /// The installer is running and will close and relaunch the app itself —
    /// the Windows path, where a running `.exe` cannot be overwritten and the
    /// Restart Manager owns the shutdown.
    InstallerRunning,
}

/// Downloads this platform's Artifact for `release`, verifies it against the
/// Release's `SHA256SUMS`, and runs it.
///
/// `exe` is the running binary's path, from
/// [`Flavour::InstallerManaged`] — only a copy that has one may be here at
/// all.
///
/// The Update runs the Release's own installer rather than swapping the binary
/// directly. A raw swap updates the executable and nothing else, so the
/// shortcut, the uninstall entry, the launcher entry and the icon stop
/// matching the installed version the first time any of them changes; reusing
/// the installer keeps that metadata correct by construction, and is less
/// code.
///
/// Nothing is written to disk before the checksum has been checked, and
/// nothing is executed that was not written by this function.
pub async fn apply(release: Release, exe: PathBuf) -> Result<Applied, String> {
    let name = artifact_name(&release.version);
    let artifact = find_asset(&release.assets, &name)?;
    let checksums = find_asset(&release.assets, CHECKSUMS_NAME)?;

    let sums = fetch(&checksums.download_url)
        .await
        .map_err(|e| format!("could not download {CHECKSUMS_NAME}: {e}"))?;
    let sums = String::from_utf8(sums).map_err(|_| format!("{CHECKSUMS_NAME} is not text"))?;
    let expected = expected_hash(&sums, &name)?;

    let bytes = fetch(&artifact.download_url)
        .await
        .map_err(|e| format!("could not download {name}: {e}"))?;
    verify(&bytes, &expected, &name)?;

    let dir = TempDir::new()?;
    let path = dir.path().join(&name);
    std::fs::write(&path, &bytes)
        .map_err(|e| format!("could not write {}: {e}", path.display()))?;

    install(&dir, &path, &release.version, &exe)
}

/// Downloads one Artifact whole.
///
/// `browser_download_url` redirects to GitHub's object storage, which
/// `reqwest` follows for us. The response is held in memory rather than
/// streamed to the file: it has to be hashed before it may be written
/// anywhere, and holding an Artifact-sized `Vec` briefly is a straight trade
/// for never having unverified bytes on disk.
async fn fetch(url: &str) -> Result<Vec<u8>, String> {
    let client = reqwest::Client::builder()
        .timeout(DOWNLOAD_TIMEOUT)
        .build()
        .map_err(|e| e.to_string())?;

    let response = client
        .get(url)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let status = response.status();
    if !status.is_success() {
        return Err(format!("HTTP {}", status.as_u16()));
    }

    response
        .bytes()
        .await
        .map(|body| body.to_vec())
        .map_err(|e| e.to_string())
}

/// Runs the verified Artifact.
///
/// Dispatched with `cfg!` rather than `#[cfg]` so that both platforms' steps
/// are compiled — and type-checked — everywhere. Only `std::process::Command`
/// is used, which is portable; the one genuinely platform-specific call is the
/// hand-over in [`hand_over`].
fn install(dir: &TempDir, artifact: &Path, version: &str, exe: &Path) -> Result<Applied, String> {
    if cfg!(windows) {
        install_windows(dir, artifact)
    } else {
        install_linux(dir, artifact, version, exe)
    }
}

/// Windows: start the setup Artifact and let it take over.
///
/// `/SILENT` runs the install with no wizard (`/VERYSILENT` would also hide
/// the progress window, and with it any error the user could act on).
/// `/NORESTART` because a per-user install into `%LOCALAPPDATA%` never needs
/// the machine rebooted, and an installer that reboots a developer's desktop
/// on its own would be unforgivable.
///
/// This process deliberately keeps running afterwards. Windows will not let a
/// running `.exe` be overwritten, so `CloseApplications=yes` in
/// `packaging/windows/loglens.iss` has Setup ask the Restart Manager to close
/// Log Lens, and `RestartApplications=yes` has it start Log Lens again once
/// the new files are in place. The Restart Manager can only close, and can
/// only restart, a process that is *there* when Setup enumerates: exiting here
/// would leave Setup with nothing to close and, more to the point, nothing to
/// bring back, and the user's app would simply vanish. See
/// `register_for_restart` in `main.rs` for the other half of that handshake.
fn install_windows(dir: &TempDir, setup: &Path) -> Result<Applied, String> {
    Command::new(setup)
        .args(["/SILENT", "/NORESTART"])
        .spawn()
        .map_err(|e| format!("could not start {}: {e}", setup.display()))?;

    // The installer is now executing out of this directory, so it has to
    // outlive both this function and this process.
    dir.keep();
    Ok(Applied::InstallerRunning)
}

/// Linux: unpack the archive and run its own `install.sh --quiet`.
///
/// `tar` rather than a crate: unpacking a `.tar.gz` in Rust means two more
/// dependencies to do what every machine that could have unpacked the Artifact
/// by hand already has. The archive's modes matter here — the release workflow
/// seals `install.sh` at 755 inside it — so the script is executed directly and
/// its own shebang chooses its interpreter, exactly as a user running
/// `./install.sh` would.
///
/// `install.sh` writes only under `$HOME` and installs the binary by renaming a
/// new file over the old one, which is what lets it replace a binary this very
/// process is executing.
fn install_linux(
    dir: &TempDir,
    archive: &Path,
    version: &str,
    exe: &Path,
) -> Result<Applied, String> {
    let unpack = Command::new("tar")
        .arg("xzf")
        .arg(archive)
        .arg("-C")
        .arg(dir.path())
        .output()
        .map_err(|e| format!("could not run tar: {e}"))?;
    if !unpack.status.success() {
        return Err(format!(
            "could not unpack the download: {}",
            last_line(&unpack.stderr)
        ));
    }

    let unpacked = dir.path().join(linux_archive_dir(version));
    let script = unpacked.join("install.sh");
    if !script.is_file() {
        return Err(format!(
            "the download does not look like a Log Lens archive: no install.sh in {}",
            linux_archive_dir(version)
        ));
    }

    let install = Command::new(&script)
        .arg("--quiet")
        .current_dir(&unpacked)
        .output()
        .map_err(|e| format!("could not run install.sh: {e}"))?;
    if !install.status.success() {
        return Err(format!("install.sh failed: {}", last_line(&install.stderr)));
    }

    Ok(Applied::HandOver(exe.to_path_buf()))
}

/// The last line a failed child process said, for a message a user can act on.
/// `install.sh` reports its own failures as one line on stderr.
fn last_line(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    text.lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("no output")
        .trim()
        .to_string()
}

/// Replaces this process with the freshly installed binary. Returns only when
/// the hand-over failed, and returns the reason.
///
/// `exec` rather than spawn-and-exit: the new build takes over this process
/// slot, so there is never a moment with two Log Lens windows open, and never
/// an orphan if the parent dies between the two steps. The window goes away
/// with the process image because the display connection's socket is
/// close-on-exec.
#[cfg(unix)]
pub fn restart(exe: &Path) -> String {
    use std::os::unix::process::CommandExt as _;

    // `exec` never returns on success.
    let failure = Command::new(exe).exec();
    format!(
        "could not restart Log Lens from {}: {failure}",
        exe.display()
    )
}

/// Unreachable in practice: only [`install_linux`] produces an
/// [`Applied::HandOver`], and on Windows the Restart Manager is what brings
/// the app back. It exists so the caller needs no `#[cfg]` of its own.
#[cfg(not(unix))]
pub fn restart(exe: &Path) -> String {
    format!("could not restart Log Lens from {}", exe.display())
}

/// Opens `url` in whatever the desktop uses for the web.
///
/// The two places a copy is sent somewhere it can help itself — a Portable
/// copy, which is told about a Release it must not install, and any copy whose
/// Update failed — both need this, and neither is worth a dependency. Errors
/// are returned rather than swallowed so the caller can fall back to showing
/// the address.
pub fn open_in_browser(url: &str) -> Result<(), String> {
    let mut command = if cfg!(windows) {
        // `start` is a cmd builtin, so cmd has to run it. The empty argument
        // is `start`'s window title, which it would otherwise take the URL to
        // be.
        let mut command = Command::new("cmd");
        command.args(["/C", "start", "", url]);
        command
    } else {
        let mut command = Command::new("xdg-open");
        command.arg(url);
        command
    };

    // The console window `cmd` would otherwise flash up in front of the app.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;

        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    // Not waited on: the browser outlives the call, and on a first launch
    // `xdg-open` can sit there for seconds.
    command
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("could not open {url}: {e}"))
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

    // --- Install flavour ---------------------------------------------------

    /// A private directory under the system temp directory, unique per call.
    /// Deliberately not prefixed `loglens-update-`, which
    /// [`clean_stale_downloads`] would sweep.
    fn temp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        let dir = std::env::temp_dir().join(format!("loglens-test-{tag}-{nanos:x}"));
        std::fs::create_dir_all(&dir).expect("a writable temp directory");
        dir
    }

    /// Writes a marker of the shape `packaging/linux/install.sh` and the
    /// `WriteInstallManifest` procedure in `packaging/windows/loglens.iss`
    /// both write.
    fn write_marker(marker: &Path, install_dir: &Path) {
        let escaped = install_dir.display().to_string().replace('\\', "\\\\");
        std::fs::write(
            marker,
            format!(
                "{{\n  \"flavour\": \"installer\",\n  \"install_dir\": \"{escaped}\",\n  \"version\": \"0.1.0\"\n}}\n"
            ),
        )
        .expect("a writable temp directory");
    }

    #[test]
    fn a_marker_recording_the_running_directory_is_installer_managed() {
        let dir = temp_dir("flavour-match");
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).expect("a writable temp directory");
        let marker = dir.join(MARKER_NAME);
        write_marker(&marker, &bin);

        assert!(is_installer_managed(&marker, &bin));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_marker_recording_another_directory_is_portable() {
        // The failure this check exists for: a portable copy run on a machine
        // that also has Log Lens installed finds the installed copy's marker
        // and must not conclude it may rewrite itself in place.
        let dir = temp_dir("flavour-elsewhere");
        let installed = dir.join("installed");
        let portable = dir.join("usb-stick");
        std::fs::create_dir_all(&installed).expect("a writable temp directory");
        std::fs::create_dir_all(&portable).expect("a writable temp directory");
        let marker = dir.join(MARKER_NAME);
        write_marker(&marker, &installed);

        assert!(is_installer_managed(&marker, &installed));
        assert!(!is_installer_managed(&marker, &portable));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_missing_marker_is_portable() {
        let dir = temp_dir("flavour-missing");
        assert!(!is_installer_managed(&dir.join(MARKER_NAME), &dir));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_unreadable_marker_is_portable() {
        // A directory where the file should be: the read fails rather than
        // returning something to parse. Stands in for every version of the
        // failure — a permission bit, a dangling symlink, a half-written file.
        let dir = temp_dir("flavour-unreadable");
        let marker = dir.join(MARKER_NAME);
        std::fs::create_dir_all(&marker).expect("a writable temp directory");

        assert!(!is_installer_managed(&marker, &dir));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_malformed_marker_is_portable() {
        let dir = temp_dir("flavour-malformed");
        let marker = dir.join(MARKER_NAME);
        let escaped = dir.display().to_string().replace('\\', "\\\\");

        for body in [
            // Empty, truncated mid-write, not JSON at all, JSON of the wrong
            // shape, and a marker recording some other flavour.
            String::new(),
            "{\"flavour\": \"insta".to_string(),
            "install_dir=/opt/loglens\n".to_string(),
            "{\"flavour\": \"installer\"}".to_string(),
            format!("{{\"flavour\": \"portable\", \"install_dir\": \"{escaped}\"}}"),
        ] {
            std::fs::write(&marker, &body).expect("a writable temp directory");
            assert!(
                !is_installer_managed(&marker, &dir),
                "{body:?} should not be installer-managed"
            );
        }

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_windows_marker_reads_back_the_path_inno_escaped() {
        // Every backslash in `install_dir` is doubled by `WriteInstallManifest`
        // in packaging/windows/loglens.iss, because `C:\Users` inside a JSON
        // string is an invalid escape sequence that serde_json rejects outright
        // — which would silently demote a real Windows installation to
        // Portable. This is that file, byte for byte, and the path it must come
        // back as. Neither path exists on the machine running this test, so the
        // comparison is the textual one on both platforms.
        let dir = temp_dir("flavour-windows");
        let marker = dir.join(MARKER_NAME);
        std::fs::write(
            &marker,
            "{\n  \"flavour\": \"installer\",\n  \"install_dir\": \"C:\\\\Users\\\\you\\\\AppData\\\\Local\\\\Programs\\\\Log Lens\",\n  \"version\": \"0.1.0\"\n}\n",
        )
        .expect("a writable temp directory");

        assert!(is_installer_managed(
            &marker,
            Path::new(r"C:\Users\you\AppData\Local\Programs\Log Lens"),
        ));
        assert!(!is_installer_managed(
            &marker,
            Path::new(r"D:\PortableApps\Log Lens"),
        ));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_same_directory_reached_by_a_different_path_still_matches() {
        // `~/.local/bin` reached through a symlinked home, a trailing
        // separator, a `..` — the recorded and running paths can be spelled
        // differently and still be one directory. Demoting a real
        // installation to Portable over spelling would silently disable
        // self-update.
        let dir = temp_dir("flavour-spelling");
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).expect("a writable temp directory");
        let marker = dir.join(MARKER_NAME);
        write_marker(&marker, &bin);

        assert!(is_installer_managed(
            &marker,
            &dir.join("bin").join("..").join("bin")
        ));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_marker_lives_beside_the_binary_on_windows_and_in_the_data_directory_elsewhere() {
        // The two installers put it in different places, and both are load
        // bearing: Inno owns `{app}` and takes the marker away with the
        // uninstall, while `install.sh` has only `~/.local/bin`, which is the
        // user's own bin directory and not Log Lens's to write markers into.
        let exe_dir = Path::new("/opt/loglens");
        let path = marker_path(exe_dir).expect("this platform has a data directory");
        assert!(path.ends_with(MARKER_NAME), "{}", path.display());
        if cfg!(windows) {
            assert_eq!(path, exe_dir.join(MARKER_NAME));
        } else {
            assert_eq!(
                path,
                dirs::data_dir()
                    .expect("a data directory")
                    .join("loglens")
                    .join(MARKER_NAME)
            );
        }
    }

    #[test]
    fn a_build_running_out_of_target_is_portable() {
        // `flavour()` itself, over this machine's real `current_exe` and real
        // data directory. A test binary runs from `target/debug/deps`, which no
        // installer has ever written a marker for, so the honest answer is
        // Portable — and a developer's build offering to install over their
        // installed copy is exactly the confusion the directory comparison
        // exists to prevent.
        assert_eq!(flavour(), Flavour::Portable);
    }

    #[test]
    fn a_portable_flavour_offers_no_binary_to_hand_over_to() {
        assert_eq!(Flavour::Portable.installed_exe(), None);
        assert_eq!(
            Flavour::InstallerManaged {
                exe: PathBuf::from("/home/you/.local/bin/loglens"),
            }
            .installed_exe(),
            Some(Path::new("/home/you/.local/bin/loglens")),
        );
    }

    // --- Choosing the Artifact ---------------------------------------------

    #[test]
    fn the_artifact_names_are_the_ones_the_release_workflow_uploads() {
        // A compatibility contract, not a formatting choice: renaming one
        // breaks self-update for every already-installed copy. Pinned here in
        // full so that a rename has to be a deliberate edit to this test.
        assert_eq!(
            windows_artifact("0.2.0"),
            "LogLens-0.2.0-windows-x86_64-setup.exe"
        );
        assert_eq!(linux_artifact("0.2.0"), "LogLens-0.2.0-linux-x86_64.tar.gz");
        assert_eq!(linux_archive_dir("0.2.0"), "LogLens-0.2.0-linux-x86_64");
        assert_eq!(CHECKSUMS_NAME, "SHA256SUMS");
    }

    #[test]
    fn this_build_asks_for_its_own_platforms_artifact() {
        let name = artifact_name("0.2.0");
        if cfg!(windows) {
            assert_eq!(name, windows_artifact("0.2.0"));
        } else {
            assert_eq!(name, linux_artifact("0.2.0"));
        }
    }

    #[test]
    fn an_artifact_is_found_by_name_among_the_others() {
        let release = parse_latest(LATEST_JSON).expect("a well-formed response");
        let found = find_asset(&release.assets, "LogLens-0.2.0-linux-x86_64.tar.gz")
            .expect("the linux archive");
        assert!(
            found.download_url.ends_with(".tar.gz"),
            "{}",
            found.download_url
        );
        assert_eq!(
            find_asset(&release.assets, CHECKSUMS_NAME).map(|a| a.name.as_str()),
            Ok(CHECKSUMS_NAME),
        );
    }

    #[test]
    fn an_artifact_this_release_does_not_carry_is_an_error() {
        let release = parse_latest(LATEST_JSON).expect("a well-formed response");
        let err = find_asset(&release.assets, &windows_artifact("0.2.0"))
            .expect_err("no windows setup in this response");
        assert!(err.contains("windows-x86_64-setup.exe"), "{err}");
    }

    // --- Checksums ---------------------------------------------------------

    /// Real `sha256sum` output over the three Artifacts, as
    /// `.github/workflows/release.yml` generates it: bare names, no directory
    /// prefix, two spaces.
    const SUMS: &str = "\
9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08  LogLens-0.2.0-linux-x86_64.tar.gz
60303ae22b998861bce3b28f33eec1be758a213c86c93c076dbe9f558c11c752  LogLens-0.2.0-windows-x86_64-setup.exe
fcde2b2edba56bf408601fb721fe9b5c338d10ee429ea04fae5511b68fbf8fb9  LogLens-0.2.0-windows-x86_64-portable.zip
";

    #[test]
    fn a_hash_is_read_out_of_sha256sums_by_artifact_name() {
        assert_eq!(
            expected_hash(SUMS, "LogLens-0.2.0-windows-x86_64-setup.exe"),
            Ok("60303ae22b998861bce3b28f33eec1be758a213c86c93c076dbe9f558c11c752".to_string()),
        );
    }

    #[test]
    fn an_artifact_sha256sums_does_not_list_is_an_error() {
        let err = expected_hash(SUMS, "LogLens-0.3.0-linux-x86_64.tar.gz")
            .expect_err("a version this file does not cover");
        assert!(err.contains("SHA256SUMS does not list"), "{err}");
    }

    #[test]
    fn malformed_lines_are_skipped_rather_than_fatal() {
        // A comment, a blank line, a truncated hash, a hash with no name, a
        // name with no hash, and something that is not hex. None of them may
        // stop the well-formed line at the end being found.
        let sums = format!(
            "# SHA256SUMS\n\
             \n\
             deadbeef  LogLens-0.2.0-linux-x86_64.tar.gz\n\
             9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08\n\
             LogLens-0.2.0-linux-x86_64.tar.gz\n\
             zzzz2b2edba56bf408601fb721fe9b5c338d10ee429ea04fae5511b68fbf8fb9  LogLens-0.2.0-linux-x86_64.tar.gz\n\
             {SUMS}"
        );
        assert_eq!(
            expected_hash(&sums, "LogLens-0.2.0-linux-x86_64.tar.gz"),
            Ok("9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08".to_string()),
        );
    }

    #[test]
    fn binary_mode_carriage_returns_and_upper_case_are_all_read() {
        // `sha256sum -b` marks the name with `*`; a file that has been through
        // a Windows editor carries CRLF; a hash typed by hand may be upper
        // case. None of the three is worth failing an Update over.
        let sums = "9F86D081884C7D659A2FEAA0C55AD015A3BF4F1B2B0B822CD15D6C15B0F00A08 *LogLens-0.2.0-linux-x86_64.tar.gz\r\n";
        assert_eq!(
            expected_hash(sums, "LogLens-0.2.0-linux-x86_64.tar.gz"),
            Ok("9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08".to_string()),
        );
    }

    #[test]
    fn the_hash_is_the_one_sha256sum_prints() {
        // The standard vector, and the same bytes `printf 'abc' | sha256sum`
        // reports.
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        );
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        );
    }

    #[test]
    fn bytes_that_match_their_recorded_hash_verify() {
        let expected = expected_hash(SUMS, "LogLens-0.2.0-linux-x86_64.tar.gz")
            .expect("the linux archive is listed");
        // "test" hashes to the value the fixture records for the archive.
        assert_eq!(
            verify(b"test", &expected, "LogLens-0.2.0-linux-x86_64.tar.gz"),
            Ok(())
        );
    }

    #[test]
    fn bytes_that_do_not_match_are_a_failed_download() {
        let expected = expected_hash(SUMS, "LogLens-0.2.0-linux-x86_64.tar.gz")
            .expect("the linux archive is listed");
        let err = verify(
            b"a corrupted download",
            &expected,
            "LogLens-0.2.0-linux-x86_64.tar.gz",
        )
        .expect_err("the bytes are not the ones SHA256SUMS covers");

        // The message names the Artifact, both hashes, and what it means. It
        // is reported as a failed download and nothing is run: with nothing
        // code-signed this comparison is the only integrity check there is.
        assert!(err.contains("LogLens-0.2.0-linux-x86_64.tar.gz"), "{err}");
        assert!(err.contains(&expected), "{err}");
        assert!(err.contains(&sha256_hex(b"a corrupted download")), "{err}");
        assert!(err.contains("corrupted or"), "{err}");
    }

    #[test]
    fn a_hash_recorded_in_upper_case_still_verifies() {
        let upper = "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD";
        assert_eq!(
            verify(b"abc", upper, "LogLens-0.2.0-linux-x86_64.tar.gz"),
            Ok(())
        );
    }

    // --- The temp directory ------------------------------------------------

    #[test]
    fn a_download_directory_deletes_itself() {
        let path = {
            let dir = TempDir::new().expect("a writable temp directory");
            let path = dir.path().to_path_buf();
            std::fs::write(path.join("LogLens-0.2.0-linux-x86_64.tar.gz"), b"bytes")
                .expect("a writable temp directory");
            assert!(path.is_dir());
            path
        };
        // Every failure path in `apply` drops one of these on the way out.
        assert!(!path.exists(), "{} outlived its TempDir", path.display());
    }

    #[test]
    fn a_kept_download_directory_survives() {
        // The Windows path: the installer is running out of this directory
        // when the app is closed, so it has to outlive the process.
        let dir = TempDir::new().expect("a writable temp directory");
        let path = dir.path().to_path_buf();
        dir.keep();
        drop(dir);
        assert!(path.is_dir());

        // And the next start collects it.
        clean_stale_downloads_in(&std::env::temp_dir());
        assert!(!path.exists(), "{} survived the sweep", path.display());
    }

    #[test]
    fn the_sweep_takes_only_its_own_leftovers() {
        let root = temp_dir("sweep");
        let ours = root.join(format!("{TEMP_PREFIX}1234-abcd"));
        let theirs = root.join("someone-elses-work");
        std::fs::create_dir_all(ours.join("nested")).expect("a writable temp directory");
        std::fs::create_dir_all(&theirs).expect("a writable temp directory");

        clean_stale_downloads_in(&root);

        assert!(!ours.exists());
        assert!(theirs.is_dir());

        std::fs::remove_dir_all(&root).ok();
    }

    // --- Unpacking ---------------------------------------------------------

    #[test]
    fn a_download_that_is_not_an_archive_fails_before_anything_runs() {
        let dir = TempDir::new().expect("a writable temp directory");
        let archive = dir.path().join(linux_artifact("0.2.0"));
        std::fs::write(&archive, b"<html>404 Not Found</html>").expect("a writable temp directory");

        let err = install_linux(&dir, &archive, "0.2.0", Path::new("/nonexistent/loglens"))
            .expect_err("this is not a tarball");
        assert!(err.starts_with("could not unpack the download"), "{err}");
    }

    #[test]
    fn an_archive_without_an_install_script_is_refused() {
        // Unpacks, so `tar` really ran, but carries no `install.sh`. Nothing
        // is executed: the alternative is running whatever else was in there.
        let dir = TempDir::new().expect("a writable temp directory");
        let staged = dir.path().join(linux_archive_dir("0.2.0"));
        std::fs::create_dir_all(&staged).expect("a writable temp directory");
        std::fs::write(staged.join("loglens"), b"not really a binary")
            .expect("a writable temp directory");

        let archive = dir.path().join(linux_artifact("0.2.0"));
        let tarred = Command::new("tar")
            .arg("czf")
            .arg(&archive)
            .arg("-C")
            .arg(dir.path())
            .arg(linux_archive_dir("0.2.0"))
            .status()
            .expect("tar is installed");
        assert!(tarred.success());
        std::fs::remove_dir_all(&staged).expect("a writable temp directory");

        let err = install_linux(&dir, &archive, "0.2.0", Path::new("/nonexistent/loglens"))
            .expect_err("no install.sh in this archive");
        assert!(err.contains("no install.sh"), "{err}");
    }

    #[test]
    fn a_failed_child_process_is_reported_by_its_last_words() {
        assert_eq!(
            last_line(
                b"Installing Log Lens 0.2.0\ninstall.sh: missing loglens next to install.sh\n"
            ),
            "install.sh: missing loglens next to install.sh",
        );
        assert_eq!(last_line(b""), "no output");
        assert_eq!(last_line(b"\n  \n"), "no output");
    }
}
