//! Length-prefixed JSON framing shared by the mesh protocols (`protocol` = sync, `infer` = the
//! mobile→desktop inference bridge). Each frame is a 4-byte big-endian length followed by that many
//! bytes of JSON, over anything `AsyncRead + AsyncWrite` (an Iroh QUIC bi-stream in production; an
//! in-memory duplex in tests).

use anyhow::{bail, Result};
use serde::{de::DeserializeOwned, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Hard cap on a single framed message (guards against a hostile/buggy peer forcing a huge alloc).
pub const MAX_FRAME: u32 = 128 * 1024 * 1024;

/// Serialize `msg` to JSON and write it as one length-prefixed frame.
pub async fn write_frame<W: AsyncWrite + Unpin, T: Serialize>(w: &mut W, msg: &T) -> Result<()> {
    let bytes = serde_json::to_vec(msg)?;
    if bytes.len() as u64 > MAX_FRAME as u64 {
        bail!("outgoing frame too large: {} bytes", bytes.len());
    }
    w.write_all(&(bytes.len() as u32).to_be_bytes()).await?;
    w.write_all(&bytes).await?;
    w.flush().await?;
    Ok(())
}

/// Read one length-prefixed frame and deserialize it from JSON.
pub async fn read_frame<R: AsyncRead + Unpin, T: DeserializeOwned>(r: &mut R) -> Result<T> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len).await?;
    let len = u32::from_be_bytes(len);
    if len > MAX_FRAME {
        bail!("incoming frame too large: {len} bytes");
    }
    let mut buf = vec![0u8; len as usize];
    r.read_exact(&mut buf).await?;
    Ok(serde_json::from_slice(&buf)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use tokio::io::AsyncWriteExt;

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct Msg {
        kind: String,
        n: u64,
    }

    fn msg(kind: &str, n: u64) -> Msg {
        Msg { kind: kind.into(), n }
    }

    /// A pair of in-memory duplex halves standing in for an Iroh bi-stream.
    fn pipe() -> (tokio::io::DuplexStream, tokio::io::DuplexStream) {
        tokio::io::duplex(1024 * 1024)
    }

    #[tokio::test]
    async fn a_frame_round_trips() {
        let (mut a, mut b) = pipe();
        let sent = msg("hello", 7);
        write_frame(&mut a, &sent).await.unwrap();
        let got: Msg = read_frame(&mut b).await.unwrap();
        assert_eq!(got, sent);
    }

    #[tokio::test]
    async fn frames_keep_their_boundaries_back_to_back() {
        // The whole point of length prefixing: three messages written in a row must come back as
        // three, not as one run-on parse or a stall.
        let (mut a, mut b) = pipe();
        for i in 0..3u64 {
            write_frame(&mut a, &msg("tick", i)).await.unwrap();
        }
        for i in 0..3u64 {
            let got: Msg = read_frame(&mut b).await.unwrap();
            assert_eq!(got, msg("tick", i));
        }
    }

    #[tokio::test]
    async fn an_empty_payload_and_a_large_one_both_survive() {
        let (mut a, mut b) = pipe();
        write_frame(&mut a, &msg("", 0)).await.unwrap();
        let got: Msg = read_frame(&mut b).await.unwrap();
        assert_eq!(got, msg("", 0));

        // ~256 KB of payload — comfortably over any single socket read.
        let big = msg(&"x".repeat(256 * 1024), u64::MAX);
        write_frame(&mut a, &big).await.unwrap();
        let got: Msg = read_frame(&mut b).await.unwrap();
        assert_eq!(got, big);
    }

    #[tokio::test]
    async fn non_ascii_payloads_survive_the_byte_framing() {
        let (mut a, mut b) = pipe();
        let m = msg("caf\u{e9} \u{2615} \u{4f1a}\u{8b70} \u{1f600}", 1);
        write_frame(&mut a, &m).await.unwrap();
        let got: Msg = read_frame(&mut b).await.unwrap();
        assert_eq!(got, m, "the length prefix counts BYTES, not characters");
    }

    #[tokio::test]
    async fn an_oversized_length_prefix_is_refused_without_allocating_for_it() {
        // A hostile or buggy peer announcing a huge frame must be rejected on the header, before the
        // `vec![0; len]` that would otherwise let it dictate our memory use.
        let (mut a, mut b) = pipe();
        a.write_all(&(MAX_FRAME + 1).to_be_bytes()).await.unwrap();
        a.flush().await.unwrap();
        let err = read_frame::<_, Msg>(&mut b).await.unwrap_err();
        assert!(err.to_string().contains("too large"), "got {err}");
    }

    #[tokio::test]
    async fn the_maximum_advertised_length_is_still_refused_politely_when_the_body_never_arrives() {
        // Exactly MAX_FRAME is allowed by the cap, so the read proceeds — and then the peer hangs up
        // without sending the body. That must surface as an error, not a panic.
        let (mut a, mut b) = pipe();
        a.write_all(&MAX_FRAME.to_be_bytes()).await.unwrap();
        a.flush().await.unwrap();
        drop(a);
        assert!(read_frame::<_, Msg>(&mut b).await.is_err());
    }

    #[tokio::test]
    async fn a_truncated_header_is_an_error_not_a_hang_or_a_panic() {
        let (mut a, mut b) = pipe();
        a.write_all(&[0u8, 0u8]).await.unwrap(); // half a length prefix
        a.flush().await.unwrap();
        drop(a);
        assert!(read_frame::<_, Msg>(&mut b).await.is_err());
    }

    #[tokio::test]
    async fn a_truncated_body_is_an_error() {
        let (mut a, mut b) = pipe();
        a.write_all(&100u32.to_be_bytes()).await.unwrap();
        a.write_all(b"only a few bytes").await.unwrap();
        a.flush().await.unwrap();
        drop(a);
        assert!(read_frame::<_, Msg>(&mut b).await.is_err());
    }

    #[tokio::test]
    async fn a_correctly_framed_but_non_json_body_is_an_error_not_a_panic() {
        let (mut a, mut b) = pipe();
        let junk = b"\xff\xfe not json at all";
        a.write_all(&(junk.len() as u32).to_be_bytes()).await.unwrap();
        a.write_all(junk).await.unwrap();
        a.flush().await.unwrap();
        assert!(read_frame::<_, Msg>(&mut b).await.is_err());
    }

    #[tokio::test]
    async fn valid_json_of_the_wrong_shape_is_an_error() {
        // A peer on a different protocol version sends well-formed JSON we cannot use.
        let (mut a, mut b) = pipe();
        write_frame(&mut a, &serde_json::json!({ "totally": "different" })).await.unwrap();
        assert!(read_frame::<_, Msg>(&mut b).await.is_err());
    }

    #[tokio::test]
    async fn a_zero_length_frame_is_read_without_blocking() {
        // A 0-length frame is legal framing but empty JSON, so it must fail to DESERIALIZE rather
        // than block forever waiting for a body that is already complete.
        let (mut a, mut b) = pipe();
        a.write_all(&0u32.to_be_bytes()).await.unwrap();
        a.flush().await.unwrap();
        assert!(read_frame::<_, Msg>(&mut b).await.is_err());
    }

    #[tokio::test]
    async fn a_stream_that_closes_cleanly_between_frames_reports_eof() {
        let (mut a, mut b) = pipe();
        write_frame(&mut a, &msg("last", 1)).await.unwrap();
        drop(a);
        let got: Msg = read_frame(&mut b).await.unwrap();
        assert_eq!(got, msg("last", 1));
        assert!(read_frame::<_, Msg>(&mut b).await.is_err(), "the next read reports the close");
    }

    #[tokio::test]
    async fn an_error_on_one_frame_does_not_corrupt_the_next() {
        // Framing recovery: a body we cannot deserialize is fully consumed, so the following frame
        // still lines up. Without that, one bad message desynchronises the stream forever.
        let (mut a, mut b) = pipe();
        write_frame(&mut a, &serde_json::json!({ "wrong": true })).await.unwrap();
        write_frame(&mut a, &msg("recovered", 42)).await.unwrap();

        assert!(read_frame::<_, Msg>(&mut b).await.is_err());
        let got: Msg = read_frame(&mut b).await.unwrap();
        assert_eq!(got, msg("recovered", 42));
    }
}
