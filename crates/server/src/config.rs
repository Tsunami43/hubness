//! Hub settings from `HUBNESS_*` environment variables (same names and defaults as the
//! TypeScript server, so both can run on the same data folder).

use std::path::{Path, PathBuf};

use crate::paths::resolve;

#[derive(Debug, Clone)]
pub struct Config {
    pub port: u16,
    pub host: String,
    pub data_dir: PathBuf,
    pub db_path: PathBuf,
    pub logs_dir: PathBuf,
    pub chats_dir: PathBuf,
    /// Hub working folder: inbox for new chats, projects, home — the system project.
    pub hub_home: PathBuf,
    pub inbox_dir: PathBuf,
    pub projects_dir: PathBuf,
    pub home_project_dir: PathBuf,
    /// The built web app (`pnpm build`), served from the same origin.
    pub web_dist: PathBuf,
}

impl Config {
    /// Read the settings. `.env` in the current folder is loaded first (it never overrides
    /// variables that are already set). Relative defaults resolve against the current folder,
    /// which is the repository root when started via `cargo run` there.
    pub fn from_env() -> anyhow::Result<Self> {
        let _ = dotenvy::dotenv();
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let port = match var("HUBNESS_PORT") {
            Some(p) => p.parse().map_err(|_| anyhow::anyhow!("HUBNESS_PORT must be a port number"))?,
            None => 8787,
        };
        let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("/"));
        Ok(Self::new(
            port,
            var("HUBNESS_HOST").unwrap_or_else(|| "127.0.0.1".into()),
            &resolve(&var("HUBNESS_DATA_DIR").unwrap_or_else(|| ".data".into())),
            &var("HUBNESS_HOME").map(|h| resolve(&h)).unwrap_or_else(|| home.join("hub")),
            &resolve(&var("HUBNESS_WEB_DIST").unwrap_or_else(|| "apps/web/dist".into())),
        ))
    }

    pub fn new(port: u16, host: String, data_dir: &Path, hub_home: &Path, web_dist: &Path) -> Self {
        Self {
            port,
            host,
            data_dir: data_dir.to_path_buf(),
            db_path: data_dir.join("hubness.sqlite"),
            logs_dir: data_dir.join("logs"),
            chats_dir: data_dir.join("chats"),
            hub_home: hub_home.to_path_buf(),
            inbox_dir: hub_home.join("inbox"),
            projects_dir: hub_home.join("projects"),
            home_project_dir: hub_home.join("home"),
            web_dist: web_dist.to_path_buf(),
        }
    }

    /// Create the folders the hub writes to.
    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        for d in [&self.data_dir, &self.logs_dir, &self.chats_dir, &self.inbox_dir, &self.projects_dir, &self.home_project_dir] {
            std::fs::create_dir_all(d)?;
        }
        Ok(())
    }

    /// Folder with the symlink workspaces of groups (local workspace provider).
    pub fn groups_dir(&self) -> PathBuf {
        self.data_dir.join("groups")
    }
}

/// Environment for child processes (agents, Run, Shell, CLI): without hub settings and
/// secrets (`HUBNESS_*`, including the secrets key) and without the internal pnpm/npm
/// variables the hub may have been started with.
pub fn child_env() -> Vec<(String, String)> {
    std::env::vars()
        .filter(|(k, _)| {
            !k.starts_with("HUBNESS_") && !k.starts_with("npm_") && !k.starts_with("PNPM_") && !k.starts_with("pnpm_") && k != "INIT_CWD"
        })
        .collect()
}
