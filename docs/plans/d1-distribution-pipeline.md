# Implementation plan: build, release, and update pipeline

**Status:** approved, ready to implement.
**Origin:** a full grilling session over four rounds. Every decision below is
load-bearing — it was chosen over stated alternatives for stated reasons. Do
not silently substitute a different choice because it seems simpler or more
idiomatic; if something here turns out to be wrong once you're in the code,
stop and raise it rather than deviating quietly.

## How to use this document

Work the steps in order. Each is a checkpoint: the code compiles, `cargo
clippy -- -D warnings` is clean, and the app still runs, before you move to
the next one. Steps 1–3 change no on-screen behavior.

Terms **Release**, **Artifact**, **Install flavour**, **Update check** and
**Update** are defined in `CONTEXT.md`. The distribution decisions themselves
are recorded in `docs/adr/0003-user-scope-distribution.md`.

## Non-negotiable constraints

These cut across every step. Violating one is a plan deviation even if no
single step repeats it.

- **No admin rights, ever.** Nothing in the install or update path may
  trigger UAC, `sudo`, or write outside the user's own directories. This is
  the constraint the whole design hangs off; if a step seems to need
  elevation, the step is wrong.
- **x86_64 only, Windows and Linux only.** No macOS target, no `aarch64`.
- **No code signing.** Windows SmartScreen will warn on first run; that is
  accepted and documented, not worked around.
- **Artifact names are a contract** (step 4.4). The Update check matches on
  them, so a rename breaks self-update for every already-installed copy —
  the one population that cannot fix itself. Changing the convention later
  requires a deliberate migration, not a commit.
- **A partially-published Release is worse than none.** The Release is
  created as a draft, published only once every Artifact has uploaded. If
  any platform's build fails, no Release is published at all.
- **Settings and secrets are never touched by install, update, or
  uninstall.** `dirs::config_dir()/loglens/config.json` and the OS keyring
  sit outside every install directory, deliberately.
- **The Linux keyring backend stays pure Rust on the `async-io` runtime.**
  See step 2 — switching it to the `tokio` feature reintroduces a documented
  deadlock. There is a comment saying so; do not "clean it up".

---

## Step 1 — App identity rename

**Visible change: none** (the window may re-bind its icon in the dock).

`APP_ID` is currently the string `"Log Lens"`, used as the Wayland
`application_id` and mirrored in `StartupWMClass`. Freedesktop convention is
a reverse-DNS ID whose `.desktop` filename and icon basename match it
exactly — that filename match is how GNOME binds a window to its launcher
icon in the dock and alt-tab. A space in an `application_id` works until a
compositor decides it doesn't. Do this before anything ships, while the ID
is still free to change.

### 1.1 Constant

`src/main.rs:65`:

```rust
const APP_ID: &str = "io.github.dennisdms.LogLens";
```

The binary stays `loglens`. The display name stays `Log Lens`.

### 1.2 Desktop entry

Rename `assets/loglens.desktop` → `assets/io.github.dennisdms.LogLens.desktop`
and set:

```ini
Icon=io.github.dennisdms.LogLens
StartupWMClass=io.github.dennisdms.LogLens
```

`Exec=` is rewritten at install time to an absolute path (step 4.2) — leave
it as `loglens` in the source file.

### 1.3 Icon name

The installed icon becomes
`~/.local/share/icons/hicolor/256x256/apps/io.github.dennisdms.LogLens.png`.
`assets/app-icon/icon.png` keeps its repo path; only the installed name
changes.

---

## Step 2 — Keyring: drop the C dependency

**Visible change: none.**

`keyring`'s `sync-secret-service` feature pulls `dbus-secret-service` →
`dbus`, which is C bindings to libdbus-1: a `libdbus-1-dev` build dependency
and a `libdbus-1.so.3` runtime dependency. `async-secret-service` +
`crypto-rust` is pure Rust over `zbus` and drops both.

### 2.1 Cargo.toml

