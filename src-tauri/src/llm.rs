//! Client for the local, OpenAI-compatible inference server (llama.cpp `llama-server`).
//! Everything here stays on `127.0.0.1` — no data leaves the device.

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

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

/// Run a chat completion constrained to `schema` and return the parsed JSON object.
/// `schema` is a JSON Schema describing the expected response shape.
pub async fn chat_json(
    client: &reqwest::Client,
    base_url: &str,
    model: &str,
    messages: Value,
    schema: Value,
) -> Result<Value> {
    let base = base_url.trim_end_matches('/');
    let body = json!({
        "model": model,
        "temperature": 0.1,
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

    // Retry once: degeneration is stochastic, so a second sample usually succeeds.
    let mut last_err = None;
    for _ in 0..2 {
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
