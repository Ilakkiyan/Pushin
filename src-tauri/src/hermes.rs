//! Hermes — Pushin's on-device memory layer (the "second brain").
//!
//! The user gives Pushin freeform notes; Hermes stores them and recalls the relevant ones later.
//! Recall is **semantic** when an embedding backend is available (cosine similarity over on-device
//! vectors) and gracefully falls back to **keyword** scoring otherwise — so notes are useful the
//! moment you write them and "upgrade" to semantic search when embeddings are present.
//!
//! Everything here is local: embeddings are computed by the same OpenAI-compatible server Pushin
//! already talks to (`{base}/v1/embeddings`), and vectors live in SQLite as little-endian f32.

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};

/// Embed `input` via an OpenAI-compatible `/v1/embeddings` endpoint. Returns the vector, or an
/// error if the backend has no embeddings support (the caller treats that as "not indexed").
pub async fn embed_text(client: &reqwest::Client, base_url: &str, model: &str, input: &str) -> Result<Vec<f32>> {
    let base = base_url.trim_end_matches('/');
    let resp = client
        .post(format!("{base}/v1/embeddings"))
        .json(&json!({ "model": model, "input": input }))
        .send()
        .await
        .map_err(|e| anyhow!("embeddings request failed: {e}"))?
        .error_for_status()
        .map_err(|e| anyhow!("embeddings endpoint error: {e}"))?;
    let v: Value = resp.json().await?;
    let arr = v["data"][0]["embedding"]
        .as_array()
        .ok_or_else(|| anyhow!("no embedding in response (is this an embeddings-capable model?)"))?;
    let vec: Vec<f32> = arr.iter().filter_map(|x| x.as_f64().map(|f| f as f32)).collect();
    if vec.is_empty() {
        bail!("empty embedding returned");
    }
    Ok(vec)
}

/// Embed several inputs in one request (the OpenAI API accepts an array). Results are placed by the
/// response's `index` field so order matches `inputs`. Used to embed the few-shot exemplar bank once.
pub async fn embed_batch(client: &reqwest::Client, base_url: &str, model: &str, inputs: &[&str]) -> Result<Vec<Vec<f32>>> {
    let base = base_url.trim_end_matches('/');
    let resp = client
        .post(format!("{base}/v1/embeddings"))
        .json(&json!({ "model": model, "input": inputs }))
        .send()
        .await
        .map_err(|e| anyhow!("embeddings request failed: {e}"))?
        .error_for_status()
        .map_err(|e| anyhow!("embeddings endpoint error: {e}"))?;
    let v: Value = resp.json().await?;
    let data = v["data"].as_array().ok_or_else(|| anyhow!("no data in embeddings response"))?;
    let mut out: Vec<Vec<f32>> = vec![Vec::new(); inputs.len()];
    for d in data {
        let idx = d["index"].as_u64().unwrap_or(0) as usize;
        let vec: Vec<f32> = d["embedding"].as_array().map(|a| a.iter().filter_map(|x| x.as_f64().map(|f| f as f32)).collect()).unwrap_or_default();
        if idx < out.len() {
            out[idx] = vec;
        }
    }
    if out.iter().any(|v| v.is_empty()) {
        bail!("batch embedding returned an empty vector");
    }
    Ok(out)
}

/// Pack an f32 vector into a little-endian byte blob for SQLite storage.
pub fn vec_to_blob(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for f in v {
        out.extend_from_slice(&f.to_le_bytes());
    }
    out
}

/// Unpack a little-endian f32 blob. Trailing bytes that don't form a full f32 are ignored.
pub fn blob_to_vec(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
}

/// Cosine similarity in [-1, 1] (0 for mismatched/empty/degenerate inputs).
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let (mut dot, mut na, mut nb) = (0f32, 0f32, 0f32);
    for i in 0..a.len() {
        dot += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na.sqrt() * nb.sqrt())
    }
}

/// Fraction of distinct query terms (≥2 chars) that appear in `content`, in [0, 1]. The keyword
/// fallback for recall when no embedding is available.
pub fn keyword_score(content: &str, query: &str) -> f32 {
    let hay = content.to_lowercase();
    let terms: Vec<String> = {
        let mut seen = std::collections::HashSet::new();
        query
            .to_lowercase()
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|t| t.len() >= 2)
            .filter(|t| seen.insert(t.to_string()))
            .map(str::to_string)
            .collect()
    };
    if terms.is_empty() {
        return 0.0;
    }
    let hits = terms.iter().filter(|t| hay.contains(t.as_str())).count();
    hits as f32 / terms.len() as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blob_round_trips() {
        let v = vec![0.0f32, 1.5, -2.25, 3.125];
        assert_eq!(blob_to_vec(&vec_to_blob(&v)), v);
        // A stray trailing byte is ignored, not a panic.
        let mut bytes = vec_to_blob(&v);
        bytes.push(0xAB);
        assert_eq!(blob_to_vec(&bytes), v);
    }

    #[test]
    fn cosine_basics() {
        let a = [1.0, 0.0, 0.0];
        assert!((cosine(&a, &a) - 1.0).abs() < 1e-6); // identical
        assert!(cosine(&a, &[0.0, 1.0, 0.0]).abs() < 1e-6); // orthogonal
        assert!((cosine(&a, &[2.0, 0.0, 0.0]) - 1.0).abs() < 1e-6); // scale-invariant
        assert_eq!(cosine(&a, &[1.0, 0.0]), 0.0); // length mismatch → 0
        assert_eq!(cosine(&[], &[]), 0.0);
        assert!(cosine(&[1.0, 1.0], &[-1.0, -1.0]) < 0.0); // opposite → negative
    }

    #[test]
    fn keyword_scoring() {
        assert_eq!(keyword_score("Met Sarah about the Q3 budget", "sarah budget"), 1.0);
        assert_eq!(keyword_score("Met Sarah about the Q3 budget", "sarah taxes"), 0.5);
        assert_eq!(keyword_score("anything", "zebra koala"), 0.0);
        assert_eq!(keyword_score("anything", "a i"), 0.0); // sub-2-char terms ignored → no terms
        // Case-insensitive and dedups repeated terms.
        assert_eq!(keyword_score("GYM on tuesday", "gym gym"), 1.0);
    }

    #[test]
    fn ranks_by_similarity() {
        // A tiny semantic-style ranking sanity check using hand-made vectors.
        let q = [1.0f32, 0.0];
        let near = [0.9f32, 0.1];
        let far = [0.0f32, 1.0];
        assert!(cosine(&q, &near) > cosine(&q, &far));
    }
}
