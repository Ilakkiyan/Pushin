//! The mobile→desktop **inference bridge** protocol — a second mesh channel (separate ALPN) that lets
//! a device with no local model borrow a peer's `llama-server` (chat) and embed server (embeddings).
//! Transport-agnostic like [`super::protocol`]: it runs over any `AsyncRead + AsyncWrite` and is
//! unit-tested over an in-memory duplex with a fake handler (no real model).
//!
//! One exchange is a fixed choreography: `Hello` (both, mesh-authenticated) → initiator sends
//! `Request` → responder runs it and sends `Reply`.

use super::frame::{read_frame, write_frame};
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::future::Future;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};

/// What a device without a local model asks a peer to run.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum InferRequest {
    /// A schema-constrained chat completion — returns the parsed JSON object.
    Chat { model: String, messages: Value, schema: Value },
    /// Embed one string — returns the vector.
    Embed { model: String, input: String },
}

/// The result the peer sends back (shape matches the request kind).
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum InferReply {
    Chat(Value),
    Embed(Vec<f32>),
}

#[derive(Serialize, Deserialize, Debug)]
enum Msg {
    /// Identify + prove mesh membership (the QUIC channel is already E2E-encrypted; `mesh` is the
    /// app-level "you belong to my network"). `name` is a human label for logs/UX.
    Hello { node: String, mesh: String, name: String },
    Request(InferRequest),
    Reply { ok: bool, reply: Option<InferReply>, error: Option<String> },
}

/// Exchange Hellos and verify the peer presents the same mesh secret. Returns (peer node, peer name).
async fn handshake<R, W>(mesh: &str, node: &str, name: &str, initiator: bool, r: &mut R, w: &mut W) -> Result<(String, String)>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let me = Msg::Hello { node: node.into(), mesh: mesh.into(), name: name.into() };
    let peer: Msg = if initiator {
        write_frame(w, &me).await?;
        read_frame(r).await?
    } else {
        let p = read_frame(r).await?;
        write_frame(w, &me).await?;
        p
    };
    match peer {
        Msg::Hello { node, mesh: m, name } if m == mesh && !m.is_empty() => Ok((node, name)),
        Msg::Hello { .. } => bail!("peer failed mesh authentication"),
        _ => bail!("expected Hello, got something else"),
    }
}

/// Responder side: authenticate the peer, run its request via `handle`, and send the reply. `handle`
/// is an async closure (production calls the local chat/embed server; tests return canned data).
pub async fn serve_infer<F, Fut, R, W>(mesh: &str, node: &str, name: &str, mut r: R, mut w: W, handle: F) -> Result<()>
where
    F: FnOnce(InferRequest) -> Fut,
    Fut: Future<Output = Result<InferReply>>,
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    handshake(mesh, node, name, false, &mut r, &mut w).await?;
    let request = match read_frame::<_, Msg>(&mut r).await? {
        Msg::Request(rq) => rq,
        _ => bail!("expected Request"),
    };
    let reply = match handle(request).await {
        Ok(reply) => Msg::Reply { ok: true, reply: Some(reply), error: None },
        Err(e) => Msg::Reply { ok: false, reply: None, error: Some(format!("{e:#}")) },
    };
    write_frame(&mut w, &reply).await?;
    // Clean EOF so the peer's final read isn't a reset (matters on a real QUIC stream).
    let _ = w.shutdown().await;
    Ok(())
}

/// Initiator side: ask a peer to run `req` and return its reply (or an error if the peer failed).
pub async fn request_infer<R, W>(mesh: &str, node: &str, name: &str, req: InferRequest, mut r: R, mut w: W) -> Result<InferReply>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    handshake(mesh, node, name, true, &mut r, &mut w).await?;
    write_frame(&mut w, &Msg::Request(req)).await?;
    match read_frame::<_, Msg>(&mut r).await? {
        Msg::Reply { ok: true, reply: Some(reply), .. } => Ok(reply),
        Msg::Reply { error, .. } => bail!("peer inference failed: {}", error.unwrap_or_else(|| "unknown".into())),
        _ => bail!("expected Reply"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::io::split;

    fn chat_req() -> InferRequest {
        InferRequest::Chat { model: "m".into(), messages: json!([]), schema: json!({}) }
    }

    #[tokio::test]
    async fn chat_round_trips_over_a_stream() {
        let (c1, c2) = tokio::io::duplex(1 << 16);
        let (ar, aw) = split(c1);
        let (br, bw) = split(c2);
        let responder = serve_infer("secret", "B", "Desk", br, bw, |req| async move {
            match req {
                InferRequest::Chat { .. } => Ok(InferReply::Chat(json!({ "events": [] }))),
                _ => bail!("expected chat"),
            }
        });
        let requester = request_infer("secret", "A", "Phone", chat_req(), ar, aw);
        let (rq, rp) = tokio::join!(requester, responder);
        rp.unwrap();
        match rq.unwrap() {
            InferReply::Chat(v) => assert_eq!(v, json!({ "events": [] })),
            _ => panic!("wrong reply kind"),
        }
    }

    #[tokio::test]
    async fn embed_round_trips_over_a_stream() {
        let (c1, c2) = tokio::io::duplex(1 << 16);
        let (ar, aw) = split(c1);
        let (br, bw) = split(c2);
        let responder = serve_infer("s", "B", "Desk", br, bw, |req| async move {
            match req {
                InferRequest::Embed { input, .. } => {
                    assert_eq!(input, "hi");
                    Ok(InferReply::Embed(vec![0.1, 0.2, 0.3]))
                }
                _ => bail!("expected embed"),
            }
        });
        let requester = request_infer("s", "A", "Phone", InferRequest::Embed { model: "e".into(), input: "hi".into() }, ar, aw);
        let (rq, _) = tokio::join!(requester, responder);
        match rq.unwrap() {
            InferReply::Embed(v) => assert_eq!(v, vec![0.1, 0.2, 0.3]),
            _ => panic!("wrong reply kind"),
        }
    }

    #[tokio::test]
    async fn wrong_mesh_secret_is_rejected() {
        let (c1, c2) = tokio::io::duplex(1 << 12);
        let (ar, aw) = split(c1);
        let (br, bw) = split(c2);
        let responder = serve_infer("secret", "B", "Desk", br, bw, |_| async move { Ok(InferReply::Chat(json!({}))) });
        let requester = request_infer("DIFFERENT", "A", "Phone", chat_req(), ar, aw);
        let (rq, rp) = tokio::join!(requester, responder);
        assert!(rq.is_err() || rp.is_err(), "mismatched mesh secret must fail");
    }

    #[tokio::test]
    async fn a_handler_error_surfaces_to_the_requester() {
        let (c1, c2) = tokio::io::duplex(1 << 12);
        let (ar, aw) = split(c1);
        let (br, bw) = split(c2);
        let responder = serve_infer("s", "B", "Desk", br, bw, |_| async move { bail!("model down") });
        let requester = request_infer("s", "A", "Phone", chat_req(), ar, aw);
        let (rq, _) = tokio::join!(requester, responder);
        let err = format!("{:#}", rq.unwrap_err());
        assert!(err.contains("model down"), "requester should see the peer's error, got: {err}");
    }
}
