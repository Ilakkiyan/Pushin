//! Secret storage in the OS keychain — macOS Keychain, Windows Credential Manager, or the Linux
//! Secret Service — via the `keyring` crate. Used for Google OAuth tokens so they never sit in
//! plaintext SQLite (the DB only keeps non-secret metadata: email, calendar id, sync token, expiry).
//!
//! Best-effort by design: every call degrades gracefully (returns `false`/`None`) if the platform
//! keychain is unavailable, so callers can fall back to the DB rather than hard-failing a connect.
//!
//! Tests can swap the OS keychain for an in-memory store via `test_store::enable()` — the seam is
//! `#[cfg(test)]` only, so the production path is exactly the keyring path with zero overhead.

const SERVICE: &str = "com.pushin.app";

fn entry(key: &str) -> Option<keyring::Entry> {
    keyring::Entry::new(SERVICE, key).ok()
}

/// Store a secret (empty value clears it). Returns true only if it is safely stored.
pub fn set(key: &str, value: &str) -> bool {
    if value.is_empty() {
        clear(key);
        return true;
    }
    #[cfg(test)]
    if test_store::set(key, value) {
        return true;
    }
    matches!(entry(key).map(|e| e.set_password(value)), Some(Ok(())))
}

/// Read a secret, or `None` if it's absent or the keychain is unavailable.
pub fn get(key: &str) -> Option<String> {
    #[cfg(test)]
    if let Some(v) = test_store::get(key) {
        return v;
    }
    entry(key).and_then(|e| e.get_password().ok())
}

/// Remove a secret (no-op if absent / unavailable).
pub fn clear(key: &str) {
    #[cfg(test)]
    if test_store::clear(key) {
        return;
    }
    if let Some(e) = entry(key) {
        let _ = e.delete_credential();
    }
}

/// A process-global in-memory secret store for tests (the OS keychain isn't available/shared in CI).
/// When `enable()`d, all of `set`/`get`/`clear` route here instead of the keyring.
#[cfg(test)]
pub(crate) mod test_store {
    use std::collections::HashMap;
    use std::sync::Mutex;

    static MEM: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

    pub fn enable() {
        *MEM.lock().unwrap() = Some(HashMap::new());
    }
    pub fn disable() {
        *MEM.lock().unwrap() = None;
    }
    /// Returns true if the store is active (so the caller short-circuits the keyring).
    pub fn set(key: &str, value: &str) -> bool {
        match MEM.lock().unwrap().as_mut() {
            Some(m) => {
                m.insert(key.into(), value.into());
                true
            }
            None => false,
        }
    }
    /// `Some(maybe_value)` when active, `None` when the store is off (fall through to keyring).
    pub fn get(key: &str) -> Option<Option<String>> {
        MEM.lock().unwrap().as_ref().map(|m| m.get(key).cloned())
    }
    pub fn clear(key: &str) -> bool {
        match MEM.lock().unwrap().as_mut() {
            Some(m) => {
                m.remove(key);
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // One test owns the global in-memory store for its lifetime (avoids races with parallel tests).
    #[test]
    fn store_roundtrip_clear_and_empty_value() {
        test_store::enable();

        assert_eq!(get("google_token"), None, "absent secret reads as None");
        assert!(set("google_token", "ya29.secret"), "storing a value succeeds");
        assert_eq!(get("google_token").as_deref(), Some("ya29.secret"), "round-trips");

        // Overwrite.
        assert!(set("google_token", "ya29.refreshed"));
        assert_eq!(get("google_token").as_deref(), Some("ya29.refreshed"));

        // Empty value is a clear (and still reports success).
        assert!(set("google_token", ""));
        assert_eq!(get("google_token"), None);

        // Explicit clear of an absent key is a no-op.
        clear("never_set");
        assert_eq!(get("never_set"), None);

        test_store::disable();
    }
}
