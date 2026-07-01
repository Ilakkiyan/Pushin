//! On-device model + inference-server lifecycle.
//!
//! - Downloads a quantized GGUF model on first run (streamed, SHA-256 verified).
//! - Auto-downloads the prebuilt llama.cpp `llama-server` engine for the current
//!   platform (macOS arm64/x64, Linux x64/arm64, Windows x64/arm64) and unpacks it
//!   into the app's `bin/` dir, then spawns it bound to 127.0.0.1.
//! - Auto-detects an already-running OpenAI-compatible server (llama-server OR Ollama),
//!   so development works against an existing local server.
//!
//! macOS/Linux releases ship `.tar.gz` (unpacked via the system `tar`); Windows ships
//! `.zip` (unpacked via the pure-Rust `zip` crate, since GNU `tar` can't read zip).

use anyhow::{anyhow, Result};
use futures_util::StreamExt;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::{Path, PathBuf};
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
        name: "Qwen2.5 3B Instruct (lite)",
        filename: "Qwen2.5-3B-Instruct-Q4_K_M.gguf",
        url: "https://huggingface.co/bartowski/Qwen2.5-3B-Instruct-GGUF/resolve/main/Qwen2.5-3B-Instruct-Q4_K_M.gguf",
        size_mb: 2020,
        note: "Lightest download; fast and runs on most machines. Less reliable on multi-step edits and dates.",
    },
    ModelInfo {
        id: "qwen2.5-7b-instruct-q4_k_m",
        name: "Qwen2.5 7B Instruct (recommended)",
        filename: "Qwen2.5-7B-Instruct-Q4_K_M.gguf",
        url: "https://huggingface.co/bartowski/Qwen2.5-7B-Instruct-GGUF/resolve/main/Qwen2.5-7B-Instruct-Q4_K_M.gguf",
        size_mb: 4680,
        note: "Recommended — the most reliable multi-step parsing; needs ~6 GB RAM and is a bit slower.",
    },
    ModelInfo {
        id: "qwen2.5-14b-instruct-q4_k_m",
        name: "Qwen2.5 14B Instruct (most powerful)",
        filename: "Qwen2.5-14B-Instruct-Q4_K_M.gguf",
        url: "https://huggingface.co/bartowski/Qwen2.5-14B-Instruct-GGUF/resolve/main/Qwen2.5-14B-Instruct-Q4_K_M.gguf",
        size_mb: 8990,
        note: "Highest accuracy for a strong machine; needs ~12 GB RAM and is the slowest.",
    },
];

/// The dedicated embedding model for Hermes (semantic memory recall). Auto-downloaded and served
/// by a SECOND `llama-server` instance in `--embeddings` mode, so semantic recall works with zero
/// setup — no Ollama, no manual steps. BERT-class and tiny (384-dim, ~37 MB) so the extra download
/// and RAM are negligible next to the chat model. Not in `MODELS` so it never shows in the picker.
pub const EMBED_MODEL: ModelInfo = ModelInfo {
    id: "bge-small-en-v1.5-q8_0",
    name: "BGE Small EN v1.5 (embeddings)",
    filename: "bge-small-en-v1.5-q8_0.gguf",
    url: "https://huggingface.co/CompendiumLabs/bge-small-en-v1.5-gguf/resolve/main/bge-small-en-v1.5-q8_0.gguf",
    size_mb: 37,
    note: "On-device embeddings for Hermes memory recall.",
};

/// Port for Pushin's managed embeddings server (the chat server is on 8080).
pub const EMBED_PORT: u16 = 8181;

/// Base URL of Pushin's managed embeddings server.
pub fn embed_base_url() -> String {
    format!("http://127.0.0.1:{EMBED_PORT}")
}

/// Look up a model by id — chat models (the picker list) plus the hidden embedding model, so
/// download/spawn/presence checks all work for the embedder too.
pub fn model_info(id: &str) -> Option<&'static ModelInfo> {
    MODELS.iter().find(|m| m.id == id).or_else(|| (EMBED_MODEL.id == id).then_some(&EMBED_MODEL))
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

