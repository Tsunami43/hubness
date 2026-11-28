//! Text search across project files — a port of `search.ts` (the Search view, like VS
//! Code's). ripgrep when it is installed (fast, respects .gitignore, skips binaries);
//! otherwise a plain walk over the project files. Results are capped so a broad query can't
//! flood the browser.

use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use hubness_core::{SearchFile, SearchMatch, SearchResult};
use regex::{Regex, RegexBuilder};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};

use crate::error::{ApiResult, HttpError};
use crate::files::list_files;
use crate::sessions::truncate;

const MAX_MATCHES: usize = 2000;
const MAX_PER_FILE: usize = 200;
const MAX_LINE: usize = 400;
const TIMEOUT: Duration = Duration::from_secs(15);

pub struct Options {
    pub q: String,
    pub regex: bool,
    pub case_sensitive: bool,
    pub word: bool,
    pub include: Option<String>,
    pub exclude: Option<String>,
}

fn has_rg() -> bool {
    static OK: OnceLock<bool> = OnceLock::new();
    *OK.get_or_init(|| {
        std::process::Command::new("rg")
            .arg("--version")
            .env_clear()
            .envs(crate::config::child_env())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    })
}

fn globs(s: &Option<String>) -> Vec<String> {
    s.as_deref().unwrap_or("").split(',').map(str::trim).filter(|g| !g.is_empty()).map(String::from).collect()
}

/// UTF-8 byte offset inside `text` → UTF-16 index (what the browser works with).
fn utf16_at(text: &str, bytes: usize) -> usize {
    let cut = bytes.min(text.len());
    String::from_utf8_lossy(&text.as_bytes()[..cut]).encode_utf16().count()
}

fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

pub async fn search_files(root: &Path, opts: Options) -> ApiResult<SearchResult> {
    if opts.q.trim().is_empty() {
        return Ok(SearchResult { files: vec![], total: 0, truncated: false });
    }
    if utf16_len(&opts.q) > 500 {
        return Err(HttpError::bad_request("Query is too long"));
    }
    if opts.regex
        && let Err(e) = Regex::new(&opts.q)
    {
        return Err(HttpError::bad_request(format!("Invalid regular expression: {e}")));
    }
    let cwd = std::fs::canonicalize(root)?;
    if has_rg() {
        search_rg(&cwd, &opts).await
    } else {
        // the fallback reads every file itself: off the async threads
        Ok(crate::http::blocking(move || search_walk(&cwd, &opts)).await?.0)
    }
}

async fn search_rg(cwd: &Path, opts: &Options) -> ApiResult<SearchResult> {
    let mut args: Vec<String> =
        ["--json", "--hidden", "--glob", "!.git", "--max-columns", "400", "--max-columns-preview", "--max-filesize", "2M"]
            .map(String::from)
            .to_vec();
    args.extend(["--max-count".into(), MAX_PER_FILE.to_string()]);
    args.push(if opts.case_sensitive { "--case-sensitive" } else { "--ignore-case" }.into());
    if opts.word {
        args.push("--word-regexp".into());
    }
    if !opts.regex {
        args.push("--fixed-strings".into());
    }
    for g in globs(&opts.include) {
        args.extend(["--glob".into(), g]);
    }
    for g in globs(&opts.exclude) {
        args.extend(["--glob".into(), format!("!{g}")]);
    }
    args.extend(["--".into(), opts.q.clone(), ".".into()]);

    let mut child = tokio::process::Command::new("rg")
        .args(&args)
        .current_dir(cwd)
        .env_clear()
        .envs(crate::config::child_env())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| HttpError::new(500, format!("Search failed: {e}")))?;
    let mut stderr = child.stderr.take().expect("piped stderr");
    let err_task = tokio::spawn(async move {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s).await;
        s
    });
    let mut lines = BufReader::new(child.stdout.take().expect("piped stdout")).lines();
    let mut order: Vec<String> = vec![];
    let mut by_path: HashMap<String, Vec<SearchMatch>> = HashMap::new();
    let mut total = 0;
    let mut truncated = false;
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let line = match tokio::time::timeout(left, lines.next_line()).await {
            Err(_) => {
                truncated = true;
                break;
            }
            Ok(Ok(Some(l))) => l,
            Ok(_) => break,
        };
        let Ok(m) = serde_json::from_str::<Value>(&line) else { continue };
        if m["type"] != "match" {
            continue;
        }
        let d = &m["data"];
        let path = d["path"]["text"].as_str().unwrap_or_default();
        let path = path.strip_prefix("./").unwrap_or(path).to_string();
        let raw = d["lines"]["text"].as_str().unwrap_or_default();
        let raw = raw.strip_suffix('\n').map(|r| r.strip_suffix('\r').unwrap_or(r)).unwrap_or(raw);
        let text = if utf16_len(raw) > MAX_LINE { truncate(raw, MAX_LINE) } else { raw.to_string() };
        let text_len = utf16_len(&text);
        let ranges = d["submatches"]
            .as_array()
            .map(|subs| {
                subs.iter()
                    .map(|s| {
                        (utf16_at(raw, s["start"].as_u64().unwrap_or(0) as usize), utf16_at(raw, s["end"].as_u64().unwrap_or(0) as usize))
                    })
                    .filter(|(a, _)| *a < text_len)
                    .map(|(a, b)| (a, b.min(text_len)))
                    .collect()
            })
            .unwrap_or_default();
        if !by_path.contains_key(&path) {
            order.push(path.clone());
        }
        by_path.entry(path).or_default().push(SearchMatch { line: d["line_number"].as_u64().unwrap_or(0), text, ranges });
        total += 1;
        if total >= MAX_MATCHES {
            truncated = true;
            break;
        }
    }
    if truncated {
        let _ = child.kill().await;
    }
    let status = child.wait().await.ok();
    let err = err_task.await.unwrap_or_default();
    // 0 — matches, 1 — nothing found, killed — limit reached; 2 — a real error (bad glob…)
    if status.and_then(|s| s.code()) == Some(2) && !truncated && total == 0 {
        let first = err.trim().lines().next().unwrap_or("Search failed").to_string();
        return Err(HttpError::bad_request(first));
    }
    let files = order.into_iter().map(|p| SearchFile { matches: by_path.remove(&p).unwrap_or_default(), path: p }).collect();
    Ok(SearchResult { files, total, truncated })
}

