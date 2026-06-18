//! The Iroh adapter: bind a QUIC endpoint, encode/decode pairing tickets, and dial/accept the
//! single bidirectional stream that [`super::protocol::run_session`] drives. This is the only file
//! that touches the Iroh API — everything above it is transport-agnostic and unit-tested.

use anyhow::{Context, Result};
use iroh::endpoint::Connection;
use iroh::{Endpoint, NodeAddr, RelayMode, SecretKey, Watcher};
use serde::{Deserialize, Serialize};

/// Application-layer protocol id negotiated on every connection.
pub const ALPN: &[u8] = b"pushin-sync/0";

/// Build this device's Iroh secret key from its persisted 32-byte seed.
pub fn secret_key(seed: [u8; 32]) -> SecretKey {
    SecretKey::from_bytes(&seed)
}

/// Bind a QUIC endpoint. `use_relay` off = LAN/direct-only (no n0 relays see even encrypted
/// traffic), at the cost of NAT-traversal reach.
pub async fn bind(secret: SecretKey, use_relay: bool) -> Result<Endpoint> {
    let relay = if use_relay { RelayMode::Default } else { RelayMode::Disabled };
    Endpoint::builder()
        .secret_key(secret)
        .alpns(vec![ALPN.to_vec()])
        .relay_mode(relay)
        .discovery_n0()
        .bind()
        .await
        .context("binding the Iroh endpoint")
}

/// A pairing invite: where to reach this device + the shared network key. Base32 so it copy-pastes
/// and goes into a QR cleanly (case-insensitive, no symbols).
#[derive(Serialize, Deserialize)]
struct Ticket {
    addr: NodeAddr,
    mesh: String,
}

/// Mint an invite ticket: this endpoint's reachable address + the mesh secret.
pub async fn make_ticket(ep: &Endpoint, mesh: &str) -> Result<String> {
    let addr = ep
        .node_addr()
        .initialized()
        .await
        .context("resolving this device's node address")?;
    let body = serde_json::to_vec(&Ticket { addr, mesh: mesh.to_string() })?;
    Ok(data_encoding::BASE32_NOPAD.encode(&body))
}

/// Decode an invite ticket back into (peer address, mesh secret).
pub fn parse_ticket(ticket: &str) -> Result<(NodeAddr, String)> {
    let cleaned: String = ticket.trim().to_uppercase().chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = data_encoding::BASE32_NOPAD
        .decode(cleaned.as_bytes())
        .context("ticket is not valid base32")?;
    let t: Ticket = serde_json::from_slice(&bytes).context("ticket payload is malformed")?;
    Ok((t.addr, t.mesh))
}

/// Dial a peer and open the sync stream. Returns the connection + its (send, recv) halves.
pub async fn dial(
    ep: &Endpoint,
    addr: impl Into<NodeAddr>,
) -> Result<(Connection, iroh::endpoint::SendStream, iroh::endpoint::RecvStream)> {
    let conn = ep.connect(addr, ALPN).await.context("dialing peer")?;
    let (send, recv) = conn.open_bi().await.context("opening sync stream")?;
    Ok((conn, send, recv))
}

/// Accept the sync stream on an inbound connection.
pub async fn accept_stream(
    conn: &Connection,
) -> Result<(iroh::endpoint::SendStream, iroh::endpoint::RecvStream)> {
    let (send, recv) = conn.accept_bi().await.context("accepting sync stream")?;
    Ok((send, recv))
}
