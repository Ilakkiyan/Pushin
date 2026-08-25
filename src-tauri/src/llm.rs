//! Client for the local, OpenAI-compatible inference server (llama.cpp `llama-server`).
//! Everything here stays on `127.0.0.1` — no data leaves the device, EXCEPT the mobile→desktop
//! bridge: a device with no local model (a phone) can register a `PeerChat` fallback that routes a
//! completion to a paired desktop over the encrypted Iroh mesh (see `sync::infer`). Desktops keep a
//! reachable local server, so they never hit the fallback.

use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;

/// A registered "borrow a peer's model" callback: `(messages, schema) -> completion JSON`. Set by the
/// sync engine at startup; used only when the local server is unreachable.
pub type PeerChat = Box<dyn Fn(Value, Value) -> Pin<Box<dyn Future<Output = Result<Value>> + Send>> + Send + Sync>;
static PEER_CHAT: Mutex<Option<PeerChat>> = Mutex::new(None);

/// Register (or replace) the peer-inference fallback. The engine calls this on start/restart.
pub fn register_peer_chat(f: PeerChat) {
    *PEER_CHAT.lock().unwrap() = Some(f);
}

/// Is a local inference server reachable at `base_url`?
pub async fn health(client: &reqwest::Client, base_url: &str) -> bool {
    let base = base_url.trim_end_matches('/');
    for path in ["/health", "/v1/models"] {
        if let Ok(resp) = client.get(format!("{base}{path}")).send().await {
            if resp.status().is_success() {
                return true;
            }
        }
    }
    false
}

/// Fire a tiny, best-effort completion to warm a freshly-spawned server: this allocates the KV cache
/// and pays the one-time CUDA PTX-JIT now, so the user's FIRST real prompt doesn't eat cold-start cost.
/// Errors are ignored — a cold model still works, it's just slow on the first request. Intended to be
/// run detached (off the "AI ready" critical path) right after `health` first returns true.
pub async fn warmup(client: &reqwest::Client, base_url: &str, model: &str) {
    let base = base_url.trim_end_matches('/');
    let body = json!({
        "model": model,
        "max_tokens": 1,
        "temperature": 0.0,
        // Don't retain this throwaway prompt in the server's prompt cache — it must not shadow the
        // real system prompt that follows.
        "cache_prompt": false,
        "messages": [{ "role": "user", "content": "hi" }],
    });
    let _ = client.post(format!("{base}/v1/chat/completions")).json(&body).send().await;
}

/// Run a chat completion constrained to `schema` and return the parsed JSON object.
/// `schema` is a JSON Schema describing the expected response shape.
pub async fn chat_json(
    client: &reqwest::Client,
    base_url: &str,
    model: &str,
    messages: Value,
    schema: Value,
) -> Result<Value> {
    // Mobile bridge: if there's no local server, borrow a paired desktop's model over the mesh. Desktops
    // have a reachable local server so they skip this and run locally (PC-side inference is mobile-only).
    if !health(client, base_url).await {
        let fut = {
            let guard = PEER_CHAT.lock().unwrap();
            guard.as_ref().map(|f| f(messages.clone(), schema.clone()))
        };
        if let Some(fut) = fut {
            return fut.await;
        }
    }

    let base = base_url.trim_end_matches('/');
    let mut body = json!({
        "model": model,
        "temperature": 0.0,
        "max_tokens": 1536,
        // Anti-degeneration: stop the small model from looping a phrase until it overruns
        // the token budget (which truncates the JSON). repeat_penalty is llama.cpp-native;
        // frequency_penalty also works on Ollama. Unknown fields are ignored by servers.
        "repeat_penalty": 1.2,
        "frequency_penalty": 0.4,
        "cache_prompt": true,
        "messages": messages,
        // llama.cpp honors json_schema and constrains decoding to valid JSON.
        "response_format": {
            "type": "json_schema",
            "json_schema": { "name": "plan", "strict": true, "schema": schema }
        }
    });

    // First pass is greedy (temperature 0): the argmax decode is both the most accurate for a
    // constrained extraction and deterministic run-to-run (no more "flips between runs"). If it
    // degenerates/fails, retry with temperature — at 0 the retry would just reproduce the same
    // bad sample, so we escalate to break the loop.
    let mut last_err = None;
    for temp in [0.0_f64, 0.4] {
        body["temperature"] = json!(temp);
        match try_once(client, base, &body).await {
            Ok(v) => return Ok(v),
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow!("inference failed")))
}

async fn try_once(client: &reqwest::Client, base: &str, body: &Value) -> Result<Value> {
    let resp = client
        .post(format!("{base}/v1/chat/completions"))
        .json(body)
        .send()
        .await
        .map_err(|e| anyhow!("inference server unreachable: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(anyhow!("inference error {status}: {}", truncate(&text, 200)));
    }

    let v: Value = resp.json().await?;
    let content = v["choices"][0]["message"]["content"]
        .as_str()
        .ok_or_else(|| anyhow!("no content in completion"))?;

    parse_json_lenient(content)
}

/// Free-form chat completion — the "deharnessed" mode: NO json_schema, warmer sampling, returns the
/// raw assistant prose. Powers the general-purpose assistant (not the constrained calendar planner,
/// which stays on `chat_json`). Same local server, same model — just an unconstrained request.
pub async fn chat_text(
    client: &reqwest::Client,
    base_url: &str,
    model: &str,
    messages: Value,
) -> Result<String> {
    let base = base_url.trim_end_matches('/');
    let body = json!({
        "model": model,
        "temperature": 0.7, // warmer than the planner's greedy 0.0 — prose, not extraction
        "max_tokens": 2048,
        "repeat_penalty": 1.1,
        "cache_prompt": true,
        "messages": messages,
    });
    let resp = client
        .post(format!("{base}/v1/chat/completions"))
        .json(&body)
        .send()
        .await
        .map_err(|e| anyhow!("inference server unreachable: {e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(anyhow!("inference error {status}: {}", truncate(&text, 200)));
    }
    let v: Value = resp.json().await?;
    let content = v["choices"][0]["message"]["content"]
        .as_str()
        .ok_or_else(|| anyhow!("no content in completion"))?;
    Ok(content.trim().to_string())
}

fn truncate(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max).collect();
        format!("{head}…")
    }
}

