//! Secret storage in the OS keychain — macOS Keychain, Windows Credential Manager, or the Linux
//! Secret Service — via the `keyring` crate. Used for Google OAuth tokens so they never sit in
//! plaintext SQLite (the DB only keeps non-secret metadata: email, calendar id, sync token, expiry).
//!
//! Best-effort by design: every call degrades gracefully (returns `false`/`None`) if the platform
//! keychain is unavailable, so callers can fall back to the DB rather than hard-failing a connect.

const SERVICE: &str = "com.pushin.app";

fn entry(key: &str) -> Option<keyring::Entry> {
    keyring::Entry::new(SERVICE, key).ok()
}

/// Store a secret (empty value clears it). Returns true only if it is safely in the keychain.
pub fn set(key: &str, value: &str) -> bool {
    if value.is_empty() {
        clear(key);
        return true;
    }
    matches!(entry(key).map(|e| e.set_password(value)), Some(Ok(())))
}

/// Read a secret, or `None` if it's absent or the keychain is unavailable.
pub fn get(key: &str) -> Option<String> {
    entry(key).and_then(|e| e.get_password().ok())
}

/// Remove a secret (no-op if absent / unavailable).
pub fn clear(key: &str) {
    if let Some(e) = entry(key) {
        let _ = e.delete_credential();
    }
}
