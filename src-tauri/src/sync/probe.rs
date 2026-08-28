//! A self-test for the sync transport, runnable from inside the app.
//!
//! "Sync timed out" is one message for at least four different faults, and the app cannot tell them
//! apart from the inside: a ticket that describes no reachable path, a relay this device cannot get
//! to, a peer that is not running, and a peer that answers but rejects the protocol. Each needs a
//! different fix, and the pairing screen reports all of them identically.
//!
//! This walks the steps separately and writes each outcome to the diagnostics ring, so the answer is
//! readable on the device that is actually failing — which, being cross-device, is usually the one
//! with no console and no toolchain attached.
//!
//! It is deliberately inert: a throwaway node key (never this device's identity), no mesh
//! authentication, and it hangs up as soon as a stream opens. It cannot move data between devices,
//! so running it is always safe, including with a stranger's ticket.

use super::log;
use super::transport::{self, ALPN};
use anyhow::Result;
use iroh::{Endpoint, NodeAddr, RelayMode, RelayUrl, Watcher};
use std::time::Duration;

/// How long to wait for a relay to accept us before calling it unreachable.
const RELAY_WAIT: Duration = Duration::from_secs(15);
/// How long to wait for a dial. Longer than the relay wait — hole punching legitimately takes time.
const DIAL_WAIT: Duration = Duration::from_secs(30);

/// A node id abbreviated for a log line — enough to compare two devices, short enough to read.
fn short(id: &str) -> String {
    if id.len() > 12 { format!("{}…", &id[..12]) } else { id.to_string() }
}

/// Bind a throwaway endpoint. Never the app's own node key: two endpoints sharing one identity
/// would fight over the same relay registration and make the diagnosis worse than the fault.
async fn scratch_endpoint(use_relay: bool) -> Result<Endpoint> {
    let mut seed = [0u8; 32];
    getrandom::getrandom(&mut seed).map_err(|e| anyhow::anyhow!("random seed: {e}"))?;
    transport::bind(transport::secret_key(seed), use_relay).await
}

/// Run the transport self-test, writing every step to the diagnostics ring.
///
/// With a `ticket`, it tries to reach that peer. Without one, it tests only this device's own
/// ability to use a relay — which is worth knowing on its own, because a device that cannot reach a
/// relay cannot be paired with from anywhere beyond its own LAN.
pub async fn run(ticket: Option<String>) {
    log::info("--- connection test started ---");

    // 1. The ticket, if we were given one. A ticket carrying no relay URL and only private
    //    addresses can never work across networks, and that is visible without touching the network.
    let target = match ticket.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        Some(t) => match transport::parse_ticket(t) {
            Ok((addr, _mesh)) => {
                let relay = addr.relay_url().map(|r| r.to_string());
                let directs: Vec<String> = addr.direct_addresses().map(|a| a.to_string()).collect();
                log::info(format!("invite: peer {}", short(&addr.node_id.to_string())));
                match &relay {
                    Some(r) => log::info(format!("invite: relay {r}")),
                    None => log::warn(
                        "invite: no relay in this invite — it can only work on the same network",
                    ),
                }
                log::info(format!("invite: {} direct address(es) {:?}", directs.len(), directs));
                Some(addr)
            }
            Err(e) => {
                log::error(format!("invite: unreadable — {e:#}"));
                log::info("--- connection test finished ---");
                return;
            }
        },
        None => {
            log::info("no invite given — testing this device's own reach only");
            None
        }
    };

    // 2. Our own endpoint.
    let ep = match scratch_endpoint(true).await {
        Ok(ep) => ep,
        Err(e) => {
            log::error(format!("network: cannot open a connection at all — {e:#}"));
            log::info("--- connection test finished ---");
            return;
        }
    };

    // 3. Can we reach a relay? Without one, only same-network pairing can work.
    let relay_ok = match tokio::time::timeout(RELAY_WAIT, ep.home_relay().initialized()).await {
        Ok(Ok(r)) => {
            log::info(format!("relay: reachable ({r})"));
            true
        }
        Ok(Err(e)) => {
            log::error(format!("relay: unreachable — {e:#}"));
            false
        }
        Err(_) => {
            log::error("relay: timed out — no relay path from this network");
            false
        }
    };

    // 4. Can a relayed connection actually CARRY traffic? Reaching a relay only proves we can talk
    //    to it, not that it can route for us — and the two fail independently. Both endpoints here
    //    are ours, so a failure cannot be blamed on anyone else's device.
    if relay_ok {
        match relay_round_trip().await {
            Ok(true) => log::info("relay: routing works (self-test)"),
            Ok(false) => log::error(
                "relay: reachable but NOT routing — pairing across networks cannot work from here",
            ),
            Err(e) => log::warn(format!("relay: self-test inconclusive — {e:#}")),
        }
    }

    // 5. The dial itself, which is what pairing does.
    if let Some(addr) = target {
        let started = std::time::Instant::now();
        match tokio::time::timeout(DIAL_WAIT, transport::dial(&ep, addr)).await {
            Ok(Ok((conn, _s, _r))) => {
                log::info(format!("peer: reached in {:?}", started.elapsed()));
                log::info("peer: the network path is fine — pairing should work");
                conn.close(0u32.into(), b"probe");
            }
            Ok(Err(e)) => log::error(format!(
                "peer: refused after {:?} — it answered but would not connect ({e:#})",
                started.elapsed()
            )),
            Err(_) => {
                log::error("peer: timed out — nothing answered");
                log::info("peer: check the other device is open, and that its invite is fresh");
            }
        }
    }

    ep.close().await;
    log::info("--- connection test finished ---");
}

