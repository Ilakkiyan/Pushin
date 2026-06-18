//! Durable sync state in SQLite: this device's clock + id (`sync_self`) and the known peers + their
//! delta watermarks (`sync_peers`). Pure DB ops, unit-testable with `db::test_conn()`.

use super::hlc::HlcState;
use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};

fn get_kv(conn: &Connection, k: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT v FROM sync_self WHERE k = ?1", params![k], |r| r.get(0))
        .optional()?)
}
fn set_kv(conn: &Connection, k: &str, v: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO sync_self(k, v) VALUES(?1, ?2) ON CONFLICT(k) DO UPDATE SET v = ?2",
        params![k, v],
    )?;
    Ok(())
}

/// This device's HLC, persisted across restarts so its clock never regresses.
pub fn load_clock(conn: &Connection) -> Result<HlcState> {
    let wall = get_kv(conn, "hlc_wall")?.and_then(|s| s.parse().ok()).unwrap_or(0);
    let counter = get_kv(conn, "hlc_counter")?.and_then(|s| s.parse().ok()).unwrap_or(0);
    Ok(HlcState { wall, counter })
}
pub fn save_clock(conn: &Connection, clock: &HlcState) -> Result<()> {
    set_kv(conn, "hlc_wall", &clock.wall.to_string())?;
    set_kv(conn, "hlc_counter", &clock.counter.to_string())?;
    Ok(())
}

/// This device's stable node id (the Iroh public key, hex). Set once the endpoint binds.
pub fn node_id(conn: &Connection) -> Result<Option<String>> {
    get_kv(conn, "node_id")
}
pub fn set_node_id(conn: &Connection, id: &str) -> Result<()> {
    set_kv(conn, "node_id", id)
}

/// A human label for this device (shown in the peer list on others). Defaults to the OS hostname.
pub fn device_name(conn: &Connection) -> Result<String> {
    if let Some(n) = get_kv(conn, "device_name")? {
        return Ok(n);
    }
    let host = hostname();
    set_kv(conn, "device_name", &host)?;
    Ok(host)
}
pub fn set_device_name(conn: &Connection, name: &str) -> Result<()> {
    set_kv(conn, "device_name", name)
}

fn hostname() -> String {
    std::env::var("COMPUTERNAME") // Windows
        .or_else(|_| std::env::var("HOSTNAME")) // Linux/macOS (often)
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "device".into())
}

// ---------------- peers + watermarks ----------------

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Peer {
    pub node_id: String,
    pub name: String,
    pub last_seen: Option<String>,
    pub last_acked_hlc: String,
}

/// Whether the engine should use n0 relays for NAT traversal (default true). Off = LAN/direct-only.
pub fn use_relay(conn: &Connection) -> bool {
    get_kv(conn, "use_relay").ok().flatten().map(|v| v != "0").unwrap_or(true)
}
pub fn set_use_relay(conn: &Connection, on: bool) -> Result<()> {
    set_kv(conn, "use_relay", if on { "1" } else { "0" })
}

pub fn upsert_peer(conn: &Connection, node_id: &str, name: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO sync_peers(node_id, name) VALUES(?1, ?2) \
         ON CONFLICT(node_id) DO UPDATE SET name = ?2 WHERE ?2 <> ''",
        params![node_id, name],
    )?;
    Ok(())
}

pub fn list_peers(conn: &Connection) -> Result<Vec<Peer>> {
    let mut stmt = conn.prepare(
        "SELECT node_id, name, last_seen, last_acked_hlc FROM sync_peers ORDER BY name, node_id")?;
    let rows = stmt.query_map([], |r| {
        Ok(Peer {
            node_id: r.get(0)?,
            name: r.get(1)?,
            last_seen: r.get(2)?,
            last_acked_hlc: r.get(3)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn remove_peer(conn: &Connection, node_id: &str) -> Result<()> {
    conn.execute("DELETE FROM sync_peers WHERE node_id = ?1", params![node_id])?;
    Ok(())
}

pub fn watermark(conn: &Connection, peer: &str) -> Result<String> {
    Ok(conn
        .query_row("SELECT last_acked_hlc FROM sync_peers WHERE node_id = ?1", params![peer], |r| r.get(0))
        .optional()?
        .unwrap_or_default())
}

pub fn set_watermark(conn: &Connection, peer: &str, hlc: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO sync_peers(node_id, last_acked_hlc) VALUES(?1, ?2) \
         ON CONFLICT(node_id) DO UPDATE SET last_acked_hlc = ?2",
        params![peer, hlc],
    )?;
    Ok(())
}

pub fn touch_peer(conn: &Connection, peer: &str, now: &str) -> Result<()> {
    conn.execute(
        "UPDATE sync_peers SET last_seen = ?2 WHERE node_id = ?1",
        params![peer, now],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    #[test]
    fn clock_and_peers_roundtrip() {
        let c = db::test_conn();
        save_clock(&c, &HlcState { wall: 42, counter: 7 }).unwrap();
        assert_eq!(load_clock(&c).unwrap(), HlcState { wall: 42, counter: 7 });

        upsert_peer(&c, "node-b", "Laptop").unwrap();
        set_watermark(&c, "node-b", "00ff-00-x").unwrap();
        let peers = list_peers(&c).unwrap();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].name, "Laptop");
        assert_eq!(watermark(&c, "node-b").unwrap(), "00ff-00-x");

        // upsert with empty name keeps the old name but the row stays.
        upsert_peer(&c, "node-b", "").unwrap();
        assert_eq!(list_peers(&c).unwrap()[0].name, "Laptop");

        remove_peer(&c, "node-b").unwrap();
        assert!(list_peers(&c).unwrap().is_empty());
    }
}