/// Ask HuggingFace for a model file's sha256 without downloading it: a HEAD on the resolve URL
/// returns `X-Linked-ETag` (the LFS object's sha256) on the pre-redirect response. We must NOT
/// follow the redirect, or we'd read the CDN's etag instead. Fails closed.
async fn fetch_hf_sha256(url: &str) -> Result<String> {
    let head_client = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build()?;
    let resp = head_client.head(url).header("User-Agent", "pushin-app").send().await?;
    resp.headers()
        .get("x-linked-etag")
        .or_else(|| resp.headers().get("etag"))
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().trim_matches('"').to_string())
        .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or_else(|| anyhow!("couldn't get a sha256 checksum for this model from HuggingFace — refusing to download it unverified"))
}

/// Stream-download a model, emitting `model-download-progress` events, verify its SHA-256
/// (always — fetched from HuggingFace if not supplied), and atomically move into place.
pub async fn download_model(app: AppHandle, client: reqwest::Client, id: String, expected_sha: String) -> Result<String> {
    let info = model_info(&id).ok_or_else(|| anyhow!("unknown model id: {id}"))?;
    let dest = model_file_path(&app, info.filename)?;
    if dest.exists() {
        return Ok(dest.display().to_string());
    }

    // Resolve the expected checksum first and fail closed if we can't get one — never write an
    // unverified model. A caller-supplied hash wins; otherwise ask HuggingFace for the file's sha256.
    let expected = if expected_sha.trim().is_empty() {
        fetch_hf_sha256(info.url).await?
    } else {
        expected_sha.trim().to_string()
    };

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

    let got = hex::encode(hasher.finalize());
    if !got.eq_ignore_ascii_case(&expected) {
        let _ = std::fs::remove_file(&tmp);
        return Err(anyhow!("model checksum mismatch — refusing a tampered download (wanted {expected}, got {got})"));
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

/// llama.cpp release-asset name substrings for this platform, in preference order
/// (None = auto-download unsupported). We pick the plain **CPU** build everywhere:
/// it's GPU-agnostic and self-contained, so it runs on any machine. Users who want
/// GPU acceleration can drop a CUDA/Vulkan/Metal build into the app's `bin/` dir.
///
/// Substrings are extension-less on purpose: llama.cpp has changed asset extensions
/// over time (Linux moved `.zip` → `.tar.gz`), so we match the OS/arch portion and let
/// `ensure_server_binary` pick the unpacker from the actual downloaded filename. Each is
/// specific enough to match only the CPU build (e.g. `bin-ubuntu-x64` won't match
/// `bin-ubuntu-vulkan-x64`, `bin-win-cpu-x64` won't match `bin-win-cuda-…-x64`).
fn platform_asset_candidates() -> Option<&'static [&'static str]> {
    let arm = cfg!(target_arch = "aarch64");
    if cfg!(target_os = "macos") {
        Some(if arm { &["bin-macos-arm64"] } else { &["bin-macos-x64"] })
    } else if cfg!(target_os = "windows") {
        // arm64 asset naming has churned ("win-cpu-arm64" vs "win-arm64"); try both.
        Some(if arm { &["bin-win-cpu-arm64", "bin-win-arm64"] } else { &["bin-win-cpu-x64"] })
    } else if cfg!(target_os = "linux") {
        Some(if arm { &["bin-ubuntu-arm64"] } else { &["bin-ubuntu-x64"] })
    } else {
        None
    }
}

/// Verify downloaded bytes against a GitHub asset digest of the form `sha256:<hex>`.
/// Fails closed: a missing/non-sha256 digest or any mismatch is an error, so an unverified
/// or tampered binary is never written to disk, unpacked, or executed.
fn verify_sha256(bytes: &[u8], digest: &str) -> Result<()> {
    let want = digest
        .strip_prefix("sha256:")
        .ok_or_else(|| anyhow!("inference engine asset has no sha256 checksum — refusing to run an unverified binary"))?
        .trim();
    let got = hex::encode(Sha256::digest(bytes));
    if got.eq_ignore_ascii_case(want) {
        Ok(())
    } else {
        Err(anyhow!("inference engine checksum mismatch — refusing to run a tampered binary (wanted {want}, got {got})"))
    }
}

/// One engine download: the server binaries + (for a CUDA build) the matching cudart runtime DLLs,
/// from the same llama.cpp release.
struct EngineDownload {
    /// (asset name, download url, "sha256:<hex>" digest)
    binaries: (String, String, String),
    cudart: Option<(String, String, String)>,
}

/// True if we should prefer a CUDA engine build — i.e. an NVIDIA GPU is present. Best-effort: just
/// check whether `nvidia-smi` runs. (macOS uses Metal, which the macos build already has, so skip it.)
fn prefer_cuda() -> bool {
    if cfg!(target_os = "macos") {
        return false;
    }
    let mut cmd = Command::new("nvidia-smi");
    cmd.arg("-L").stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd.status().map(|s| s.success()).unwrap_or(false)
}

/// A hardware-based model recommendation for the setup card.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRecommendation {
    pub model_id: String,
    pub reason: String,
    pub ram_gb: Option<f64>,
    pub has_gpu: bool,
}

