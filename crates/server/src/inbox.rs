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
    let now = chrono::Local::now();
    let short: String = id.chars().filter(char::is_ascii_alphanumeric).take(8).collect();
    let short = if short.is_empty() { "chat".to_string() } else { short };
    let mut dir = config.inbox_dir.join(format!("{}_{short}", now.format("%Y-%m-%d_%H-%M-%S")));
    if dir.exists() {
        let suffix = radix36(chrono::Utc::now().timestamp_millis() as u64);
        dir = dir.with_file_name(format!("{}-{suffix}", paths::basename(&dir)));
    }
    fs::create_dir_all(&dir)?;
    Ok(paths::to_string(&dir))
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
