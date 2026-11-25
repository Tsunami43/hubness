//! Files attached to messages — a port of `attachments.ts`. They live in the session's
//! working folder (`.hub/attachments/`), not in the hub data: the sandbox hides the hub data
//! from agents, and an agent opens a video, a PDF or an archive by path. Inside a repository
//! the folder is excluded locally (`.git/info/exclude`): nothing to commit by accident.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use hubness_core::Attachment;
use serde_json::Value;

use crate::error::{ApiResult, HttpError};
use crate::paths;

pub const MAX_ATTACHMENT: usize = 20 * 1024 * 1024;
const EXCLUDE_LINE: &str = "/.hub/attachments/";
/// Images a model takes directly; larger ones go by path only.
const MODEL_IMAGES: &[&str] = &["image/png", "image/jpeg", "image/gif", "image/webp"];
const MODEL_IMAGE_MAX: u64 = 5 * 1024 * 1024;

pub fn is_model_image(a: &Attachment) -> bool {
    MODEL_IMAGES.contains(&a.mime.as_str()) && a.size <= MODEL_IMAGE_MAX
}

fn mime_by_ext(name: &str) -> &'static str {
    let ext = Path::new(name).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "pdf" => "application/pdf",
        "txt" => "text/plain",
        "md" => "text/markdown",
        "json" => "application/json",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
}

/// `type/subtype` made of `[\w.+-]` (what the TS server accepts as a MIME type).
fn valid_mime(m: &str) -> bool {
    let ok = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "_.+-".contains(c));
    matches!(m.split_once('/'), Some((a, b)) if ok(a) && ok(b))
}

fn dir_of(cwd: &Path) -> PathBuf {
    cwd.join(".hub").join("attachments")
}

/// Keep the attachments folder out of git, locally (works for worktrees too).
fn exclude_from_git(cwd: &Path) {
    let Ok(out) = Command::new("git").arg("-C").arg(cwd).args(["rev-parse", "--show-toplevel", "--git-path", "info/exclude"]).output()
    else {
        return;
    };
    if !out.status.success() {
        return;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut lines = text.trim().lines();
    let (Some(top), Some(rel)) = (lines.next(), lines.next()) else { return };
    // only when the session folder is the repository root (a group workspace is not a repository)
    if paths::normalize(Path::new(top)) != paths::normalize(cwd) {
        return;
    }
    let file = paths::normalize(&cwd.join(rel));
    let cur = fs::read_to_string(&file).unwrap_or_default();
    if cur.lines().any(|l| l.trim() == EXCLUDE_LINE) {
        return;
    }
    let _ = fs::create_dir_all(file.parent().unwrap_or(cwd));
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(&file) {
        let sep = if !cur.is_empty() && !cur.ends_with('\n') { "\n" } else { "" };
        let _ = write!(f, "{sep}# hubness: files attached in chats\n{EXCLUDE_LINE}\n");
    }
}

/// A file name safe on every OS and in a shell command line.
pub fn safe_name(name: &str) -> String {
    let replaced: String = name
        .chars()
        .map(|c| if c == '/' || c == '\\' { '_' } else { c })
        .map(|c| if c.is_alphanumeric() || "._ -".contains(c) { c } else { '_' })
        .collect();
    let collapsed = replaced.split_whitespace().collect::<Vec<_>>().join(" ");
    let base = crate::sessions::truncate(collapsed.trim(), 100);
    if base.is_empty() || base.chars().all(|c| c == '.') { "file".into() } else { base }
}

pub fn save(cwd: &Path, name: &str, mime: &str, bytes: &[u8]) -> ApiResult<Attachment> {
    if bytes.is_empty() {
        return Err(HttpError::bad_request("Empty file"));
    }
    if bytes.len() > MAX_ATTACHMENT {
        return Err(HttpError::new(413, format!("The file is larger than {} MB", MAX_ATTACHMENT / 1024 / 1024)));
    }
    let dir = dir_of(cwd);
    fs::create_dir_all(&dir)?;
    exclude_from_git(cwd);
    let clean = safe_name(name);
    let id = format!("{}-{clean}", &crate::db::new_id()[..8]);
    let path = dir.join(&id);
    fs::write(&path, bytes)?;
    let mime = if valid_mime(mime) { mime.to_string() } else { mime_by_ext(&clean).to_string() };
    Ok(Attachment { id, name: clean, mime, size: bytes.len() as u64, path: paths::to_string(&path) })
}

/// Absolute path of an attachment; 404 for anything outside the folder or missing.
pub fn path_of(cwd: &Path, id: &str) -> ApiResult<PathBuf> {
    let not_found = || HttpError::not_found("Attachment not found");
    if id.is_empty() || id.contains('/') || id.contains('\\') || id.starts_with('.') {
        return Err(not_found());
    }
    let path = dir_of(cwd).join(id);
    if !path.is_file() {
        return Err(not_found());
    }
    Ok(path)
}

/// Attachments of a message by id: only files really present in this session's folder.
pub fn resolve(cwd: &Path, list: Option<&Value>) -> ApiResult<Vec<Attachment>> {
    let items = match list {
        None | Some(Value::Null) => return Ok(vec![]),
        Some(Value::Array(a)) if a.len() <= 20 => a,
        _ => return Err(HttpError::bad_request("attachments must be a list of up to 20 items")),
    };
    items
        .iter()
        .map(|a| {
            let id = a["id"].as_str().unwrap_or_default();
            let path = path_of(cwd, id)?;
            let size = fs::metadata(&path)?.len();
            let name = a["name"].as_str().map(safe_name).unwrap_or_else(|| id.to_string());
            let mime = a["mime"].as_str().filter(|m| valid_mime(m)).unwrap_or("application/octet-stream").to_string();
            Ok(Attachment { id: id.to_string(), name, mime, size, path: paths::to_string(&path) })
        })
        .collect()
}

/// The line the agent reads: where the files are (it opens them by path).
pub fn note(list: &[Attachment]) -> String {
    if list.is_empty() {
        return String::new();
    }
    let size = |n: u64| {
        if n < 1024 * 1024 {
            format!("{} KB", ((n as f64 / 1024.0).round() as u64).max(1))
        } else {
            format!("{:.1} MB", n as f64 / 1024.0 / 1024.0)
        }
    };
    let lines: Vec<String> = list.iter().map(|a| format!("- {} ({}, {})", a.path, a.mime, size(a.size))).collect();
    format!("\n\nAttached files (open them by path if needed):\n{}", lines.join("\n"))
}

/// Types an attachment is served with: images and videos inline, the rest as a download.
pub fn serve_mime(path: &Path) -> &'static str {
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_names() {
        assert_eq!(safe_name("../etc/passwd"), ".._etc_passwd");
        assert_eq!(safe_name("my  file (1).png"), "my file _1_.png");
        assert_eq!(safe_name("..."), "file");
        assert_eq!(safe_name("отчёт.pdf"), "отчёт.pdf");
    }

    #[test]
    fn note_sizes() {
        let a = Attachment { id: "x".into(), name: "x".into(), mime: "image/png".into(), size: 100, path: "/p/x".into() };
        assert_eq!(note(&[a]), "\n\nAttached files (open them by path if needed):\n- /p/x (image/png, 1 KB)");
    }
}
