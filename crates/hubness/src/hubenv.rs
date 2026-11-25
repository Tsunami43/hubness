//! The service settings file `$XDG_CONFIG_HOME/hubness/hub.env` and where things live.
//!
//! Both builds read it: the full one manages the service with it (`hubness server ...`), and
//! `hubness webapp` takes the hub's URL from it.

// the client build only reads the file
#![cfg_attr(not(feature = "server"), allow(dead_code))]

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Context;

pub const UNIT: &str = "hubness.service";
pub const DEFAULT_HOST: &str = "127.0.0.1";
pub const DEFAULT_PORT: u16 = 8787;

// ---------- paths ----------

pub struct Paths {
    pub env: PathBuf,
    pub unit: PathBuf,
    pub default_data: PathBuf,
    pub home: PathBuf,
}

impl Paths {
    pub fn from_env() -> anyhow::Result<Self> {
        let home = std::env::var_os("HOME").filter(|h| !h.is_empty()).map(PathBuf::from).context("HOME is not set")?;
        let xdg = |var: &str, fallback: &str| {
            std::env::var_os(var)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .unwrap_or_else(|| home.join(fallback))
        };
        let config = xdg("XDG_CONFIG_HOME", ".config");
        let data = xdg("XDG_DATA_HOME", ".local/share");
        Ok(Self {
            env: config.join("hubness").join("hub.env"),
            unit: config.join("systemd").join("user").join(UNIT),
            default_data: data.join("hubness"),
            home,
        })
    }
}

/// The data folder `hubness user …` works on when HUBNESS_DATA_DIR is not set: the
/// service's one, once `hubness server start` (or `config`) wrote hub.env.
pub fn service_data_dir() -> Option<PathBuf> {
    let paths = Paths::from_env().ok()?;
    if !paths.env.exists() {
        return None;
    }
    EnvFile::load(&paths.env).ok().and_then(|e| e.get("HUBNESS_DATA_DIR").map(PathBuf::from))
}

// ---------- hub.env ----------

/// A systemd `EnvironmentFile`, edited line by line so comments and extra settings survive.
pub struct EnvFile {
    lines: Vec<String>,
}

impl EnvFile {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| format!("can't read {}", path.display()))?;
        Ok(Self { lines: text.lines().map(String::from).collect() })
    }

    pub fn defaults(paths: &Paths) -> Self {
        let mut env = Self {
            lines: [
                "# hubness service settings (`hubness server config` edits them; restart to apply).",
                "# HUBNESS_HOST other than 127.0.0.1 makes the hub reachable from other machines:",
                "# only with accounts, and behind HTTPS or a VPN.",
            ]
            .map(String::from)
            .to_vec(),
        };
        env.set("HUBNESS_HOST", DEFAULT_HOST);
        env.set("HUBNESS_PORT", &DEFAULT_PORT.to_string());
        env.set("HUBNESS_DATA_DIR", &paths.default_data.to_string_lossy());
        env.lines.push("# Agents and run commands see this PATH (`hubness server start` adds new folders).".into());
        env.set("PATH", &std::env::var("PATH").unwrap_or_default());
        env
    }

    pub fn load_or_default(paths: &Paths) -> anyhow::Result<Self> {
        if paths.env.exists() { Self::load(&paths.env) } else { Ok(Self::defaults(paths)) }
    }

    pub fn get(&self, key: &str) -> Option<String> {
        self.lines.iter().rev().find_map(|l| parse_line(l).filter(|(k, _)| k == key).map(|(_, v)| v)).filter(|v| !v.is_empty())
    }

    pub fn set(&mut self, key: &str, value: &str) {
        let line = format!("{key}={}", quote(value));
        match self.lines.iter().rposition(|l| parse_line(l).is_some_and(|(k, _)| k == key)) {
            Some(i) => self.lines[i] = line,
            None => self.lines.push(line),
        }
    }

    pub fn host(&self) -> String {
        self.get("HUBNESS_HOST").unwrap_or_else(|| DEFAULT_HOST.into())
    }

    pub fn port(&self) -> u16 {
        self.get("HUBNESS_PORT").and_then(|p| p.parse().ok()).unwrap_or(DEFAULT_PORT)
    }

    pub fn data_dir(&self, paths: &Paths) -> PathBuf {
        self.get("HUBNESS_DATA_DIR").map(PathBuf::from).unwrap_or_else(|| paths.default_data.clone())
    }

    /// Add the folders of the current PATH the snapshot doesn't have yet (newly installed
    /// agent CLIs); folders the user put there stay first.
    pub fn merge_path(&mut self) {
        let current = std::env::var("PATH").unwrap_or_default();
        let mut dirs: Vec<String> = self.get("PATH").unwrap_or_default().split(':').filter(|d| !d.is_empty()).map(String::from).collect();
        for d in current.split(':').filter(|d| !d.is_empty()) {
            if !dirs.iter().any(|x| x == d) {
                dirs.push(d.to_string());
            }
        }
        self.set("PATH", &dirs.join(":"));
    }

    /// Write for the owner only (it may hold more settings later): 0600, folder 0700.
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
        if let Some(dir) = path.parent() {
            std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
        }
        let tmp = path.with_extension("env.tmp");
        let _ = std::fs::remove_file(&tmp);
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
        f.write_all((self.lines.join("\n") + "\n").as_bytes())?;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
        std::fs::rename(&tmp, path).with_context(|| format!("can't write {}", path.display()))
    }
}

