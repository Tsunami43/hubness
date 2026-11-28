//! Inbox: folders of plain chats (`~/hub/inbox/<date>_<id>`). Moving a chat to a project
//! (`inbox.ts` convert/suggestion) is not ported yet.

use std::fs;

use crate::config::Config;
use crate::error::ApiResult;
use crate::paths;

/// Folder of a new chat: creation time (server local time, sorts chronologically) plus the
/// start of the session id, e.g. `2026-11-06_15-09-30_4333b6d2`. Not the message text: a
/// title in a path is unreadable after transliteration and leaks the topic.
pub fn inbox_dir_for(config: &Config, id: &str) -> ApiResult<String> {
    let now = jiff::ZonedDateTime::now();
    let short: String = id.chars().filter(char::is_ascii_alphanumeric).take(8).collect();
    let short = if short.is_empty() { "chat".to_string() } else { short };
    let mut dir = config.inbox_dir.join(format!("{}_{short}", now.strftime("%Y-%m-%d_%H-%M-%S")));
    if dir.exists() {
        let suffix = radix36(jiff::Timestamp::now().as_millisecond() as u64);
        dir = dir.with_file_name(format!("{}-{suffix}", paths::basename(&dir)));
    }
    fs::create_dir_all(&dir)?;
    Ok(paths::to_string(&dir))
}

/// The folder of a regular chat that deleting the chat may take along (the user asks for it):
/// its own Inbox folder, or the hub's folder of a chat from before Inbox. None for a project
/// or group session, for anything that is not directly inside those folders, and for a folder
/// another session works in too.
pub fn chat_folder_of(db: &rusqlite::Connection, config: &Config, s: &hubness_core::Session) -> ApiResult<Option<std::path::PathBuf>> {
    use crate::db::sessions::{self as repo, Archived, Filter};
    if s.project_id.is_some() || s.group_id.is_some() {
        return Ok(None);
    }
    let abs = |p: &std::path::Path| paths::normalize(&std::env::current_dir().unwrap_or_default().join(p));
    let dir = abs(&s.cwd.as_ref().map(std::path::PathBuf::from).unwrap_or_else(|| config.chats_dir.join(&s.id)));
    let parent = dir.parent().map(std::path::Path::to_path_buf);
    if parent != Some(abs(&config.inbox_dir)) && parent != Some(abs(&config.chats_dir)) {
        return Ok(None);
    }
    let all = repo::list(db, Filter { chats: true, archived: Archived::All, ..Default::default() })?;
    let shared = all.iter().any(|o| o.id != s.id && o.cwd.as_ref().is_some_and(|c| abs(std::path::Path::new(c)) == dir));
    Ok(if shared { None } else { Some(dir) })
}

/// Like JavaScript's `n.toString(36)`.
fn radix36(mut n: u64) -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out = vec![];
    loop {
        out.push(DIGITS[(n % 36) as usize]);
        n /= 36;
        if n == 0 {
            break;
        }
    }
    out.reverse();
    String::from_utf8(out).expect("ascii")
}

#[cfg(test)]
mod tests {
    use super::radix36;

    #[test]
    fn base36_like_javascript() {
        assert_eq!(radix36(0), "0");
        assert_eq!(radix36(35), "z");
        assert_eq!(radix36(1_764_000_000_000), "midc1z40");
    }
}

// ---------- Inbox → Project ----------

/// Cyrillic → Latin, so folder names stay ASCII (shells, globs, Docker Compose names).
fn translit(c: char) -> Option<&'static str> {
    const FROM: &str = "абвгдеёжзийклмнопрстуфхцчшщъыьэюяіїєґ";
    const TO: [&str; 37] = [
        "a", "b", "v", "g", "d", "e", "e", "zh", "z", "i", "y", "k", "l", "m", "n", "o", "p", "r", "s", "t", "u", "f", "kh", "ts", "ch",
        "sh", "shch", "", "y", "", "e", "yu", "ya", "i", "yi", "ye", "g",
    ];
    FROM.chars().position(|x| x == c).map(|i| TO[i])
}

/// A folder name from a title: transliterated, lowercase, `-`-separated, at most 40 chars cut
/// at a word boundary (unless the first word alone is longer); `chat` when nothing is left.
pub fn slug(s: &str) -> String {
    let lower = s.to_lowercase();
    let mut latin = String::new();
    for c in lower.chars() {
        match translit(c) {
            Some(t) => latin.push_str(t),
            None => latin.push(c),
        }
    }
    // NFKD + dropping combining marks, for the common Latin letters with diacritics
    let ascii: String = latin.chars().map(|c| deaccent(c).unwrap_or(c)).collect();
    let mut out = String::new();
    let mut dash = false;
    for c in ascii.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            if dash && !out.is_empty() {
                out.push('-');
            }
            dash = false;
            out.push(c);
        } else {
            dash = true;
        }
    }
    if out.len() > 40 {
        let head = &out[..40];
        out = match head.rfind('-') {
            // the first word alone is longer than 40: cut it
            None => head.to_string(),
            Some(_) if out.as_bytes()[40] == b'-' => head.to_string(),
            Some(i) => head[..i].to_string(),
        };
    }
    if out.is_empty() { "chat".into() } else { out }
}