/// Best-effort total physical RAM in GB — dep-free, shelling out per-OS like the GPU probe (keeps the
/// build lean + cross-compile-safe). `None` when undeterminable (or on mobile, which is companion-only).
fn total_ram_gb() -> Option<f64> {
    #[cfg(target_os = "linux")]
    {
        let s = std::fs::read_to_string("/proc/meminfo").ok()?;
        let line = s.lines().find(|l| l.starts_with("MemTotal:"))?;
        let kb: f64 = line.split_whitespace().nth(1)?.parse().ok()?;
        return Some(kb / 1024.0 / 1024.0);
    }
    #[cfg(target_os = "macos")]
    {
        let out = Command::new("sysctl").args(["-n", "hw.memsize"]).output().ok()?;
        let bytes: f64 = String::from_utf8_lossy(&out.stdout).trim().parse().ok()?;
        return Some(bytes / 1_073_741_824.0);
    }
    #[cfg(target_os = "windows")]
    {
        let mut cmd = Command::new("powershell");
        cmd.args(["-NoProfile", "-Command", "(Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory"]);
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        let out = cmd.output().ok()?;
        let bytes: f64 = String::from_utf8_lossy(&out.stdout).trim().parse().ok()?;
        return Some(bytes / 1_073_741_824.0);
    }
    #[allow(unreachable_code)]
    None
}

/// Recommend a model for this machine from its RAM (and whether a CUDA GPU is present) — the model
/// must fit comfortably in RAM. Falls back to the balanced 7B when RAM can't be read.
pub fn recommend_model() -> ModelRecommendation {
    let ram = total_ram_gb();
    let gpu = prefer_cuda();
    let (model, why): (&ModelInfo, String) = match ram {
        Some(g) if g >= 15.0 => (&MODELS[2], format!("{g:.0} GB RAM comfortably fits the 14B")),
        Some(g) if g >= 8.0 => (&MODELS[1], format!("{g:.0} GB RAM is a good match for the 7B")),
        Some(g) => (&MODELS[0], format!("{g:.0} GB RAM — the light 3B is the safe pick")),
        None => (&MODELS[1], "a balanced default".to_string()),
    };
    let reason = if gpu { format!("{why}, plus a GPU for acceleration") } else { why };
    ModelRecommendation { model_id: model.id.to_string(), reason, ram_gb: ram, has_gpu: gpu }
}

/// CUDA release-asset substrings for this platform (None where there's no prebuilt CUDA engine).
/// cu12.4 over cu13 on purpose: it runs on more drivers and handles Blackwell (sm_120) via PTX JIT,
/// whereas cu13 needs a very recent driver (>=580). Linux CUDA prebuilt naming is unsettled → CPU there.
fn cuda_asset_candidates() -> Option<&'static [&'static str]> {
    if cfg!(target_os = "windows") && !cfg!(target_arch = "aarch64") {
        Some(&["bin-win-cuda-12.4-x64"])
    } else {
        None
    }
}