```toml
keyring = { version = "3", features = [
    "apple-native",
    "windows-native",
    # Linux: pure-Rust secret-service over zbus. The runtime feature MUST stay
    # `async-io`, not `tokio`: with the tokio runtime, keyring calls made on the
    # main thread deadlock (keyring-3.6.3/src/secret_service.rs, "Tokio runtime
    # caution"), and all of secrets.rs is called from the iced main thread.
    "async-secret-service",
    "crypto-rust",
    "async-io",
] }
```

Keep that comment. It is the only thing standing between a future reader and
a deadlock that looks like a hang with no error.

### 2.2 No code changes

`keyring`'s public API stays synchronous — the async store is wrapped in
`secret_service::blocking`. All seven call sites (`src/connection.rs:111`,
`src/main.rs:562`, `565`, `687`, `1507`, `1512`, `1609`) compile unchanged.

Add to the `src/secrets.rs` module doc: the keyring call blocks the UI thread
for the duration of the secret-service round trip, and if the keyring is
*locked* that includes however long the desktop's unlock prompt is on screen.
Accepted: rare, and the alternative is threading an async boundary through
every secret read. Revisit only if it bites in practice.

### 2.3 Verify

`cargo tree -i dbus` must report that nothing depends on it. That is the
whole point of the step; if `dbus` is still in the tree, a feature is wrong.

---

## Step 3 — Native-feel groundwork and `--version`

**Visible change on Windows: no console window behind the app.**

### 3.1 Kill the console

Top of `src/main.rs`, before anything else:

```rust
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
```

Release builds get no console — a console flashing up behind a GUI launched
from the Start menu is the single loudest "this is not a real app" tell on
Windows. Debug builds keep it, so `cargo run` still prints.

### 3.2 Crash log

Because 3.1 discards stdout and stderr in release, a release-mode panic
otherwise vanishes without trace. Install a panic hook early in `main` that
appends the panic message, location, and a UTC timestamp to
`dirs::data_dir()/loglens/loglens.log`, creating the directory if needed,
then chains to the previous hook. Best-effort: any IO error writing the log
is swallowed — a failure to log must never itself abort.

The About dialog (step 7.5) shows this path so a user can find it when asked.

### 3.3 `build.rs`

New file at the repo root. Two jobs:

```rust
fn main() {
    // Version string: 0.2.0 (a1b2c3d) — the SHA is what makes a bug report
    // against "0.2.0" actionable when several builds share that version.
    let sha = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=LOGLENS_GIT_SHA={sha}");
    println!("cargo:rerun-if-changed=.git/HEAD");

    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/app-icon/icon.ico");
        res.set("ProductName", "Log Lens");
        res.set("FileDescription", "Log Lens — a desktop IDE for browsing logs");
        res.compile().unwrap();
    }
}
```

`"unknown"` is the honest answer when built from a source tarball with no
`.git` — do not fail the build over it.

`[build-dependencies] winresource = "0.1"` (target-gated to Windows).

### 3.4 Windows icon file

Commit `assets/app-icon/icon.ico` as a multi-resolution ICO (16, 32, 48, 64,
128, 256) generated from `icon.svg`. Without it the exe carries the generic
Rust binary icon in the taskbar, in Explorer, and in the Start menu — the
`window::Settings.icon` set in `base_window_settings()` only affects the
live window, not the file on disk or its shortcuts. Inno also points
`SetupIconFile` at it (step 4.1).

### 3.5 `--version` / `--help`

Before `iced` is started, hand-roll the argument check — do not add `clap`
for two flags:

```rust
const VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"), " (", env!("LOGLENS_GIT_SHA"), ")"
);
```

`--version` / `-V` prints `Log Lens <VERSION>` and exits 0. `--help` / `-h`
prints a two-line usage and exits 0. Anything else is ignored and the app
starts as normal.

This is what CI runs to prove an artifact is not dead on arrival (step 6.3).
It exits before any window is opened, so it works on a headless runner —
which is the entire reason it exists.

---

## Step 4 — Packaging

