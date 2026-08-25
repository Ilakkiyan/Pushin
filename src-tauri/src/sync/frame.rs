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