/// Glob → regex for the fallback (supports *, ** and ?; a pattern without / matches the
/// file name anywhere).
fn glob_re(g: &str) -> Regex {
    let chars: Vec<char> = g.chars().collect();
    let mut re = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '*' && chars.get(i + 1) == Some(&'*') {
            re.push_str(".*");
            i += 1;
            if chars.get(i + 1) == Some(&'/') {
                i += 1;
            }
        } else if c == '*' {
            re.push_str("[^/]*");
        } else if c == '?' {
            re.push_str("[^/]");
        } else {
            re.push_str(&regex::escape(&c.to_string()));
        }
        i += 1;
    }
    let full = if g.contains('/') { format!("^{re}$") } else { format!("(^|/){re}(/|$)") };
    Regex::new(&full).expect("escaped glob")
}

fn search_walk(cwd: &Path, opts: &Options) -> ApiResult<SearchResult> {
    let source = if opts.regex { opts.q.clone() } else { regex::escape(&opts.q) };
    let pattern = if opts.word { format!(r"\b(?:{source})\b") } else { source };
    let re =
        RegexBuilder::new(&pattern).case_insensitive(!opts.case_sensitive).build().map_err(|e| HttpError::bad_request(e.to_string()))?;
    let inc: Vec<Regex> = globs(&opts.include).iter().map(|g| glob_re(g)).collect();
    let exc: Vec<Regex> = globs(&opts.exclude).iter().map(|g| glob_re(g)).collect();
    let mut files = vec![];
    let mut total = 0;
    let started = Instant::now();
    for path in list_files(cwd, 20_000)? {
        if !inc.is_empty() && !inc.iter().any(|r| r.is_match(&path)) {
            continue;
        }
        if exc.iter().any(|r| r.is_match(&path)) {
            continue;
        }
        if started.elapsed() > TIMEOUT {
            return Ok(SearchResult { files, total, truncated: true });
        }
        let abs = cwd.join(&path);
        if std::fs::metadata(&abs).map(|m| m.len() > 2 * 1024 * 1024).unwrap_or(true) {
            continue;
        }
        let Ok(content) = std::fs::read(&abs) else { continue };
        if content[..content.len().min(8000)].contains(&0) {
            continue;
        }
        let content = String::from_utf8_lossy(&content);
        let mut matches = vec![];
        for (n, line) in content.split('\n').enumerate() {
            let raw = line.strip_suffix('\r').unwrap_or(line);
            let text = truncate(raw, MAX_LINE);
            let text_len = utf16_len(&text);
            let ranges: Vec<(usize, usize)> = re
                .find_iter(raw)
                .filter(|m| !m.as_str().is_empty())
                .map(|m| (utf16_at(raw, m.start()), utf16_at(raw, m.end())))
                .filter(|(a, _)| *a < text_len)
                .map(|(a, b)| (a, b.min(text_len)))
                .collect();
            if ranges.is_empty() {
                continue;
            }
            matches.push(SearchMatch { line: n as u64 + 1, text, ranges });
            total += 1;
            if total >= MAX_MATCHES {
                files.push(SearchFile { path, matches });
                return Ok(SearchResult { files, total, truncated: true });
            }
            if matches.len() >= MAX_PER_FILE {
                break;
            }
        }
        if !matches.is_empty() {
            files.push(SearchFile { path, matches });
        }
    }
    Ok(SearchResult { files, total, truncated: false })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs_like_the_ts_fallback() {
        assert!(glob_re("*.ts").is_match("src/a.ts"));
        assert!(!glob_re("*.ts").is_match("src/a.tsx"));
        assert!(glob_re("src/**/*.rs").is_match("src/a/b/c.rs"));
        assert!(glob_re("node_modules").is_match("x/node_modules/y"));
    }

    #[test]
    fn walk_finds_utf16_ranges() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "привет world\nnone\n").unwrap();
        let opts = Options { q: "WORLD".into(), regex: false, case_sensitive: false, word: true, include: None, exclude: None };
        let r = search_walk(dir.path(), &opts).unwrap();
        assert_eq!(r.total, 1);
        assert_eq!(r.files[0].matches[0].ranges, vec![(7, 12)]);
        assert_eq!(utf16_at("привет world", 13), 7);
    }
}