/// Parse model output as JSON, tolerating leading/trailing prose or code fences.
fn parse_json_lenient(s: &str) -> Result<Value> {
    if let Ok(v) = serde_json::from_str::<Value>(s) {
        return Ok(v);
    }
    let cleaned = s.trim().trim_start_matches("```json").trim_start_matches("```").trim_end_matches("```");
    if let Ok(v) = serde_json::from_str::<Value>(cleaned.trim()) {
        return Ok(v);
    }
    if let (Some(start), Some(end)) = (s.find('{'), s.rfind('}')) {
        if end > start {
            if let Ok(v) = serde_json::from_str::<Value>(&s[start..=end]) {
                return Ok(v);
            }
        }
    }
    Err(anyhow!("the AI returned a malformed plan — try rephrasing, or use the larger model. (got: {})", truncate(s, 160)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;

    #[test]
    fn parse_json_lenient_handles_clean_fenced_and_wrapped() {
        assert_eq!(parse_json_lenient("{\"a\":1}").unwrap()["a"], 1);
        assert_eq!(parse_json_lenient("```json\n{\"a\":2}\n```").unwrap()["a"], 2);
        assert_eq!(parse_json_lenient("sure, here: {\"a\":3} hope that helps").unwrap()["a"], 3);
        assert!(parse_json_lenient("not json at all").is_err());
    }

    #[test]
    fn truncate_caps_by_chars_with_ellipsis() {
        assert_eq!(truncate("  hi  ", 10), "hi");
        let t = truncate(&"x".repeat(50), 10);
        assert!(t.ends_with('…') && t.chars().count() == 11);
    }

    fn completion(content: &str) -> serde_json::Value {
        json!({ "choices": [ { "message": { "content": content } } ] })
    }

    #[tokio::test]
    async fn chat_json_returns_parsed_content() {
        let server = MockServer::start_async().await;
        let m = server.mock(|when, then| {
            when.method(POST).path("/v1/chat/completions");
            then.status(200).json_body(completion("{\"ok\":true}"));
        });
        let v = chat_json(&reqwest::Client::new(), &server.base_url(), "m", json!([]), json!({})).await.unwrap();
        assert_eq!(v["ok"], true);
        m.assert();
    }

    #[tokio::test]
    async fn chat_json_retries_once_on_bad_json_then_succeeds() {
        let server = MockServer::start_async().await;
        // Both attempts hit the same path; first returns garbage, second returns valid JSON.
        let _bad = server.mock(|when, then| {
            when.method(POST).path("/v1/chat/completions").json_body_partial(r#"{"temperature":0.0}"#);
            then.status(200).json_body(completion("totally not json"));
        });
        let good = server.mock(|when, then| {
            when.method(POST).path("/v1/chat/completions").json_body_partial(r#"{"temperature":0.4}"#);
            then.status(200).json_body(completion("{\"recovered\":1}"));
        });
        let v = chat_json(&reqwest::Client::new(), &server.base_url(), "m", json!([]), json!({})).await.unwrap();
        assert_eq!(v["recovered"], 1);
        good.assert();
    }

    #[tokio::test]
    async fn chat_json_surfaces_http_errors() {
        let server = MockServer::start_async().await;
        server.mock(|when, then| {
            when.method(POST).path("/v1/chat/completions");
            then.status(500).body("boom");
        });
        assert!(chat_json(&reqwest::Client::new(), &server.base_url(), "m", json!([]), json!({})).await.is_err());
    }

    #[tokio::test]
    async fn chat_text_returns_trimmed_prose() {
        let server = MockServer::start_async().await;
        let m = server.mock(|when, then| {
            when.method(POST).path("/v1/chat/completions");
            then.status(200).json_body(completion("  Sure — here's a thought.  "));
        });
        let s = chat_text(&reqwest::Client::new(), &server.base_url(), "m", json!([])).await.unwrap();
        assert_eq!(s, "Sure — here's a thought.");
        m.assert();
    }

    #[tokio::test]
    async fn chat_text_surfaces_http_errors() {
        let server = MockServer::start_async().await;
        server.mock(|when, then| {
            when.method(POST).path("/v1/chat/completions");
            then.status(500).body("boom");
        });
        assert!(chat_text(&reqwest::Client::new(), &server.base_url(), "m", json!([])).await.is_err());
    }

    #[tokio::test]
    async fn health_true_on_200_false_when_unreachable() {
        let server = MockServer::start_async().await;
        server.mock(|when, then| {
            when.method(GET).path("/health");
            then.status(200).body("ok");
        });
        assert!(health(&reqwest::Client::new(), &server.base_url()).await);
        // A port with nothing listening → unreachable → false.
        assert!(!health(&reqwest::Client::new(), "http://127.0.0.1:1").await);
    }
}