/// Extract (name, url, "sha256:" digest) from a release asset, but only for a verifiable archive.
fn asset_tuple(a: &serde_json::Value) -> Option<(String, String, String)> {
    let name = a["name"].as_str()?;
    let is_archive = name.ends_with(".zip") || name.ends_with(".tar.gz");
    let url = a["browser_download_url"].as_str().unwrap_or_default();
    let digest = a["digest"].as_str().unwrap_or_default();
    // Never run an unverified binary: require GitHub's published sha256 digest.
    if is_archive && !url.is_empty() && digest.starts_with("sha256:") {
        Some((name.to_string(), url.to_string(), digest.to_string()))
    } else {
        None
    }
}

/// Find the newest llama.cpp release engine for this platform. With `want_cuda`, prefer a CUDA build
/// (binaries + matching cudart, both verifiable, from one release); otherwise — or if no CUDA build is
/// found — return the CPU build. ("latest" sometimes has no assets, so scan recent releases.)
async fn find_engine_download(client: &reqwest::Client, want_cuda: bool) -> Result<EngineDownload> {
    let releases: serde_json::Value = client
        .get("https://api.github.com/repos/ggml-org/llama.cpp/releases?per_page=15")
        .header("User-Agent", "pushin-app")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let empty = Vec::new();
    let rels = releases.as_array().unwrap_or(&empty);

    // 1) CUDA: need BOTH the binaries (`llama-…-bin-…-cuda-…`) and cudart (`cudart-…`) from one release.
    if want_cuda {
        if let Some(cands) = cuda_asset_candidates() {
            for rel in rels {
                let Some(assets) = rel["assets"].as_array() else { continue };
                for cand in cands {
                    let has = |pred: &dyn Fn(&str) -> bool| {
                        assets.iter().find(|a| a["name"].as_str().map(pred).unwrap_or(false)).and_then(asset_tuple)
                    };
                    let binaries = has(&|n| n.contains(cand) && !n.starts_with("cudart"));
                    let cudart = has(&|n| n.contains(cand) && n.starts_with("cudart"));
                    if let (Some(b), Some(c)) = (binaries, cudart) {
                        return Ok(EngineDownload { binaries: b, cudart: Some(c) });
                    }
                }
            }
        }
        // No CUDA build in recent releases — fall through to the CPU build.
    }

    // 2) CPU build.
    let cands = platform_asset_candidates()
        .ok_or_else(|| anyhow!("automatic engine download isn't supported on this OS yet — install llama.cpp or Ollama"))?;
    for rel in rels {
        let Some(assets) = rel["assets"].as_array() else { continue };
        for cand in cands {
            if let Some(b) = assets.iter().find(|a| a["name"].as_str().map(|n| n.contains(cand)).unwrap_or(false)).and_then(asset_tuple) {
                return Ok(EngineDownload { binaries: b, cudart: None });
            }
        }
    }
    Err(anyhow!("couldn't find a prebuilt llama.cpp server for this platform"))
}

/// Download one verified asset, unpack it, and flatten its files into `dir` (bin/).
async fn download_and_unpack(client: &reqwest::Client, dir: &Path, asset: &(String, String, String)) -> Result<()> {
    let (name, url, digest) = asset;
    let archive = dir.join(name);
    let bytes = client.get(url).send().await?.error_for_status()?.bytes().await?;
    // Integrity gate: verify against GitHub's checksum BEFORE writing/unpacking/spawning.
    verify_sha256(&bytes, digest)?;
    std::fs::write(&archive, &bytes)?;

    let staging = dir.join("_unpack");
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;
    let unpacked = if name.ends_with(".zip") {
        extract_zip(&archive, &staging)
    } else {
        extract_tar_gz(&archive, &staging)
    };
    let _ = std::fs::remove_file(&archive);
    if let Err(e) = unpacked.and_then(|()| flatten_files_into(&staging, dir)) {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e);
    }
    let _ = std::fs::remove_dir_all(&staging);
    Ok(())
}