New top-level `packaging/` directory. Nothing here is compiled; it is
consumed by the release workflow.

### 4.1 `packaging/windows/loglens.iss` (Inno Setup)

Per-user install, modelled on the VS Code User Installer:

```ini
[Setup]
AppId={{io.github.dennisdms.LogLens}
AppName=Log Lens
AppVersion={#AppVersion}
DefaultDirName={localappdata}\Programs\Log Lens
DefaultGroupName=Log Lens
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
SetupIconFile=..\..\assets\app-icon\icon.ico
UninstallDisplayIcon={app}\loglens.exe
CloseApplications=yes
RestartApplications=yes
OutputBaseFilename=LogLens-{#AppVersion}-windows-x86_64-setup
```

- `PrivilegesRequired=lowest` is the no-admin guarantee: install goes to
  `{localappdata}\Programs`, the uninstall entry lands under the user's
  hive, and no UAC prompt appears.
- `CloseApplications` + `RestartApplications` let the Restart Manager close
  a running Log Lens during a silent update and bring it back afterwards
  (step 7.4). Windows will not let a running `.exe` be overwritten, so this
  is not optional.

`[Icons]` — a Start menu entry always (`{group}\Log Lens`), a desktop icon
behind an *unchecked* `[Tasks]` entry. `[Run]` relaunches after an
interactive install with `nowait postinstall skipifsilent`; the silent
update path relies on the Restart Manager instead.

**No PATH entry.** Not offered, not defaulted.

`[Code]` writes `install-manifest.json` (4.3) into `{app}` after install.

### 4.2 `packaging/linux/install.sh` and `uninstall.sh`

`install.sh` is run by the user from the unpacked tarball, or by the app
during an update with `--quiet`. It writes only under `$HOME`:

| What | Where |
|---|---|
| binary | `~/.local/bin/loglens` |
| desktop entry | `~/.local/share/applications/io.github.dennisdms.LogLens.desktop` |
| icon | `~/.local/share/icons/hicolor/256x256/apps/io.github.dennisdms.LogLens.png` |
| manifest + uninstaller | `~/.local/share/loglens/` |

Rules:

- Rewrite `Exec=` to the **absolute** `$HOME/.local/bin/loglens` as the file
  is copied. The launcher then works regardless of `PATH`, which on a fresh
  Debian does not include `~/.local/bin`.
- Run `update-desktop-database` and `gtk-update-icon-cache` if present;
  ignore them if not. A missing cache tool must not fail the install.
- If `~/.local/bin` is not on `PATH`, **print a note** saying so. Do not
  edit the user's `.bashrc`, `.zshrc`, or `.profile` — an installer that
  silently rewrites shell configuration earns a bug report.
- Copy `uninstall.sh` to `~/.local/share/loglens/uninstall.sh`, so it is
  still findable long after the tarball is deleted.

`uninstall.sh` removes exactly those four paths and nothing else. It prints
the config path and says the settings were kept (see 4.5).

### 4.3 `install-manifest.json` — the Install flavour marker

Written by both installers, next to the binary on Windows (`{app}`) and in
`~/.local/share/loglens/` on Linux:

```json
{
  "flavour": "installer",
  "install_dir": "/home/you/.local/bin",
  "version": "0.2.0"
}
```

The app reads it to decide whether it may update itself (step 7.4). It
**must** compare the recorded `install_dir` against
`std::env::current_exe()`'s parent and treat a mismatch as portable —
otherwise a portable copy run on a machine that also has Log Lens installed
picks up the installed copy's manifest and cheerfully "updates" a directory
it is not running from.

Absent or mismatched manifest ⇒ **Portable** ⇒ the update banner links to
the releases page and shows no Update button.

### 4.4 Artifact names — the contract

```
LogLens-<version>-windows-x86_64-setup.exe
LogLens-<version>-windows-x86_64-portable.zip
LogLens-<version>-linux-x86_64.tar.gz
SHA256SUMS
```

