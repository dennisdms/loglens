//! Build script. Stamps the commit the binary was built from into the
//! executable, so `--version` can report it, and on Windows compiles the icon
//! and version information resource into the executable itself.

fn main() {
    // Version string: 0.1.0 (a1b2c3d) — the SHA is what makes a bug report
    // against "0.1.0" actionable when several builds share that version.
    let sha = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        // "unknown" is the honest answer when building from a source archive
        // with no `.git` present. Never fail the build over it.
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=LOGLENS_GIT_SHA={sha}");
    rerun_on_commit_change();
    embed_windows_resources();
}

/// Compiles `assets/app-icon/icon.ico` and the version information block into
/// the executable's Windows resources.
///
/// Without this the exe carries the generic Rust binary icon everywhere it is
/// seen as a file — Explorer, the taskbar, the Start menu, any shortcut
/// pointing at it — and reports nothing useful in its file properties. The
/// `window::Settings.icon` set in `base_window_settings()` only dresses the
/// live window; it does not touch the file on disk. Inno Setup points
/// `SetupIconFile` at the same `.ico`.
///
/// `cfg(windows)` on a build script is the *host*, and a build script only ever
/// runs on the host — which is also the only place `winresource` can invoke a
/// resource compiler. Releases are built on a Windows runner, so this runs
/// where it matters; cross-compiling to Windows from Linux would produce an
/// unadorned exe rather than a failure.
#[cfg(windows)]
fn embed_windows_resources() {
    println!("cargo:rerun-if-changed=assets/app-icon/icon.ico");

    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/app-icon/icon.ico");
    res.set("ProductName", "Log Lens");
    res.set(
        "FileDescription",
        "Log Lens — a desktop IDE for browsing logs",
    );
    res.compile().unwrap();
}

#[cfg(not(windows))]
fn embed_windows_resources() {}

/// Tell cargo which files to watch so a new commit restamps the SHA.
///
/// Watching only `.git/HEAD` is not enough: committing onto an
/// already-checked-out branch rewrites the *ref* (`.git/refs/heads/main`) and
/// leaves `HEAD` itself untouched, so the build script never reruns and the
/// binary keeps reporting the previous commit. That is invisible on a clean CI
/// checkout but not on any build that reuses a cached `target/` — and a
/// published Artifact stamped with a stale SHA is exactly the kind of lie the
/// release pipeline's tag-versus-version check exists to prevent.
///
/// Paths are resolved with `git rev-parse --git-path` rather than assembled by
/// hand, so this stays correct inside a worktree or a submodule, where `.git`
/// is a file rather than a directory. A source archive with no repository
/// present simply registers nothing.
fn rerun_on_commit_change() {
    let git_path = |arg: &str| -> Option<String> {
        std::process::Command::new("git")
            .args(["rev-parse", "--git-path", arg])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };

    // The branch ref HEAD points at, if HEAD is not detached.
    let head_ref = std::process::Command::new("git")
        .args(["symbolic-ref", "-q", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    // `packed-refs` covers the case where the loose ref file does not exist
    // because the ref has been packed.
    let watched = ["HEAD", "packed-refs"]
        .iter()
        .filter_map(|p| git_path(p))
        .chain(head_ref.as_deref().and_then(git_path));

    for path in watched {
        println!("cargo:rerun-if-changed={path}");
    }
}