/// Download + unpack the engine into `dir` — a CUDA build (binaries + cudart) when `want_cuda`, else CPU.
async fn download_engine(app: &AppHandle, client: &reqwest::Client, dir: &Path, want_cuda: bool) -> Result<()> {
    let dl = find_engine_download(client, want_cuda).await?;
    let _ = app.emit(
        "inference-status",
        if dl.cudart.is_some() { "Downloading the GPU inference engine (~600 MB)…" } else { "Downloading the inference engine…" },
    );
    download_and_unpack(client, dir, &dl.binaries).await?;
    if let Some(cudart) = &dl.cudart {
        let _ = app.emit("inference-status", "Downloading the GPU runtime…");
        download_and_unpack(client, dir, cudart).await?;
    }
    Ok(())
}

/// Ensure a runnable `llama-server` exists in the app's bin/ dir, downloading + unpacking the prebuilt
/// llama.cpp engine if needed — a CUDA build when an NVIDIA GPU is present, else CPU. Emits
/// `inference-status` updates.
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

    // Prefer a CUDA build when a GPU is present, but NEVER let a GPU hiccup leave the user without an
    // engine: any failure falls back to the CPU build (worst case is CPU, never a broken install).
    let want_cuda = prefer_cuda();
    if let Err(gpu_err) = download_engine(app, client, &dir, want_cuda).await {
        if want_cuda {
            let _ = app.emit("inference-status", "GPU engine unavailable — using the CPU build…");
            let _ = std::fs::remove_dir_all(dir.join("_unpack"));
            download_engine(app, client, &dir, false)
                .await
                .map_err(|cpu_err| anyhow!("engine download failed (gpu: {gpu_err}; cpu: {cpu_err})"))?;
        } else {
            return Err(gpu_err);
        }
    }

    if !target.exists() {
        return Err(anyhow!("engine archive didn't contain {}", server_bin_name()));
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

/// Extract a `.tar.gz` via the system `tar` (present on macOS/Linux/Win10+).
fn extract_tar_gz(archive: &Path, dest: &Path) -> Result<()> {
    let status = Command::new("tar")
        .arg("-xzf")
        .arg(archive)
        .arg("-C")
        .arg(dest)
        .status()?;
    if !status.success() {
        return Err(anyhow!("`tar` failed to unpack the inference engine"));
    }
    Ok(())
}

/// Extract a `.zip` in-process (no reliance on `unzip`/PowerShell being installed),
/// preserving unix exec bits so the engine binary stays runnable.
fn extract_zip(archive: &Path, dest: &Path) -> Result<()> {
    let file = std::fs::File::open(archive)?;
    let mut zip = zip::ZipArchive::new(file)?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        // `enclosed_name` returns None for unsafe (zip-slip) paths, which we skip.
        let out = match entry.enclosed_name() {
            Some(p) => dest.join(p),
            None => continue,
        };
        if entry.is_dir() {
            std::fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut writer = std::fs::File::create(&out)?;
        std::io::copy(&mut entry, &mut writer)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Some(mode) = entry.unix_mode() {
                let _ = std::fs::set_permissions(&out, std::fs::Permissions::from_mode(mode));
            }
        }
    }
    Ok(())
}

/// Move every file found anywhere under `src` up into `dest` (flattening directories).
/// The engine ships its binary alongside its shared libs, so co-locating them all in
/// `bin/` keeps the loader happy regardless of how the archive nested them.
fn flatten_files_into(src: &Path, dest: &Path) -> Result<()> {
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            flatten_files_into(&path, dest)?;
        } else {
            let to = dest.join(entry.file_name());
            let _ = std::fs::remove_file(&to);
            // `rename` is cheap within the same dir tree; fall back to copy across devices.
            if std::fs::rename(&path, &to).is_err() {
                std::fs::copy(&path, &to)?;
            }
        }
    }
    Ok(())
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

