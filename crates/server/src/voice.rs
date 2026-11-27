//! Voice input — a port of `voice.ts`.
//!
//! A recording from the browser becomes text for the chat input, with whisper.cpp on this
//! machine. The hub finds `whisper-cli` on PATH or builds its own copy into
//! `<data>/whisper/bin` (a terminal job: the source archive, and a portable cmake into
//! `<data>/whisper/cmake` when the host has none), downloads ggml models into
//! `<data>/whisper/models` and converts the recording to 16 kHz WAV with ffmpeg. The speech
//! never leaves the host.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use hubness_core::{JobInfo, TranscriptionResult, VoiceLocal, VoiceModel, VoiceProvider, VoiceSettings};
use regex::Regex;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::AppState;
use crate::config::Config;
use crate::db::kv;
use crate::error::{ApiResult, HttpError};
use crate::http::{JsonBody, LenientBody};
use crate::runs::TermSize;
use crate::terminal::{JobOpts, shq, start_job};

pub const MAX_AUDIO: usize = 25 * 1024 * 1024;
const KEY: &str = "settings.voice";
const MODELS_BASE: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main";
const WHISPER_REPO: &str = "https://github.com/ggml-org/whisper.cpp";
const WHISPER_ARCHIVE: &str = "https://github.com/ggml-org/whisper.cpp/archive/refs/heads/master.tar.gz";
/// The portable cmake the hub downloads when the host has none (Kitware's release archives).
const CMAKE_VERSION: &str = "3.31.6";
const DEFAULT_MODEL: &str = "large-v3-turbo-q5_0";

/// Models offered in Settings, smallest first; ids are ggml file names without `ggml-` and `.bin`.
pub const MODELS: &[(&str, &str, u64)] = &[
    ("tiny", "Tiny", 77_691_713),
    ("base", "Base", 147_951_465),
    ("small", "Small", 487_601_967),
    ("large-v3-turbo-q5_0", "Large v3 Turbo (q5)", 574_041_195),
    ("large-v3-turbo", "Large v3 Turbo", 1_624_555_275),
];

static LANG_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(auto|[a-z]{2,3})$").unwrap());

/// The kv row, shared with the TS server; the removed cloud service's fields (`apiUrl`,
/// `apiModel`, the encrypted `apiKey`) of older settings are dropped by the next save.
#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct Stored {
    #[serde(skip_serializing_if = "Option::is_none")]
    provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    local_model: Option<String>,
}

fn root(data_dir: &Path) -> PathBuf {
    data_dir.join("whisper")
}
fn models_dir(data_dir: &Path) -> PathBuf {
    root(data_dir).join("models")
}
fn model_file(data_dir: &Path, id: &str) -> PathBuf {
    models_dir(data_dir).join(format!("ggml-{id}.bin"))
}
fn managed_binary(data_dir: &Path) -> PathBuf {
    root(data_dir).join("bin").join("whisper-cli")
}

fn which(bin: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path).filter(|d| !d.as_os_str().is_empty()).map(|d| d.join(bin)).find(|p| p.is_file())
}

/// whisper-cli to run: the hub's own build first, then PATH (`whisper-cpp` is the Homebrew/Nix name).
fn find_binary(data_dir: &Path) -> Option<(PathBuf, bool)> {
    let own = managed_binary(data_dir);
    if own.is_file() {
        return Some((own, true));
    }
    ["whisper-cli", "whisper-cpp"].iter().find_map(|n| which(n)).map(|p| (p, false))
}

/// What the host must have to build whisper.cpp; cmake isn't listed (the hub downloads a
/// portable one) and neither is git (the source comes as an archive).
fn missing_build_tools() -> Vec<String> {
    let mut out = vec![];
    if !["c++", "g++", "clang++"].iter().any(|c| which(c).is_some()) {
        out.push("c++".into());
    }
    if which("make").is_none() {
        out.push("make".into());
    }
    if which("curl").is_none() && which("wget").is_none() {
        out.push("curl".into());
    }
    if which("tar").is_none() {
        out.push("tar".into());
    }
    out
}