fn parse_line(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    if line.starts_with('#') || line.starts_with(';') {
        return None;
    }
    let (k, v) = line.split_once('=')?;
    let v = v.trim();
    let v = if v.len() >= 2 && ((v.starts_with('"') && v.ends_with('"')) || (v.starts_with('\'') && v.ends_with('\''))) {
        let inner = &v[1..v.len() - 1];
        if v.starts_with('"') { inner.replace("\\\"", "\"").replace("\\\\", "\\") } else { inner.to_string() }
    } else {
        v.to_string()
    };
    Some((k.trim().to_string(), v))
}

fn quote(v: &str) -> String {
    if !v.is_empty() && v.chars().all(|c| c.is_ascii_alphanumeric() || "/._-:+@%,".contains(c)) {
        v.to_string()
    } else {
        format!("\"{}\"", v.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

/// Where this machine reaches the hub (`0.0.0.0` and `::` mean every interface).
pub fn local_url(env: &EnvFile) -> String {
    let host = env.host();
    let host = match host.as_str() {
        "0.0.0.0" | "" => "127.0.0.1".to_string(),
        "::" | "[::]" => "[::1]".to_string(),
        h if h.contains(':') && !h.starts_with('[') => format!("[{h}]"),
        h => h.to_string(),
    };
    format!("http://{host}:{}", env.port())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(dir: &Path) -> Paths {
        Paths {
            env: dir.join("config/hubness/hub.env"),
            unit: dir.join("config/systemd/user/hubness.service"),
            default_data: dir.join("data/hubness"),
            home: dir.to_path_buf(),
        }
    }

    #[test]
    fn env_file_keeps_comments_and_quotes_values() {
        let dir = std::env::temp_dir().join(format!("hubness-env-{}", std::process::id()));
        let p = paths(&dir);
        let mut env = EnvFile::defaults(&p);
        assert_eq!(env.host(), "127.0.0.1");
        assert_eq!(env.port(), 8787);
        assert_eq!(env.data_dir(&p), dir.join("data/hubness"));
        env.lines.push("HUBNESS_PREVIEW_PORTS=5173".into());
        env.set("HUBNESS_DATA_DIR", "/srv/hub data/\"x\"");
        env.set("HUBNESS_PORT", "9000");
        env.save(&p.env).unwrap();
        let back = EnvFile::load(&p.env).unwrap();
        assert_eq!(back.get("HUBNESS_DATA_DIR").as_deref(), Some("/srv/hub data/\"x\""));
        assert_eq!(back.port(), 9000);
        assert_eq!(back.get("HUBNESS_PREVIEW_PORTS").as_deref(), Some("5173"));
        assert!(back.lines[0].starts_with('#'));
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&p.env).unwrap().permissions().mode() & 0o777, 0o600);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn parse_systemd_lines() {
        assert_eq!(parse_line("PATH=\"/a b:/c\""), Some(("PATH".into(), "/a b:/c".into())));
        assert_eq!(parse_line(" X = 'y' "), Some(("X".into(), "y".into())));
        assert_eq!(parse_line("# HUBNESS_HOME=x"), None);
        assert_eq!(quote("/usr/bin:/bin"), "/usr/bin:/bin");
        assert_eq!(quote("a b"), "\"a b\"");
    }

    #[test]
    fn urls_for_this_machine() {
        let mut env = EnvFile { lines: vec![] };
        assert_eq!(local_url(&env), "http://127.0.0.1:8787");
        env.set("HUBNESS_HOST", "0.0.0.0");
        env.set("HUBNESS_PORT", "9001");
        assert_eq!(local_url(&env), "http://127.0.0.1:9001");
        env.set("HUBNESS_HOST", "::1");
        assert_eq!(local_url(&env), "http://[::1]:9001");
    }
}
