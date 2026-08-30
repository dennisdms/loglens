//! Connection secrets, kept out of the config file.
//!
//! The OS keyring is the store of record. Where no keyring backend is available
//! (headless Linux, locked-down environments) we fall back to a per-session
//! in-memory map and the UI prompts the user to re-enter the secret each run.
//!
//! Every function here is synchronous and is called from the iced main thread,
//! so a keyring call blocks the UI for the whole round trip to the platform's
//! credential store. If the keyring is *locked*, that includes however long the
//! desktop's unlock prompt stays on screen. Accepted: it is rare, and the
//! alternative is threading an async boundary through every secret read. Revisit
//! only if it bites in practice.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

const SERVICE: &str = "loglens";

/// Secrets held only for this process, used when the keyring is unavailable.
static SESSION: LazyLock<Mutex<HashMap<String, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Where a stored secret ended up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stored {
    /// Persisted to the OS keyring; survives a restart.
    Keyring,
    /// Kept in memory only; gone when the app exits.
    Session,
}

fn session() -> std::sync::MutexGuard<'static, HashMap<String, String>> {
    SESSION.lock().unwrap_or_else(|e| e.into_inner())
}

/// Stores `secret` for connection `id`, preferring the keyring.
pub fn set(id: &str, secret: &str) -> Stored {
    let keyring_ok = keyring::Entry::new(SERVICE, id)
        .and_then(|e| e.set_password(secret))
        .is_ok();
    if keyring_ok {
        session().remove(id);
        Stored::Keyring
    } else {
        session().insert(id.to_string(), secret.to_string());
        Stored::Session
    }
}

/// Keeps `secret` for connection `id` for this session only, without touching
/// the keyring. Used when the user re-enters a secret at a prompt.
pub fn remember_session(id: &str, secret: &str) {
    session().insert(id.to_string(), secret.to_string());
}

/// The secret for connection `id`, from the keyring or the session map.
pub fn get(id: &str) -> Option<String> {
    if let Ok(entry) = keyring::Entry::new(SERVICE, id)
        && let Ok(secret) = entry.get_password()
    {
        return Some(secret);
    }
    session().get(id).cloned()
}

/// Forgets the secret for connection `id` everywhere.
pub fn delete(id: &str) {
    if let Ok(entry) = keyring::Entry::new(SERVICE, id) {
        let _ = entry.delete_credential();
    }
    session().remove(id);
}
