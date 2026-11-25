//! Repository languages — a port of `languages.ts`, like the language bar on GitHub (GitHub
//! Linguist data: extensions, file names, colors; see `languages_data.rs`). We count bytes of
//! git-tracked files; like Linguist, we count only programming and markup languages,
//! skipping vendor/build/documentation. Ambiguous extensions are resolved by a preference
//! table (Linguist resolves them with content heuristics).

use std::collections::HashMap;
use std::path::Path;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use hubness_core::RepoLanguage;
use regex::Regex;

use crate::languages_data::{EXTENSIONS, FILENAMES, LANGUAGES, SKIPPED};

// like Linguist's vendor.yml/documentation.yml (abridged)
static SKIP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(^|/)(node_modules|vendor|third[_-]party|dist|build|out|target|\.yarn|bower_components|Pods|docs?|documentation)/|\.min\.(js|css)$|(^|/)(package-lock\.json|pnpm-lock\.yaml|yarn\.lock|Cargo\.lock|go\.sum|composer\.lock|Gemfile\.lock|poetry\.lock)$",
    )
    .unwrap()
});

fn lookup(table: &[(&str, u16)], key: &str) -> Option<u16> {
    table.binary_search_by(|(k, _)| (*k).cmp(key)).ok().map(|i| table[i].1)
}

/// Index in `LANGUAGES` (or `SKIPPED` for a known language that isn't counted).
fn lang_of(path: &str) -> Option<u16> {
    let name = &path[path.rfind('/').map(|i| i + 1).unwrap_or(0)..];
    if let Some(l) = lookup(FILENAMES, name) {
        return Some(l);
    }
    let lower = name.to_lowercase();
    // compound extensions (.d.ts, .blade.php) — from longest to shortest
    lower.match_indices('.').find_map(|(i, _)| lookup(EXTENSIONS, &lower[i..]))
}

/// Languages by repository folder, valid while HEAD stays the same.
type Cache = HashMap<String, (String, Vec<RepoLanguage>)>;
static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(Default::default);
const MAX_FILES: usize = 50_000;

pub async fn repo_languages(dir: &Path) -> Vec<RepoLanguage> {
    let key = crate::paths::to_string(dir);
    // empty repository without commits — count from the working tree
    let head = crate::git::run(dir, &["rev-parse", "HEAD"], Duration::from_secs(5), &[]).await;
    let head = if head.code == 0 { head.stdout.trim().to_string() } else { String::new() };
    if !head.is_empty()
        && let Some((h, langs)) = CACHE.lock().ok().and_then(|c| c.get(&key).cloned())
        && h == head
    {
        return langs;
    }
    let out = crate::git::run(dir, &["ls-files", "-z", "--cached", "--others", "--exclude-standard"], Duration::from_secs(10), &[]).await;
    if out.code != 0 {
        return vec![];
    }
    let dir_owned = dir.to_path_buf();
    let files: Vec<String> = out.stdout.split('\0').filter(|f| !f.is_empty()).take(MAX_FILES).map(String::from).collect();
    let langs = tokio::task::spawn_blocking(move || count(&dir_owned, &files)).await.unwrap_or_default();
    if !head.is_empty()
        && let Ok(mut c) = CACHE.lock()
    {
        c.insert(key, (head, langs.clone()));
    }
    langs
}

/// Bytes per language → percentages (rounded to 0.1 like `Math.round(x * 1000) / 10`).
fn count(dir: &Path, files: &[String]) -> Vec<RepoLanguage> {
    let mut bytes: Vec<(u16, u64)> = vec![];
    for f in files {
        if SKIP.is_match(f) {
            continue;
        }
        let Some(l) = lang_of(f).filter(|&l| l != SKIPPED) else { continue };
        let Ok(meta) = std::fs::metadata(dir.join(f)) else { continue };
        if !meta.is_file() {
            continue;
        }
        match bytes.iter_mut().find(|(x, _)| *x == l) {
            Some(e) => e.1 += meta.len(),
            None => bytes.push((l, meta.len())),
        }
    }
    let total: u64 = bytes.iter().map(|b| b.1).sum();
    if total == 0 {
        return vec![];
    }
    // stable: ties keep the order languages were first seen in
    bytes.sort_by_key(|b| std::cmp::Reverse(b.1));
    bytes
        .into_iter()
        .map(|(l, b)| {
            let (name, color) = LANGUAGES[l as usize];
            let percent = ((b as f64 / total as f64) * 1000.0 + 0.5).floor() / 10.0;
            RepoLanguage { name: name.to_string(), color: color.map(String::from), percent }
        })
        .filter(|l| l.percent >= 0.1)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(path: &str) -> Option<&'static str> {
        lang_of(path).map(|l| if l == SKIPPED { "-" } else { LANGUAGES[l as usize].0 })
    }

    #[test]
    fn languages_by_extension_and_name() {
        assert_eq!(name("src/main.rs"), Some("Rust"));
        assert_eq!(name("a/b/index.TS"), Some("TypeScript"));
        assert_eq!(name("types.d.ts"), Some("TypeScript"));
        assert_eq!(name("Dockerfile"), Some("Dockerfile"));
        assert_eq!(name("README.md"), Some("-"), "Markdown is prose in Linguist");
        assert_eq!(name("index.html"), Some("HTML"));
        assert_eq!(name("data.csv"), Some("-"), "data languages stop the lookup and aren't counted");
        assert_eq!(name("noext"), None);
        assert!(SKIP.is_match("node_modules/x.js") && SKIP.is_match("a/Cargo.lock") && SKIP.is_match("x.min.js"));
        assert!(!SKIP.is_match("src/build.rs"));
    }

    #[test]
    fn tables_are_sorted() {
        assert!(EXTENSIONS.windows(2).all(|w| w[0].0 < w[1].0));
        assert!(FILENAMES.windows(2).all(|w| w[0].0 < w[1].0));
    }
}