fn stored(db: &Connection) -> ApiResult<Stored> {
    Ok(kv::get::<Stored>(db, KEY)?.unwrap_or_default())
}

pub fn settings(db: &Connection, config: &Config) -> ApiResult<VoiceSettings> {
    let s = stored(db)?;
    let data = &config.data_dir;
    let provider = s.provider.as_deref().and_then(VoiceProvider::parse).unwrap_or_default();
    let model = s.local_model.filter(|m| MODELS.iter().any(|(id, ..)| id == m)).unwrap_or_else(|| DEFAULT_MODEL.into());
    let bin = find_binary(data);
    let ffmpeg = which("ffmpeg").is_some();
    let models: Vec<VoiceModel> = MODELS
        .iter()
        .map(|(id, label, size)| VoiceModel {
            id: (*id).into(),
            label: (*label).into(),
            size: *size,
            installed: model_file(data, id).is_file(),
        })
        .collect();
    // a cloud service (`api`, removed) left in older settings counts as off
    let ready = provider == VoiceProvider::Local && bin.is_some() && ffmpeg && models.iter().any(|m| m.id == model && m.installed);
    Ok(VoiceSettings {
        provider,
        language: s.language.unwrap_or_else(|| "auto".into()),
        ready,
        local: VoiceLocal {
            model,
            binary: bin.as_ref().map(|(p, _)| p.to_string_lossy().into_owned()),
            managed: bin.is_some_and(|(_, m)| m),
            ffmpeg,
            missing_build_tools: missing_build_tools(),
            models,
        },
    })
}

/// PUT /api/voice: only the given fields change.
pub fn set_settings(db: &Connection, config: &Config, b: &serde_json::Map<String, Value>) -> ApiResult<VoiceSettings> {
    let mut next = stored(db)?;
    next.provider = next.provider.filter(|p| VoiceProvider::parse(p).is_some());
    if let Some(v) = b.get("provider") {
        let p = v.as_str().filter(|p| VoiceProvider::parse(p).is_some()).ok_or_else(|| HttpError::bad_request("provider: off | local"))?;
        next.provider = Some(p.into());
    }
    if let Some(v) = b.get("language") {
        let l = v
            .as_str()
            .filter(|l| LANG_RE.is_match(l))
            .ok_or_else(|| HttpError::bad_request("language: auto or a language code like en"))?;
        next.language = Some(l.into());
    }
    if let Some(v) = b.get("localModel") {
        let m = v.as_str().filter(|m| MODELS.iter().any(|(id, ..)| id == m));
        let Some(m) = m else {
            let shown = v.as_str().map(String::from).unwrap_or_else(|| v.to_string());
            return Err(HttpError::bad_request(format!("Unknown model: {shown}")));
        };
        next.local_model = Some(m.into());
    }
    kv::set(db, KEY, &next)?;
    settings(db, config)
}

// ---------- whisper.cpp: build and models (terminal jobs) ----------

fn parallelism() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1)
}

fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// Build whisper-cli from source into the data folder (static, tuned for this CPU).
pub fn install(config: &Config, size: &TermSize) -> ApiResult<JobInfo> {
    let missing = missing_build_tools();
    if !missing.is_empty() {
        return Err(HttpError::bad_request(format!("Missing to build whisper.cpp: {}", missing.join(", "))));
    }
    let script = build_script(&path_str(&root(&config.data_dir)), &path_str(&managed_binary(&config.data_dir)), parallelism().clamp(1, 16));
    Ok(start_job(JobOpts {
        title: "whisper.cpp · build".into(),
        command: script,
        display: Some(format!("build whisper.cpp from {WHISPER_REPO}")),
        cols: size.cols,
        rows: size.rows,
        ..Default::default()
    }))
}

