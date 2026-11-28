//! Files attached to messages — a port of `attachments.ts`. They live in the session's
//! working folder, one folder per session (`.hub/attachments/<session id>/`), not in the hub
//! data: the sandbox hides the hub data from agents, and an agent opens a video, a PDF or an
//! archive by path. Inside a repository the folder is excluded locally (`.git/info/exclude`):
//! nothing to commit by accident.
//!
//! Archiving a session keeps its files (it can come back); deleting it removes its folder.
//! Files from before the per-session folders lie flat in `.hub/attachments/`: they are still
//! served, and deleting a session removes the ones its messages refer to.

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

fn root_of(cwd: &Path) -> PathBuf {
    cwd.join(".hub").join("attachments")
}

/// The session's own folder (session ids are UUIDs; anything else is made path-safe).
fn dir_of(cwd: &Path, session_id: &str) -> PathBuf {
    let safe: String = session_id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect();
    root_of(cwd).join(if safe.is_empty() { "_".into() } else { safe })
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && !id.contains('/') && !id.contains('\\') && !id.starts_with('.')
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

pub fn save(cwd: &Path, session_id: &str, name: &str, mime: &str, bytes: &[u8]) -> ApiResult<Attachment> {
    if bytes.is_empty() {
        return Err(HttpError::bad_request("Empty file"));
    }
    if bytes.len() > MAX_ATTACHMENT {
        return Err(HttpError::new(413, format!("The file is larger than {} MB", MAX_ATTACHMENT / 1024 / 1024)));
    }
    let dir = dir_of(cwd, session_id);
    fs::create_dir_all(&dir)?;
    exclude_from_git(cwd);
    let clean = safe_name(name);
    let id = format!("{}-{clean}", &crate::db::new_id()[..8]);
    let path = dir.join(&id);
    fs::write(&path, bytes)?;
    let mime = if valid_mime(mime) { mime.to_string() } else { mime_by_ext(&clean).to_string() };
    Ok(Attachment { id, name: clean, mime, size: bytes.len() as u64, path: paths::to_string(&path) })
}

/// Absolute path of an attachment: the session's folder, else a file from before the
/// per-session folders. 404 for anything outside the folder or missing.
pub fn path_of(cwd: &Path, session_id: &str, id: &str) -> ApiResult<PathBuf> {
    if valid_id(id) {
        for path in [dir_of(cwd, session_id).join(id), root_of(cwd).join(id)] {
            if path.is_file() {
                return Ok(path);
            }
        }
    }
    Err(HttpError::not_found("Attachment not found"))
}

/// The files of a deleted session: its folder, plus the flat files from before the
/// per-session folders that its messages refer to (`legacy_ids`). Best effort — a file that
/// can't be removed stays.
pub fn remove_session(cwd: &Path, session_id: &str, legacy_ids: &[String]) {
    let _ = fs::remove_dir_all(dir_of(cwd, session_id));
    for id in legacy_ids.iter().filter(|id| valid_id(id)) {
        let path = root_of(cwd).join(id);
        if fs::symlink_metadata(&path).is_ok_and(|m| m.is_file()) {
            let _ = fs::remove_file(path);
        }
    }
}

/// Attachment ids the user messages of a session's feed refer to.
pub fn ids_in_events(events: &[Value]) -> Vec<String> {
    events
        .iter()
        .filter(|e| e["kind"] == "user")
        .filter_map(|e| e["attachments"].as_array())
        .flatten()
        .filter_map(|a| a["id"].as_str().map(str::to_string))
        .collect()
}

/// Attachments of a message by id: only files really present in this session's folder.
pub fn resolve(cwd: &Path, session_id: &str, list: Option<&Value>) -> ApiResult<Vec<Attachment>> {
    let items = match list {
        None | Some(Value::Null) => return Ok(vec![]),
        Some(Value::Array(a)) if a.len() <= 20 => a,
        _ => return Err(HttpError::bad_request("attachments must be a list of up to 20 items")),
    };
    items
        .iter()
        .map(|a| {
            let id = a["id"].as_str().unwrap_or_default();
            let path = path_of(cwd, session_id, id)?;
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
    fn removing_a_session() {
        let dir = std::env::temp_dir().join(format!("hubness-attach-{}", crate::db::new_id()));
        let mine = save(&dir, "s1", "mine.png", "image/png", b"x").unwrap();
        let theirs = save(&dir, "s2", "theirs.png", "image/png", b"x").unwrap();
        fs::write(root_of(&dir).join("aaaa0000-old.png"), b"x").unwrap();
        fs::write(root_of(&dir).join("bbbb0000-other.png"), b"x").unwrap();
        assert_eq!(path_of(&dir, "s1", "aaaa0000-old.png").unwrap(), root_of(&dir).join("aaaa0000-old.png"));
        assert!(path_of(&dir, "s2", &mine.id).is_err(), "another session's file");
        remove_session(&dir, "s1", &["aaaa0000-old.png".into(), "../escape".into(), "missing".into()]);
        assert!(!Path::new(&mine.path).exists() && !dir_of(&dir, "s1").exists());
        assert!(!root_of(&dir).join("aaaa0000-old.png").exists());
        assert!(Path::new(&theirs.path).exists() && root_of(&dir).join("bbbb0000-other.png").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn note_sizes() {
        let a = Attachment { id: "x".into(), name: "x".into(), mime: "image/png".into(), size: 100, path: "/p/x".into() };
        assert_eq!(note(&[a]), "\n\nAttached files (open them by path if needed):\n- /p/x (image/png, 1 KB)");
    }
}