fn deaccent(c: char) -> Option<char> {
    const PAIRS: &[(&str, char)] = &[
        ("àáâãäåāă", 'a'),
        ("çćč", 'c'),
        ("ďđ", 'd'),
        ("èéêëēěę", 'e'),
        ("ìíîïī", 'i'),
        ("ñńň", 'n'),
        ("òóôõöō", 'o'),
        ("řŕ", 'r'),
        ("śšş", 's'),
        ("ťţ", 't'),
        ("ùúûüūů", 'u'),
        ("ýÿ", 'y'),
        ("źżž", 'z'),
    ];
    PAIRS.iter().find(|(set, _)| set.contains(c)).map(|(_, r)| *r)
}

const CODE_EXT: &[&str] = &[
    "ts", "tsx", "js", "jsx", "mjs", "py", "rs", "go", "java", "kt", "rb", "php", "c", "cc", "cpp", "h", "cs", "swift", "vue", "svelte",
    "html", "css", "scss", "sh", "sql",
];
const MARKERS: &[&str] = &[
    "package.json",
    "Cargo.toml",
    "pyproject.toml",
    "requirements.txt",
    "go.mod",
    "pom.xml",
    "build.gradle",
    "Makefile",
    "docker-compose.yml",
    "compose.yaml",
    "Dockerfile",
    ".git",
];

fn count_code(dir: &std::path::Path, depth: usize) -> usize {
    if depth > 3 {
        return 0;
    }
    let Ok(rd) = fs::read_dir(dir) else { return 0 };
    let mut n = 0;
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name == "node_modules" || name.starts_with('.') {
            continue;
        }
        let p = dir.join(&name);
        if p.is_dir() {
            n += count_code(&p, depth + 1);
        } else if std::path::Path::new(&name).extension().is_some_and(|x| CODE_EXT.contains(&x.to_string_lossy().as_ref())) {
            n += 1;
        }
        if n > 50 {
            break;
        }
    }
    n
}

/// Offer to move a chat into a project when code appeared in its folder.
pub fn suggestion(s: &hubness_core::Session) -> hubness_core::ProjectSuggestion {
    let name = slug(&s.title);
    let none = |name: String| hubness_core::ProjectSuggestion { suggest: false, reason: None, name };
    let Some(cwd) = s.cwd.as_deref().filter(|c| s.project_id.is_none() && std::path::Path::new(c).exists()) else { return none(name) };
    let dir = std::path::Path::new(cwd);
    if let Some(m) = MARKERS.iter().find(|m| dir.join(m).exists()) {
        return hubness_core::ProjectSuggestion { suggest: true, reason: Some(format!("found {m}")), name };
    }
    let n = count_code(dir, 0);
    if n >= 2 {
        return hubness_core::ProjectSuggestion { suggest: true, reason: Some(format!("{n} source files")), name };
    }
    none(name)
}

/// Short conversation summary — passed to the agent if it can't continue the session in the
/// new folder.
pub fn handoff_text(events: &[serde_json::Value], from: &str, to: &str) -> String {
    let recent: Vec<String> = events
        .iter()
        .filter(|e| e["kind"] == "user" || e["kind"] == "text")
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .take(12)
        .rev()
        .map(|e| {
            let who = if e["kind"] == "user" { "User" } else { "Assistant" };
            format!("{who}: {}", crate::sessions::truncate(e["text"].as_str().unwrap_or_default(), 1500))
        })
        .collect();
    format!(
        "This conversation was moved from the inbox folder {from} into the project folder {to}. All files are now in the current directory. Recent conversation for context:\n\n{}\n\n---\n",
        recent.join("\n\n")
    )
}

#[cfg(test)]
mod slug_tests {
    use super::slug;

    #[test]
    fn slugs_like_the_ts_server() {
        assert_eq!(slug("Привет, мир!"), "privet-mir");
        assert_eq!(slug("  Fix the Café bug  "), "fix-the-cafe-bug");
        assert_eq!(slug("!!!"), "chat");
        assert_eq!(slug("one two three four five six seven eight nine"), "one-two-three-four-five-six-seven-eight");
        assert_eq!(slug(&"a".repeat(50)), "a".repeat(40));
    }
}
