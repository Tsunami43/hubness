//! The `home` system project, created at server startup (as `ensureHomeProject` in TS).

use std::fs;

use hubness_core::{Project, ProjectKind};
use rusqlite::Connection;

use crate::config::Config;
use crate::db::projects;
use crate::paths;

const HOME_AGENTS_MD: &str = "# home — system project

This folder is the hub's system project. Sessions here are about the server itself:
Docker containers, systemd units, packages and disks.

Rules for agents:
- Diagnose first: explain the cause in plain words, then propose a concrete fix.
- Never run destructive or state-changing commands (restart, stop, rm, edit configs,
  package installs) without asking the user and getting an explicit yes.
- System units and packages need root; show the exact command for the user instead.
";

/// Context files agents read next to AGENTS.md (from each agent module in the TS server).
const AGENT_CONTEXT_FILES: &[(&str, &str)] = &[("CLAUDE.md", "@AGENTS.md\n")];

pub fn ensure_home_project(db: &Connection, config: &Config) -> anyhow::Result<Project> {
    if let Some(p) = projects::system(db)? {
        return Ok(p);
    }
    let agents_md = config.home_project_dir.join("AGENTS.md");
    if !agents_md.exists() {
        fs::write(&agents_md, HOME_AGENTS_MD)?;
        for (name, content) in AGENT_CONTEXT_FILES {
            fs::write(config.home_project_dir.join(name), content)?;
        }
    }
    Ok(projects::create(db, "home", &paths::to_string(&config.home_project_dir), ProjectKind::System)?)
}