/// The build as a shell script (the same text as `buildScript` in `voice.ts`).
fn build_script(root: &str, bin: &str, jobs: usize) -> String {
    let src = format!("{root}/src");
    let build = format!("{src}/build");
    let cmake_dir = format!("{root}/cmake");
    [
        "set -e".to_string(),
        "fetch() { if command -v curl >/dev/null 2>&1; then curl -fsSL \"$1\"; else wget -qO- \"$1\"; fi; }".into(),
        // cmake: the host's, else a portable one kept next to the build
        "if command -v cmake >/dev/null 2>&1; then CMAKE=cmake; else".into(),
        format!("  CMAKE={}; [ -x \"$CMAKE\" ] || CMAKE={}", shq(&format!("{cmake_dir}/bin/cmake")), shq(&format!("{cmake_dir}/CMake.app/Contents/bin/cmake"))),
        "  if [ ! -x \"$CMAKE\" ]; then".into(),
        "    case \"$(uname -s)-$(uname -m)\" in".into(),
        "      Linux-x86_64) A=linux-x86_64 ;; Linux-aarch64|Linux-arm64) A=linux-aarch64 ;; Darwin-*) A=macos-universal ;;".into(),
        "      *) echo 'No portable cmake for this platform: install cmake'; exit 1 ;;".into(),
        "    esac".into(),
        format!("    echo \"Downloading cmake {CMAKE_VERSION}…\""),
        format!("    rm -rf {d} && mkdir -p {d}", d = shq(&cmake_dir)),
        format!(
            "    fetch \"https://github.com/Kitware/CMake/releases/download/v{CMAKE_VERSION}/cmake-{CMAKE_VERSION}-$A.tar.gz\" | tar -xz --strip-components=1 -C {}",
            shq(&cmake_dir)
        ),
        format!("    CMAKE={}; [ -x \"$CMAKE\" ] || CMAKE={}", shq(&format!("{cmake_dir}/bin/cmake")), shq(&format!("{cmake_dir}/CMake.app/Contents/bin/cmake"))),
        "  fi".into(),
        "fi".into(),
        "echo 'Downloading whisper.cpp…'".into(),
        format!("rm -rf {s} && mkdir -p {s}", s = shq(&src)),
        format!("fetch {WHISPER_ARCHIVE} | tar -xz --strip-components=1 -C {}", shq(&src)),
        format!("\"$CMAKE\" -S {} -B {} -DCMAKE_BUILD_TYPE=Release -DBUILD_SHARED_LIBS=OFF -DWHISPER_BUILD_TESTS=OFF", shq(&src), shq(&build)),
        format!("\"$CMAKE\" --build {} --config Release --target whisper-cli -j {jobs}", shq(&build)),
        format!("mkdir -p {}", shq(&format!("{root}/bin"))),
        format!("cp {} {}", shq(&format!("{build}/bin/whisper-cli")), shq(&format!("{bin}.new"))),
        format!("mv {} {}", shq(&format!("{bin}.new")), shq(bin)),
        format!("rm -rf {}", shq(&src)),
        format!("echo; echo {}", shq(&format!("whisper.cpp is installed: {bin}"))),
    ]
    .join("\n")
}

fn model_or_404(id: &str) -> ApiResult<()> {
    if MODELS.iter().any(|(m, ..)| *m == id) { Ok(()) } else { Err(HttpError::not_found(format!("Unknown model: {id}"))) }
}