Version in the middle, platform and arch explicit, flavour last. The Update
check matches on this shape; adding `aarch64` later slots in without
touching the matcher. **Do not rename these casually** — see the constraints.

The tarball unpacks into a single `LogLens-<version>-linux-x86_64/`
directory containing `loglens`, `install.sh`, `uninstall.sh`, the desktop
entry, and the icon. No tarbombs.

### 4.5 What uninstall leaves behind

Both uninstallers leave `dirs::config_dir()/loglens/` and every keyring
entry untouched, and say so on the way out ("Your connections and settings
were kept in `<path>`"). Reinstall-to-fix is the most common reason anyone
uninstalls; wiping their configured Connections and stored credentials for
that is hostile. The README documents the manual removal path for people who
genuinely mean it.

---

## Step 5 — `.github/workflows/ci.yml`

On `push` and `pull_request`. One job, `ubuntu-22.04`:

1. `actions/checkout`
2. `dtolnay/rust-toolchain@stable` with `rustfmt`, `clippy`
3. `Swatinem/rust-cache`
4. `cargo fmt --check`
5. `cargo clippy --all-targets -- -D warnings`
6. `cargo test`

Linux only. Compile errors in this codebase are near-always
platform-agnostic, and the release workflow builds Windows anyway; a Windows
CI leg doubles minutes for rare value.

**Start with no `apt-get` step.** After step 2 the tree has no C library
dependency, and `winit`/`wgpu` `dlopen` their X11 and Wayland libraries at
runtime rather than linking them. If the first run disproves that, add
exactly the packages the error names — likely `libxkbcommon-dev` — and
record which and why in a comment. Do not pre-emptively paste a long
`apt-get` line copied from another project.

---

## Step 6 — `.github/workflows/release.yml`

On `push` of tags matching `v*`. `permissions: contents: write`.

### 6.1 `verify-version`

Fails the whole workflow unless the tag equals `v` + the `Cargo.toml`
version, exactly (so `v0.2.0-rc.1` requires `version = "0.2.0-rc.1"`). A
release whose tag and reported version disagree is a pipeline that lies to
you, and every later bug report inherits the lie.

### 6.2 `build-linux` — `ubuntu-22.04`

Pinned, not `ubuntu-latest`. A binary built against glibc 2.39 (24.04)
refuses to start on Ubuntu 22.04 or Debian 12 with `GLIBC_2.39 not found`;
newer glibc runs older binaries, never the reverse. 22.04 gives glibc 2.35
and covers Ubuntu 22.04+ and Debian 12+.

`cargo build --release`, `strip`, stage the tarball layout from 4.4,
`tar czf`.

### 6.3 `build-windows` — `windows-latest`

`cargo build --release`, then `iscc` on the Inno script (Inno Setup is
preinstalled on GitHub's Windows runners — if that changes, install it via
`choco install innosetup`), then zip the portable bundle.

### 6.4 Smoke test — in both build jobs

Run the freshly built binary with `--version` and assert the output contains
the expected version. This catches a missing shared library, a bad link, or
a wrong-architecture build — the failures that actually happen — without
pretending to be a GUI test. An iced window cannot be meaningfully launched
on either runner, and `xvfb` would only test a configuration nobody runs.

### 6.5 `release` — needs all three jobs

1. Download every artifact.
2. Generate `SHA256SUMS` over all three files.
3. `gh release create "$TAG" --draft --generate-notes`, adding
   `--prerelease` when the tag contains `-rc` or `-beta`.
4. Upload all four assets.
5. `gh release edit "$TAG" --draft=false`.

The draft-then-publish order is the point: until step 5 runs, no user and no
Update check can see a Release with half its Artifacts. Any build job
failing means nothing is ever published — fix and re-tag.

---

## Step 7 — `src/update.rs` and the Help menu

### 7.1 New dependency

`sha2 = "0.10"`, to verify downloads against `SHA256SUMS`. With no code
signing this is the only integrity check there is. It does not defend
against an attacker who controls the Release — they would control both files
— but it does catch a corrupt or truncated download and a tampered CDN hop,
and it costs one dependency and one CI line.

### 7.2 Checking

```rust
pub struct Release {
    pub version: String,       // "0.2.0"
    pub notes: String,         // GitHub's generated body, shown in the banner
    pub html_url: String,      // where the Portable flavour sends the user
    pub assets: Vec<Asset>,
}

pub async fn check() -> Result<Option<Release>, String>;
```

`GET https://api.github.com/repos/dennisdms/loglens/releases/latest`, with
`User-Agent: LogLens/<version>` (GitHub rejects requests without one) and
`Accept: application/vnd.github+json`. `reqwest` is already a dependency with
`json` and `rustls-tls`.

`/releases/latest` excludes pre-releases server-side, so `v*-rc*` tags are
invisible here for free — no client-side filtering to get wrong.

Version comparison is a hand-rolled `parse_version(&str) -> Option<(u64, u64,
u64)>` compared as a tuple. Do not add `semver` for this; anything with a
pre-release suffix has already been filtered out by the endpoint.

Unauthenticated GitHub API is 60 requests/hour per IP. A once-a-day check
cannot approach that alone, but a shared office IP can — which is why 7.6
treats a failed background check as a non-event.

### 7.3 Cadence

`Config` gains `#[serde(default)] last_update_check: Option<DateTime<Utc>>`
(`chrono` is already a dependency). On startup, check only if that is more
than 24 hours ago. `Help > Check for updates…` always checks, ignoring the
timestamp.

Surface a hit as a **dismissible banner** below the menu bar: version,
release notes, an Update button, a dismiss ✕. Not a modal — a modal
interrupts whatever query the user is reading. Not a subtle dot — too easy
to never notice.

### 7.4 Applying

Only when the Install flavour (4.3) is Installer-managed:

- **Windows:** download `…-setup.exe`, verify its SHA-256, save config, spawn
  it with `/SILENT /NORESTART`, exit. The Restart Manager (`CloseApplications`
  + `RestartApplications` in 4.1) closes and relaunches the app.
- **Linux:** download `…-linux-x86_64.tar.gz`, verify, unpack into a temp
  directory, run its `install.sh --quiet`, then re-exec the new binary.

Reusing the installers rather than hand-rolling a binary swap is deliberate:
a raw swap silently skips shortcut, uninstall-entry, `.desktop` and icon
updates, so installed metadata drifts from what is on disk by v0.5. Downloads
go to a temp directory and are deleted after use.

**Portable flavour:** the banner shows the new version and a link to
`html_url`. No Update button. Running the installer would install a *second*
copy into `%LOCALAPPDATA%` while the user carried on running the old one from
their USB stick.

### 7.5 Help menu

The menu bar gains `Help`, alongside `File` and the inert `View`:

- `Check for updates…`
- `About` — modal showing `Log Lens <version> (<sha>)`, the repository link,
  and the crash-log path from 3.2.

### 7.6 Failure policy

- Background check fails ⇒ **silent**. Log it, show nothing. Nobody asked.
- Manual check fails ⇒ **visible error**. The user asked and deserves an
  answer.
- Download or checksum fails ⇒ **always visible**, with the option to open
  the releases page. The user clicked Update.

A checksum mismatch is reported as a failed download, never silently retried
and never applied anyway.

---

## Out of scope, decided rather than forgotten

- **No `.deb`.** It cannot be installed without root, which contradicts the
  no-admin constraint, and a system-wide copy could not self-update.
- **No AppImage.** Admin-free, but produces no launcher entry without
  separate integration tooling — the opposite of what "searchable in the
  launcher" asks for.
- **No code signing.** README documents the SmartScreen click-through
  ("More info" → "Run anyway"). The workflow leaves an obvious seam for a
  signing step later.
- **No `.log` file association.** The app has no local-file code path at
  all; registering as a handler would open the app and ignore the file.
- **No `aarch64`, no macOS.**
- **No auto-start on login, no telemetry, no crash reporting upload.**
