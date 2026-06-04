//! On-device model + inference-server lifecycle.
//!
//! - Downloads a quantized GGUF model on first run (streamed, SHA-256 verified).
//! - Starts a local `llama-server` (if a binary can be found) bound to 127.0.0.1.
//! - Auto-detects an already-running OpenAI-compatible server (llama-server OR Ollama),
//!   so development works against an existing local server.
//!
//! Bundling a `llama-server` binary with the app is the documented packaging step;
//! until then the app guides the user to point at / install a local server.

use anyhow::{anyhow, Result};
use futures_util::StreamExt;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use tauri::{AppHandle, Emitter, Manager};

/// A model the user can download on first run.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub filename: &'static str,
    pub url: &'static str,
    pub size_mb: u32,
    pub note: &'static str,
}

pub const MODELS: &[ModelInfo] = &[
    ModelInfo {
        id: "qwen2.5-3b-instruct-q4_k_m",
        name: "Qwen2.5 3B Instruct (recommended)",
        filename: "Qwen2.5-3B-Instruct-Q4_K_M.gguf",
        url: "https://huggingface.co/bartowski/Qwen2.5-3B-Instruct-GGUF/resolve/main/Qwen2.5-3B-Instruct-Q4_K_M.gguf",
        size_mb: 2020,
        note: "Best structured-output quality at a size most machines can run.",
    },
    ModelInfo {
        id: "qwen2.5-7b-instruct-q4_k_m",
        name: "Qwen2.5 7B Instruct (most reliable)",
        filename: "Qwen2.5-7B-Instruct-Q4_K_M.gguf",
        url: "https://huggingface.co/bartowski/Qwen2.5-7B-Instruct-GGUF/resolve/main/Qwen2.5-7B-Instruct-Q4_K_M.gguf",
        size_mb: 4680,
        note: "Best instruction-following and accuracy; needs ~6 GB RAM and is slower.",
    },
    ModelInfo {
        id: "qwen2.5-1.5b-instruct-q4_k_m",
        name: "Qwen2.5 1.5B Instruct (lite)",
        filename: "Qwen2.5-1.5B-Instruct-Q4_K_M.gguf",
        url: "https://huggingface.co/bartowski/Qwen2.5-1.5B-Instruct-GGUF/resolve/main/Qwen2.5-1.5B-Instruct-Q4_K_M.gguf",
        size_mb: 1010,
        note: "For weaker machines; lighter and faster, slightly less reliable.",
    },
];

pub fn model_info(id: &str) -> Option<&'static ModelInfo> {
    MODELS.iter().find(|m| m.id == id)
}

pub fn models_dir(app: &AppHandle) -> Result<PathBuf> {
    let dir = app.path().app_data_dir()?.join("models");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

pub fn model_file_path(app: &AppHandle, filename: &str) -> Result<PathBuf> {
    Ok(models_dir(app)?.join(filename))
}

pub fn is_model_present(app: &AppHandle, id: &str) -> bool {
    match model_info(id) {
        Some(m) => model_file_path(app, m.filename).map(|p| p.exists()).unwrap_or(false),
        None => false,
    }
}

/// First downloaded model (used as a fallback when the configured model isn't present).
pub fn first_present_model(app: &AppHandle) -> Option<&'static str> {
    MODELS.iter().find(|m| is_model_present(app, m.id)).map(|m| m.id)
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DownloadProgress {
    downloaded: u64,
    total: u64,
}

/// Stream-download a model, emitting `model-download-progress` events, verify SHA-256
/// when provided, and atomically move into place.
pub async fn download_model(app: AppHandle, client: reqwest::Client, id: String, expected_sha: String) -> Result<String> {
    let info = model_info(&id).ok_or_else(|| anyhow!("unknown model id: {id}"))?;
    let dest = model_file_path(&app, info.filename)?;
    if dest.exists() {
        return Ok(dest.display().to_string());
    }

    let resp = client.get(info.url).send().await?.error_for_status()?;
    let total = resp.content_length().unwrap_or((info.size_mb as u64) * 1_000_000);
    let tmp = dest.with_extension("part");
    let mut file = std::fs::File::create(&tmp)?;
    let mut hasher = Sha256::new();
    let mut downloaded: u64 = 0;
    let mut last_emit: u64 = 0;

    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        file.write_all(&chunk)?;
        hasher.update(&chunk);
        downloaded += chunk.len() as u64;
        // Throttle events to ~every 4 MB.
        if downloaded - last_emit > 4_000_000 {
            last_emit = downloaded;
            let _ = app.emit("model-download-progress", DownloadProgress { downloaded, total });
        }
    }
    file.flush()?;
    drop(file);

    if !expected_sha.trim().is_empty() {
        let got = hex::encode(hasher.finalize());
        if !got.eq_ignore_ascii_case(expected_sha.trim()) {
            let _ = std::fs::remove_file(&tmp);
            return Err(anyhow!("model checksum mismatch (got {got})"));
        }
    }

    std::fs::rename(&tmp, &dest)?;
    let _ = app.emit("model-download-progress", DownloadProgress { downloaded: total, total });
    Ok(dest.display().to_string())
}

