//! The Iroh adapter: bind a QUIC endpoint, encode/decode pairing tickets, and dial/accept the
//! single bidirectional stream that [`super::protocol::run_session`] drives. This is the only file
//! that touches the Iroh API — everything above it is transport-agnostic and unit-tested.

use anyhow::{Context, Result};
use iroh::endpoint::{presets, Connection};
use iroh::{Endpoint, EndpointAddr, SecretKey, TransportAddr};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Application-layer protocol id for the changeset-sync channel.
pub const ALPN: &[u8] = b"pushin-sync/0";
/// Application-layer protocol id for the mobile→desktop inference bridge (separate channel so it can't
/// disturb the sync choreography). Dispatched on `conn.alpn()` in the engine's accept loop.
pub const INFER_ALPN: &[u8] = b"pushin-infer/0";

/// Build this device's Iroh secret key from its persisted 32-byte seed.
pub fn secret_key(seed: [u8; 32]) -> SecretKey {
    SecretKey::from_bytes(&seed)
}

/// Bind a QUIC endpoint. `use_relay` off = LAN/direct-only (no n0 relays see even encrypted
/// traffic), at the cost of NAT-traversal reach.
pub async fn bind(secret: SecretKey, use_relay: bool) -> Result<Endpoint> {
    // iroh 1.x configures relays + discovery through a builder *preset* rather than separate
    // `relay_mode`/`discovery_n0` calls. `N0` is relays-on with n0 discovery; `N0DisableRelay` is
    // the LAN/direct-only mode the Settings toggle exposes.
    let alpns = vec![ALPN.to_vec(), INFER_ALPN.to_vec()];
    let ep = if use_relay {
        Endpoint::builder(presets::N0).secret_key(secret).alpns(alpns).bind().await
    } else {
        Endpoint::builder(presets::N0DisableRelay).secret_key(secret).alpns(alpns).bind().await
    };
    ep.context("binding the Iroh endpoint")
}

/// Does this address carry a relay path? Used to decide whether an invite is usable off-LAN.
pub fn has_relay(addr: &EndpointAddr) -> bool {
    addr.addrs.iter().any(|a| matches!(a, TransportAddr::Relay(_)))
}

/// The relay URLs in an address, as strings — for logging and diagnostics.
pub fn relay_urls(addr: &EndpointAddr) -> Vec<String> {
    addr.addrs
        .iter()
        .filter_map(|a| match a {
            TransportAddr::Relay(u) => Some(u.to_string()),
            _ => None,
        })
        .collect()
}

/// The direct (IP) addresses in an address, as strings — for logging and diagnostics.
pub fn direct_addrs(addr: &EndpointAddr) -> Vec<String> {
    addr.addrs
        .iter()
        .filter_map(|a| match a {
            TransportAddr::Ip(s) => Some(s.to_string()),
            _ => None,
        })
        .collect()
}

/// A pairing invite: where to reach this device + the shared network key. Base32 so it copy-pastes
/// and goes into a QR cleanly (case-insensitive, no symbols).
#[derive(Serialize, Deserialize)]
struct Ticket {
    addr: EndpointAddr,
    mesh: String,
}

/// How long minting an invite waits for the home relay before settling for a direct-only ticket.
const RELAY_WAIT: Duration = Duration::from_secs(10);

/// Mint an invite ticket: this endpoint's reachable address + the mesh secret.
///
/// `addr()` reports whatever is known *right now*, and local interface addresses appear in
/// milliseconds while the relay handshake takes seconds. Minting immediately therefore yields a
/// ticket carrying only the LAN address — no relay path — so the joiner has nothing to work with if
/// the direct route is blocked (a dismissed Windows Firewall prompt suffices). When relays are on,
/// poll briefly for one; giving up is fine, we fall back to a direct-only ticket rather than
/// failing the invite.
pub async fn make_ticket(ep: &Endpoint, mesh: &str, use_relay: bool) -> Result<String> {
    let mut addr = ep.addr();
    if use_relay && !has_relay(&addr) {
        let deadline = tokio::time::Instant::now() + RELAY_WAIT;
        while tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(250)).await;
            addr = ep.addr();
            if has_relay(&addr) {
                break;
            }
        }
    }
    let body = serde_json::to_vec(&Ticket { addr, mesh: mesh.to_string() })?;
    Ok(data_encoding::BASE32_NOPAD.encode(&body))
}

/// Decode an invite ticket back into (peer address, mesh secret).
pub fn parse_ticket(ticket: &str) -> Result<(EndpointAddr, String)> {
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
    addr: impl Into<EndpointAddr>,
) -> Result<(Connection, iroh::endpoint::SendStream, iroh::endpoint::RecvStream)> {
    let conn = ep.connect(addr, ALPN).await.context("dialing peer")?;
    let (send, recv) = conn.open_bi().await.context("opening sync stream")?;
    Ok((conn, send, recv))
}

/// Dial a peer and open an **inference** stream (borrow its model). Same node keys + mesh, different ALPN.
pub async fn dial_infer(
    ep: &Endpoint,
    addr: impl Into<EndpointAddr>,
) -> Result<(Connection, iroh::endpoint::SendStream, iroh::endpoint::RecvStream)> {
    let conn = ep.connect(addr, INFER_ALPN).await.context("dialing peer for inference")?;
    let (send, recv) = conn.open_bi().await.context("opening inference stream")?;
    Ok((conn, send, recv))
}

/// Accept the sync stream on an inbound connection.
pub async fn accept_stream(
    conn: &Connection,
) -> Result<(iroh::endpoint::SendStream, iroh::endpoint::RecvStream)> {
    let (send, recv) = conn.accept_bi().await.context("accepting sync stream")?;
    Ok((send, recv))
}

#[cfg(test)]
mod tests {
    use super::*;
}