/// Two endpoints of our own, connected relay-only, to prove the relay will route for us.
///
/// Discovery is off and the dialled address carries no direct addresses; without both, the two
/// endpoints find each other over loopback and the test proves nothing.
async fn relay_round_trip() -> Result<bool> {
    // Seeds are drawn outside the closure: inside, `?` would have to convert into iroh's
    // `BindError`, which has no `From<anyhow::Error>`.
    let mut seeds = [[0u8; 32]; 2];
    for s in seeds.iter_mut() {
        getrandom::getrandom(s).map_err(|e| anyhow::anyhow!("random seed: {e}"))?;
    }
    let mk = |seed: [u8; 32]| async move {
        Endpoint::builder()
            .secret_key(transport::secret_key(seed))
            .alpns(vec![ALPN.to_vec()])
            .relay_mode(RelayMode::Default)
            .bind()
            .await
    };
    let a = mk(seeds[0]).await?;
    let b = mk(seeds[1]).await?;
    let relay: RelayUrl = match tokio::time::timeout(RELAY_WAIT, a.home_relay().initialized()).await
    {
        Ok(Ok(r)) => r,
        _ => {
            a.close().await;
            b.close().await;
            anyhow::bail!("the test endpoint could not register with a relay");
        }
    };
    let addr = NodeAddr::new(a.node_id()).with_relay_url(relay);
    let accept = tokio::spawn(async move {
        if let Some(incoming) = a.accept().await {
            let _ = incoming.await;
        }
    });
    let ok = matches!(
        tokio::time::timeout(DIAL_WAIT, b.connect(addr, ALPN)).await,
        Ok(Ok(_))
    );
    accept.abort();
    b.close().await;
    Ok(ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_node_id_is_shortened_for_reading_but_a_short_one_is_left_alone() {
        assert_eq!(short("fb4691d316c785188043f0acc"), "fb4691d316c7…");
        assert_eq!(short("abc"), "abc");
    }

    #[tokio::test]
    async fn an_unreadable_invite_is_reported_without_touching_the_network() {
        // The first thing a user does is paste the wrong thing. That has to come back as "unreadable
        // invite" immediately, not as a 30-second timeout that looks identical to a dead peer.
        log::clear();
        let started = std::time::Instant::now();
        run(Some("this is not a ticket".into())).await;
        assert!(started.elapsed() < Duration::from_secs(5), "must not have dialled anything");

        let lines = log::lines();
        assert!(
            lines.iter().any(|l| l.level == "error" && l.text.contains("unreadable")),
            "expected an 'unreadable' error, got {:?}",
            lines.iter().map(|l| &l.text).collect::<Vec<_>>()
        );
        assert!(
            lines.last().is_some_and(|l| l.text.contains("finished")),
            "the test must always close its own log block"
        );
        log::clear();
    }
}
