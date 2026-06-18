//! Device identity + the shared "tailnet key", stored in the OS keychain via [`crate::secrets`]
//! (same place as Google tokens — never plaintext SQLite). Two secrets:
//!
//! - **node key** (32 bytes): this device's stable Iroh keypair seed; its derived NodeId is the
//!   address peers dial. Created once per device.
//! - **mesh secret** (the "key" the user shares): proves membership of one private network. A device
//!   only accepts peers presenting the same mesh secret. Created when you start a network, or set
//!   from an invite when you join one. Absent ⇒ sync is off.
//!
//! Keychain access is best-effort (see `secrets`); if it's unavailable the identity simply won't
//! persist across restarts and the user re-pairs — sync degrades, it doesn't crash.

use crate::secrets;

const NODE_KEY: &str = "sync_node_key";
const MESH_KEY: &str = "sync_mesh_secret";

fn random32() -> [u8; 32] {
    let mut a = [0u8; 32];
    getrandom::getrandom(&mut a).expect("system rng");
    a
}

/// Load this device's 32-byte node key, generating + persisting one on first use.
pub fn load_or_create_node_key() -> [u8; 32] {
    if let Some(h) = secrets::get(NODE_KEY) {
        if let Ok(b) = hex::decode(h.trim()) {
            if b.len() == 32 {
                let mut a = [0u8; 32];
                a.copy_from_slice(&b);
                return a;
            }
        }
    }
    let a = random32();
    secrets::set(NODE_KEY, &hex::encode(a));
    a
}

/// The current mesh secret, or `None` if this device hasn't joined/created a network yet.
pub fn mesh_secret() -> Option<String> {
    secrets::get(MESH_KEY).filter(|s| !s.is_empty())
}

/// Adopt a mesh secret from an invite (joining someone's network).
pub fn set_mesh_secret(secret: &str) -> bool {
    secrets::set(MESH_KEY, secret)
}

/// Return the mesh secret, generating a fresh one if none exists (starting a new network).
pub fn ensure_mesh_secret() -> String {
    if let Some(s) = mesh_secret() {
        return s;
    }
    let s = data_encoding::BASE32_NOPAD.encode(&random32());
    secrets::set(MESH_KEY, &s);
    s
}

/// Leave the network: forget the mesh secret (the node key stays so the device keeps its identity).
pub fn forget_mesh() {
    secrets::clear(MESH_KEY);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_key_is_stable_and_mesh_lifecycle_works() {
        secrets::test_store::enable();
        // First call generates + persists; second returns the same bytes.
        let k1 = load_or_create_node_key();
        let k2 = load_or_create_node_key();
        assert_eq!(k1, k2);

        assert!(mesh_secret().is_none(), "no network until created/joined");
        let s = ensure_mesh_secret();
        assert!(!s.is_empty());
        assert_eq!(mesh_secret().as_deref(), Some(s.as_str()));
        // ensure is idempotent.
        assert_eq!(ensure_mesh_secret(), s);

        set_mesh_secret("JOINED-SECRET");
        assert_eq!(mesh_secret().as_deref(), Some("JOINED-SECRET"));
        forget_mesh();
        assert!(mesh_secret().is_none());
        // Node identity survives leaving the network.
        assert_eq!(load_or_create_node_key(), k1);
    }
}