/// Download a ggml model (curl, or wget) into the data folder; a partial file never counts as installed.
pub fn download_model(config: &Config, id: &str, size: &TermSize) -> ApiResult<JobInfo> {
    model_or_404(id)?;
    let file = path_str(&model_file(&config.data_dir, id));
    let part = format!("{file}.part");
    let url = format!("{MODELS_BASE}/ggml-{id}.bin");
    let script = [
        "set -e".to_string(),
        format!("mkdir -p {}", shq(&path_str(&models_dir(&config.data_dir)))),
        format!(
            "if command -v curl >/dev/null 2>&1; then curl -fL --progress-bar -o {p} {u}; else wget -O {p} {u}; fi",
            p = shq(&part),
            u = shq(&url)
        ),
        format!("mv {} {}", shq(&part), shq(&file)),
    ]
    .join("\n");
    Ok(start_job(JobOpts {
        title: format!("whisper.cpp · {id}"),
        command: script,
        display: Some(format!("download {url}")),
        cols: size.cols,
        rows: size.rows,
        ..Default::default()
    }))
}

pub fn delete_model(db: &Connection, config: &Config, id: &str) -> ApiResult<VoiceSettings> {
    model_or_404(id)?;
    let file = model_file(&config.data_dir, id);
    let _ = std::fs::remove_file(&file);
    let _ = std::fs::remove_file(format!("{}.part", file.display()));
    settings(db, config)
}

// ---------- transcription ----------

/// Run a tool with the child environment; `(exit code, stderr tail)`, `None` code on a spawn failure or timeout.
async fn run(cmd: &Path, args: &[&str], timeout: Duration) -> (Option<i32>, String) {
    let child = tokio::process::Command::new(cmd)
        .args(args)
        .env_clear()
        .envs(crate::config::child_env())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn();
    let child = match child {
        Ok(c) => c,
        Err(e) => return (None, e.to_string()),
    };
    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(out)) => {
            let err = String::from_utf8_lossy(&out.stderr);
            let tail: String = err.chars().rev().take(4000).collect::<Vec<_>>().into_iter().rev().collect();
            (out.status.code(), tail)
        }
        Ok(Err(e)) => (None, e.to_string()),
        Err(_) => (None, "timed out".into()),
    }
}

fn last_line(s: &str) -> String {
    s.lines().map(str::trim).rfind(|l| !l.is_empty()).unwrap_or_default().to_string()
}

/// whisper.cpp prints one segment per line and marks silence: plain text for the input.
pub fn clean_transcript(raw: &str) -> String {
    let joined = raw.replace("[BLANK_AUDIO]", " ").lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" ");
    joined.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A temporary folder removed on drop.
struct TempDir(PathBuf);
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn transcribe_local(config: &Config, audio: &[u8], s: &VoiceSettings) -> ApiResult<String> {
    let bin = s.local.binary.clone().ok_or_else(|| HttpError::new(409, "whisper.cpp is not installed: Settings → Voice input"))?;
    let ffmpeg = which("ffmpeg").ok_or_else(|| HttpError::new(409, "ffmpeg is not installed"))?;
    let model = model_file(&config.data_dir, &s.local.model);
    if !model.is_file() {
        return Err(HttpError::new(409, "The model is not downloaded: Settings → Voice input"));
    }
    let dir = TempDir(std::env::temp_dir().join(format!("hubness-voice-{}", crate::db::new_id())));
    std::fs::create_dir_all(&dir.0)?;
    let input = dir.0.join("input");
    let wav = dir.0.join("audio.wav");
    std::fs::write(&input, audio)?;
    let (input_s, wav_s) = (path_str(&input), path_str(&wav));
    let (code, err) = run(
        &ffmpeg,
        &["-nostdin", "-loglevel", "error", "-y", "-i", &input_s, "-ar", "16000", "-ac", "1", "-c:a", "pcm_s16le", &wav_s],
        Duration::from_secs(60),
    )
    .await;
    if code != Some(0) {
        let line = last_line(&err);
        return Err(HttpError::bad_request(format!(
            "Couldn’t decode the recording: {}",
            if line.is_empty() { "ffmpeg failed".into() } else { line }
        )));
    }
    let threads = parallelism().clamp(1, 8).to_string();
    let out = path_str(&dir.0.join("out"));
    let model_s = path_str(&model);
    let (code, err) = run(
        Path::new(&bin),
        &["-m", &model_s, "-f", &wav_s, "-l", &s.language, "-t", &threads, "-nt", "-np", "-otxt", "-of", &out],
        Duration::from_secs(300),
    )
    .await;
    let txt = format!("{out}.txt");
    if code != Some(0) || !Path::new(&txt).is_file() {
        let line = last_line(&err);
        let why =
            if line.is_empty() { format!("exit code {}", code.map(|c| c.to_string()).unwrap_or_else(|| "null".into())) } else { line };
        return Err(HttpError::new(500, format!("whisper.cpp failed: {why}")));
    }
    Ok(clean_transcript(&std::fs::read_to_string(&txt)?))
}