/// True if a CUDA/HIP/Metal GPU backend lib sits next to the engine binary (e.g. `ggml-cuda.dll`,
/// `libggml-cuda.so`). When present we offload to the GPU; a CPU-only build won't have it.
fn has_gpu_backend(bin_dir: &std::path::Path) -> bool {
    std::fs::read_dir(bin_dir)
        .map(|rd| {
            rd.flatten().any(|e| {
                let n = e.file_name().to_string_lossy().to_lowercase();
                n.contains("ggml-cuda") || n.contains("ggml-hip") || n.contains("ggml-metal")
            })
        })
        .unwrap_or(false)
}

fn port_from_url(url: &str) -> u16 {
    url.rsplit(':')
        .next()
        .and_then(|s| s.trim_end_matches('/').parse().ok())
        .unwrap_or(8080)
}

/// Spawn a local `llama-server` for `model_id` (chat) bound to the port in `base_url`.
pub fn spawn_server(app: &AppHandle, model_id: &str, base_url: &str) -> Result<Child> {
    let info = model_info(model_id).ok_or_else(|| anyhow!("unknown model id"))?;
    let model = model_file_path(app, info.filename)?;
    if !model.exists() {
        return Err(anyhow!("model not downloaded yet"));
    }
    spawn_llama(app, &model, port_from_url(base_url), false)
}

/// Spawn the SECOND `llama-server` in embeddings mode (Hermes memory) on `EMBED_PORT`.
pub fn spawn_embed_server(app: &AppHandle) -> Result<Child> {
    let model = model_file_path(app, EMBED_MODEL.filename)?;
    if !model.exists() {
        return Err(anyhow!("embedding model not downloaded yet"));
    }
    spawn_llama(app, &model, EMBED_PORT, true)
}

