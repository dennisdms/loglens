//! The trail a crash leaves behind.
//!
//! A release build is linked as a GUI subsystem executable on Windows (see the
//! `windows_subsystem` attribute at the top of `main.rs`), so it has no console
//! and whatever a panic prints to stderr goes nowhere at all: the window
//! disappears and the user is left with nothing to report. Panics are therefore
//! also appended to a log file under the user's data directory, with a UTC
//! timestamp and the source location, so "it vanished" can become an actual bug
//! report. The About dialog shows this path so a user can find the file when
//! asked for it.
//!
//! Everything here is best effort, deliberately. A panic hook that panics
//! aborts the process — turning a crash into a worse crash and destroying the
//! very message it was trying to save — so every failure to write is swallowed
//! and nothing in the write path is allowed to panic.

use std::io::Write;
use std::path::{Path, PathBuf};

/// `~/.local/share/loglens/loglens.log`, or the platform equivalent. `None` on
/// a platform, or in an environment, with no data directory to speak of.
pub fn log_path() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("loglens").join("loglens.log"))
}

/// Installs the panic hook for the process. Call once, as early in `main` as
/// possible: a panic before this point is only reported wherever stderr
/// happens to go.
pub fn install_panic_hook() {
    install_panic_hook_at(log_path());
}

/// The body of [`install_panic_hook`] with the destination handed in, so tests
/// can point it somewhere they can read back.
///
/// The hook chains to whichever hook was installed before it, rather than
/// replacing it, so the standard panic message still reaches stderr wherever
/// there is one — a debug build, or a release build run from a terminal.
fn install_panic_hook_at(path: Option<PathBuf>) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Some(path) = &path {
            // Ignored on purpose: there is nowhere to report a failure to
            // report a crash, and trying would abort the process.
            let _ = append(path, &entry(info));
        }
        previous(info);
    }));
}

/// One log line: when it happened, where, and what was panicked with.
fn entry(info: &std::panic::PanicHookInfo<'_>) -> String {
    let when = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let location = info
        .location()
        .map_or_else(|| "unknown location".to_string(), ToString::to_string);
    format!("[{when}] panic at {location}: {}\n", message(info))
}

/// The value that was panicked with, for the two payload types `panic!`
/// produces. Anything else is a `panic_any` with a custom type, which cannot be
/// rendered here.
fn message<'a>(info: &'a std::panic::PanicHookInfo<'_>) -> &'a str {
    let payload = info.payload();
    if let Some(text) = payload.downcast_ref::<&str>() {
        text
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text
    } else {
        "<non-string panic payload>"
    }
}

/// Appends one entry, creating the containing directory if it is not there yet.
/// Every failure is returned rather than handled; the caller swallows it.
fn append(path: &Path, entry: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?
        .write_all(entry.as_bytes())
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;

    /// The panic hook is process-wide, so these tests cannot run at the same
    /// time as each other.
    static HOOK: Mutex<()> = Mutex::new(());

    /// A private directory under the system temp directory, unique per call.
    fn temp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let dir = std::env::temp_dir().join(format!("loglens-{tag}-{nanos:x}"));
        std::fs::create_dir_all(&dir).expect("a writable temp directory");
        dir
    }

    /// Panics under the hook pointed at `path`, and reports whether the hook it
    /// chains to ran. Returns once the panic has been caught; if the hook
    /// itself ever panicked the process would abort instead of returning.
    fn panic_under_hook(path: PathBuf, message: &'static str) -> bool {
        let chained = std::sync::Arc::new(AtomicBool::new(false));
        let flag = std::sync::Arc::clone(&chained);

        let previous = std::panic::take_hook();
        // Stands in for the default hook: records that the chain reached it,
        // and keeps the deliberate panic below off the test suite's stderr.
        std::panic::set_hook(Box::new(move |_| flag.store(true, Ordering::SeqCst)));
        install_panic_hook_at(Some(path));

        let caught = std::panic::catch_unwind(|| panic!("{}", message));
        std::panic::set_hook(previous);

        assert!(caught.is_err(), "the panic should have unwound normally");
        chained.load(Ordering::SeqCst)
    }

    #[test]
    fn a_panic_is_appended_with_its_timestamp_and_location() {
        let _guard = HOOK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = temp_dir("appends");
        // A directory that does not exist yet: the hook creates it.
        let path = dir.join("loglens").join("loglens.log");

        let chained = panic_under_hook(path.clone(), "a deliberate test panic");

        let logged = std::fs::read_to_string(&path).expect("the hook wrote the log");
        assert!(logged.contains("a deliberate test panic"), "{logged}");
        assert!(logged.contains("src/crashlog.rs"), "{logged}");
        assert!(logged.starts_with("[20"), "{logged}");
        assert!(logged.ends_with('\n'), "{logged}");
        assert!(chained, "the previously installed hook should still run");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_unwritable_location_degrades_silently() {
        let _guard = HOOK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = temp_dir("unwritable");
        // A log path whose parent is an existing *file*, so neither the
        // directory nor the file can be created. This stands in for every real
        // version of the failure: a read-only home, a full disk, a data
        // directory that is not a directory.
        let blocker = dir.join("not-a-directory");
        std::fs::write(&blocker, b"").expect("a writable temp directory");
        let path = blocker.join("loglens").join("loglens.log");
        assert!(append(&path, "unwritable\n").is_err());

        // Reaching the end of this call at all is the assertion: a panic inside
        // a panic hook aborts the process, taking the test binary with it.
        let chained = panic_under_hook(path.clone(), "a deliberate test panic");

        assert!(!path.exists());
        assert!(chained, "the previously installed hook should still run");

        std::fs::remove_dir_all(&dir).ok();
    }
}