// ---------- routes ----------

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/voice", get(get_route).put(put_route))
        .route("/voice/install", post(install_route))
        .route("/voice/models/{id}", post(download_route).delete(delete_route))
        .route("/voice/transcribe", post(transcribe_route).layer(axum::extract::DefaultBodyLimit::max(MAX_AUDIO + 1024)))
}

async fn get_route(State(s): State<AppState>) -> ApiResult<Json<VoiceSettings>> {
    s.with(settings)
}

async fn put_route(State(s): State<AppState>, JsonBody(b): JsonBody<serde_json::Map<String, Value>>) -> ApiResult<Json<VoiceSettings>> {
    s.with(|db, cfg| set_settings(db, cfg, &b))
}

/// Build whisper.cpp from source: a terminal job (/api/jobs/:id/stream).
async fn install_route(State(s): State<AppState>, LenientBody(size): LenientBody<TermSize>) -> ApiResult<(StatusCode, Json<JobInfo>)> {
    size.check()?;
    Ok((StatusCode::CREATED, Json(install(s.config(), &size)?)))
}

/// Download a model: a terminal job.
async fn download_route(
    State(s): State<AppState>,
    UrlPath(id): UrlPath<String>,
    LenientBody(size): LenientBody<TermSize>,
) -> ApiResult<(StatusCode, Json<JobInfo>)> {
    size.check()?;
    Ok((StatusCode::CREATED, Json(download_model(s.config(), &id, &size)?)))
}

async fn delete_route(State(s): State<AppState>, UrlPath(id): UrlPath<String>) -> ApiResult<Json<VoiceSettings>> {
    s.with(|db, cfg| delete_model(db, cfg, &id))
}

/// The recording is the raw body (Content-Type: its audio type); answers `{ text }`.
async fn transcribe_route(State(s): State<AppState>, body: axum::body::Bytes) -> ApiResult<Json<TranscriptionResult>> {
    if body.is_empty() {
        return Err(HttpError::bad_request("The recording is empty"));
    }
    if body.len() > MAX_AUDIO {
        return Err(HttpError::new(413, format!("The recording is larger than {} MB", MAX_AUDIO / 1024 / 1024)));
    }
    let settings = {
        let db = s.db()?;
        settings(&db, s.config())?
    };
    if settings.provider != VoiceProvider::Local {
        return Err(HttpError::new(409, "Voice input is off: Settings → Voice input"));
    }
    Ok(Json(TranscriptionResult { text: transcribe_local(s.config(), &body, &settings).await? }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_script_needs_no_cmake_or_git() {
        let s = build_script("/d/whisper", "/d/whisper/bin/whisper-cli", 4);
        assert!(s.contains("cmake-3.31.6-$A.tar.gz") && s.contains("archive/refs/heads/master.tar.gz"));
        assert!(!s.contains("git clone"));
        if let Ok(out) = std::env::var("HUBNESS_PRINT_BUILD_SCRIPT") {
            std::fs::write(out, &s).unwrap();
        }
    }

    #[test]
    fn transcript() {
        assert_eq!(clean_transcript(" [BLANK_AUDIO]\n Hello  there,\n world\n"), "Hello there, world");
    }
}