/// Shared `llama-server` launcher. `embeddings` enables the `/v1/embeddings` endpoint (and a
/// smaller context) for the memory server; chat servers leave it off.
fn spawn_llama(app: &AppHandle, model: &std::path::Path, port: u16, embeddings: bool) -> Result<Child> {
    let bin = resolve_server_binary(app)
        .ok_or_else(|| anyhow!("no `llama-server` binary found (install llama.cpp or drop the binary in the app's bin/ folder)"))?;
    let model_s = model.to_str().ok_or_else(|| anyhow!("model path is not valid UTF-8"))?;
    let port_s = port.to_string();
    let ctx = if embeddings { "512" } else { "4096" };

    let mut cmd = Command::new(&bin);
    cmd.args(["-m", model_s, "--host", "127.0.0.1", "--port", &port_s, "-c", ctx]);
    // GPU offload: when a CUDA/GPU build is in bin/ (a `ggml-cuda`/`ggml-metal` lib alongside the
    // binary), push all layers onto the GPU — ~10x faster than CPU on a discrete card. Harmless with a
    // CPU-only engine (it just ignores -ngl). On a fresh CUDA server the first request pays a one-time
    // PTX-JIT warmup (Blackwell/sm_120 on an older driver), then runs at full speed.
    if bin.parent().map(has_gpu_backend).unwrap_or(false) {
        cmd.args(["-ngl", "99"]);
    }
    if embeddings {
        cmd.arg("--embeddings");
    }
    cmd.stdout(Stdio::null()).stderr(Stdio::null());

    // The engine's shared libs sit next to the binary. macOS/Windows resolve those
    // automatically (@loader_path rpath / the exe's own dir); Linux only does so if the
    // build baked in an $ORIGIN rpath, so point the loader at the binary's dir to be safe.
    #[cfg(target_os = "linux")]
    if let Some(lib_dir) = bin.parent() {
        let mut ld = lib_dir.as_os_str().to_os_string();
        if let Some(existing) = std::env::var_os("LD_LIBRARY_PATH") {
            ld.push(":");
            ld.push(existing);
        }
        cmd.env("LD_LIBRARY_PATH", ld);
    }

    // Don't flash a console window on Windows (we discard stdio anyway).
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let child = cmd.spawn()?;
    Ok(child)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_from_url_parses_or_defaults() {
        assert_eq!(port_from_url("http://127.0.0.1:8080"), 8080);
        assert_eq!(port_from_url("http://127.0.0.1:11434/"), 11434);
        assert_eq!(port_from_url("http://localhost"), 8080); // no port → default chat port
        assert_eq!(port_from_url("garbage"), 8080);
    }

    #[test]
    fn embed_base_url_uses_the_embed_port() {
        assert_eq!(embed_base_url(), format!("http://127.0.0.1:{EMBED_PORT}"));
        assert_eq!(port_from_url(&embed_base_url()), EMBED_PORT);
    }

    #[test]
    fn model_info_known_and_unknown() {
        // Every model in the catalog resolves; a bogus id does not.
        assert!(!MODELS.is_empty());
        for m in MODELS {
            assert_eq!(model_info(m.id).map(|i| i.id), Some(m.id));
        }
        assert!(model_info("does-not-exist").is_none());
    }

    #[test]
    fn platform_has_an_engine_asset_candidate() {
        // The CI build runs on a supported OS/arch, so there must be a candidate list for it.
        assert!(platform_asset_candidates().is_some(), "this platform should have llama.cpp asset substrings");
    }

    // Hits the real llama.cpp releases API — manual gate (network), like the live eval harness.
    #[tokio::test]
    #[ignore]
    async fn find_engine_download_picks_cuda_binaries_and_cudart() {
        let client = reqwest::Client::new();
        let cuda = find_engine_download(&client, true).await.unwrap();
        // On Windows x64 a GPU build = the CUDA binaries (not the cudart archive) + a matching cudart.
        #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
        {
            assert!(cuda.binaries.0.contains("cuda") && !cuda.binaries.0.starts_with("cudart"), "binaries: {}", cuda.binaries.0);
            assert!(cuda.cudart.as_ref().map(|c| c.0.starts_with("cudart")).unwrap_or(false), "cudart: {:?}", cuda.cudart);
        }
        // A CPU build never carries a cudart.
        let cpu = find_engine_download(&client, false).await.unwrap();
        assert!(cpu.cudart.is_none(), "cpu build should have no cudart");
    }

    #[test]
    fn sha256_gate_accepts_match_and_rejects_everything_else() {
        // sha256("hello world")
        let good = "sha256:b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";
        assert!(verify_sha256(b"hello world", good).is_ok());
        // hex compare is case-insensitive (prefix stays lowercase, as GitHub sends it)
        assert!(verify_sha256(b"hello world", "sha256:B94D27B9934D3E08A52E52D7DA7DABFAC484EFE37A5380EE9088F7ACE2EFCDE9").is_ok());
        assert!(verify_sha256(b"tampered bytes", good).is_err()); // wrong content
        assert!(verify_sha256(b"hello world", "md5:abc").is_err()); // non-sha256 algo
        assert!(verify_sha256(b"hello world", "").is_err()); // missing digest → fail closed
    }

    /// The engine archive may bury the binary under a top-level dir (macOS/Linux) or lay it
    /// flat (Windows). Extraction + flatten must land the binary AND its sibling libs in `bin/`
    /// either way. Uses the platform's real `server_bin_name()`, so it covers Windows `.exe`.
    #[test]
    fn extract_and_flatten_recovers_engine_and_libs() {
        let base = std::env::temp_dir().join(format!("pushin_engine_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();

        // A zip that nests the binary + a sibling shared lib under build/bin/, like a release.
        let archive = base.join("engine.zip");
        {
            let f = std::fs::File::create(&archive).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            zw.start_file(format!("build/bin/{}", server_bin_name()), opts).unwrap();
            zw.write_all(b"binary").unwrap();
            zw.start_file("build/bin/libggml-base.so", opts).unwrap();
            zw.write_all(b"lib").unwrap();
            zw.finish().unwrap();
        }

        let staging = base.join("_unpack");
        std::fs::create_dir_all(&staging).unwrap();
        extract_zip(&archive, &staging).unwrap();

        let bin = base.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        flatten_files_into(&staging, &bin).unwrap();

        assert!(bin.join(server_bin_name()).is_file(), "engine binary should flatten into bin/");
        assert!(bin.join("libggml-base.so").is_file(), "co-located lib should flatten into bin/");

        let _ = std::fs::remove_dir_all(&base);
    }
}