fn server_dir(app: &AppHandle) -> Result<PathBuf> {
    let dir = app.path().app_data_dir()?.join("bin");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// The llama.cpp release-asset substring for this platform (None = auto-download unsupported).
fn platform_asset_substr() -> Option<&'static str> {
    if cfg!(target_os = "macos") {
        if cfg!(target_arch = "aarch64") {
            Some("bin-macos-arm64.tar.gz")
        } else {
            Some("bin-macos-x64.tar.gz")
        }
    } else {
        // Linux/Windows: archive layout differs — guide the user to install instead (for now).
        None
    }
}

/// Find the newest llama.cpp release asset for this platform ("latest" sometimes has no
/// assets yet, so we scan recent releases).
async fn find_server_asset(client: &reqwest::Client) -> Result<(String, String)> {
    let substr = platform_asset_substr()
        .ok_or_else(|| anyhow!("automatic engine download isn't supported on this OS yet — install llama.cpp or Ollama"))?;
    let releases: serde_json::Value = client
        .get("https://api.github.com/repos/ggml-org/llama.cpp/releases?per_page=15")
        .header("User-Agent", "pushin-app")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let empty = Vec::new();
    for rel in releases.as_array().unwrap_or(&empty) {
        if let Some(assets) = rel["assets"].as_array() {
            for a in assets {
                if let Some(name) = a["name"].as_str() {
                    if name.contains(substr) {
                        let url = a["browser_download_url"].as_str().unwrap_or_default().to_string();
                        if !url.is_empty() {
                            return Ok((name.to_string(), url));
                        }
                    }
                }
            }
        }
    }
    Err(anyhow!("couldn't find a prebuilt llama.cpp server for this platform"))
}

/// Ensure a runnable `llama-server` exists in the app's bin/ dir, downloading + unpacking
/// the prebuilt llama.cpp engine if needed. Emits `inference-status` updates.
pub async fn ensure_server_binary(app: &AppHandle, client: &reqwest::Client) -> Result<PathBuf> {
    let dir = server_dir(app)?;
    let target = dir.join(server_bin_name());
    if target.exists() {
        return Ok(target);
    }
    // Already on PATH (e.g. user installed llama.cpp)? Use that.
    if let Some(p) = find_in_path(server_bin_name()) {
        return Ok(p);
    }

    let _ = app.emit("inference-status", "Downloading the inference engine (~10 MB)…");
    let (name, url) = find_server_asset(client).await?;
    let archive = dir.join(&name);
    let bytes = client.get(&url).send().await?.error_for_status()?.bytes().await?;
    std::fs::write(&archive, &bytes)?;

    let _ = app.emit("inference-status", "Unpacking the inference engine…");
    let status = Command::new("tar")
        .arg("-xzf")
        .arg(&archive)
        .arg("--strip-components=1")
        .arg("-C")
        .arg(&dir)
        .status()?;
    let _ = std::fs::remove_file(&archive);
    if !status.success() || !target.exists() {
        return Err(anyhow!("failed to unpack the inference engine"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&target)?.permissions();
        perms.set_mode(0o755);
        let _ = std::fs::set_permissions(&target, perms);
    }
    Ok(target)
}

/// Find a `llama-server` binary: app-data /bin first, then $PATH.
fn resolve_server_binary(app: &AppHandle) -> Option<PathBuf> {
    if let Ok(data) = app.path().app_data_dir() {
        let candidate = data.join("bin").join(server_bin_name());
        if candidate.exists() {
            return Some(candidate);
        }
    }
    find_in_path(server_bin_name())
}

fn server_bin_name() -> &'static str {
    if cfg!(windows) {
        "llama-server.exe"
    } else {
        "llama-server"
    }
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let full = dir.join(name);
        if full.is_file() {
            return Some(full);
        }
    }
    None
}

fn port_from_url(url: &str) -> u16 {
    url.rsplit(':')
        .next()
        .and_then(|s| s.trim_end_matches('/').parse().ok())
        .unwrap_or(8080)
}

/// Spawn a local `llama-server` for `model_id` bound to the port in `base_url`.
pub fn spawn_server(app: &AppHandle, model_id: &str, base_url: &str) -> Result<Child> {
    let bin = resolve_server_binary(app)
        .ok_or_else(|| anyhow!("no `llama-server` binary found (install llama.cpp or drop the binary in the app's bin/ folder)"))?;
    let info = model_info(model_id).ok_or_else(|| anyhow!("unknown model id"))?;
    let model = model_file_path(app, info.filename)?;
    if !model.exists() {
        return Err(anyhow!("model not downloaded yet"));
    }
    let port = port_from_url(base_url);
    let child = Command::new(bin)
        .args([
            "-m",
            model.to_str().unwrap(),
            "--host",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "-c",
            "4096",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(child)
}
