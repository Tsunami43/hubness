//! What else the agents read from their folders: skills, rules files, subagents, commands
//! and plugins.

use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::error::{ApiResult, HttpError};

use super::*;

pub(super) const HEAD_BYTES: u64 = 64 * 1024;
pub(super) const MAX_ENTRIES: usize = 500;
pub(super) const MAX_RULES: u64 = 1024 * 1024;

/// The first `max` bytes of a file as text ('' when unreadable).
pub(super) fn read_head(path: &Path, max: u64) -> String {
    let mut buf = vec![];
    match std::fs::File::open(path) {
        Ok(f) => {
            let _ = f.take(max).read_to_end(&mut buf);
            String::from_utf8_lossy(&buf).into_owned()
        }
        Err(_) => String::new(),
    }
}

pub(super) fn cut(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

pub(super) fn unquote(v: &str) -> String {
    if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') {
        return serde_json::from_str::<String>(v).unwrap_or_else(|_| v[1..v.len() - 1].to_string());
    }
    if v.len() >= 2 && v.starts_with('\'') && v.ends_with('\'') {
        return v[1..v.len() - 1].replace("''", "'");
    }
    v.to_string()
}

/// YAML frontmatter, the small subset agents use: top-level `key: value` with plain or
/// quoted values, plain values continued on indented lines, and `>` / `|` block scalars.
/// Nested maps and lists are skipped. Never fails.
pub fn parse_frontmatter(text: &str) -> (Vec<(String, String)>, String) {
    use std::sync::OnceLock;
    static KEY: OnceLock<regex::Regex> = OnceLock::new();
    static BLOCK: OnceLock<regex::Regex> = OnceLock::new();
    let key_re = KEY.get_or_init(|| regex::Regex::new(r"^([A-Za-z0-9_-]+)\s*:(?:\s+(.*))?$").expect("regex"));
    let block_re = BLOCK.get_or_init(|| regex::Regex::new(r"^[>|][-+0-9]*$").expect("regex"));
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let lines: Vec<&str> = text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).collect();
    if lines.first().map(|l| l.trim()) != Some("---") {
        return (vec![], text.to_string());
    }
    let Some(end) = lines.iter().enumerate().skip(1).find(|(_, l)| l.trim() == "---" || l.trim() == "...").map(|(i, _)| i) else {
        return (vec![], text.to_string());
    };
    let indented = |l: &str| l.starts_with(' ') || l.starts_with('\t');
    let mut fields: Vec<(String, String)> = vec![];
    let mut i = 1;
    while i < end {
        let caps = key_re.captures(lines[i]);
        i += 1;
        let Some(caps) = caps else { continue };
        let key = caps[1].to_string();
        let mut val = caps.get(2).map(|m| m.as_str()).unwrap_or_default().trim().to_string();
        let mut more: Vec<&str> = vec![];
        while i < end && (indented(lines[i]) || lines[i].trim().is_empty()) {
            more.push(lines[i]);
            i += 1;
        }
        while more.last().is_some_and(|l| l.trim().is_empty()) {
            more.pop();
        }
        if block_re.is_match(&val) {
            let parts: Vec<&str> = more.iter().map(|l| l.trim()).collect();
            val = if val.starts_with('>') {
                parts.into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join(" ")
            } else {
                parts.join("\n")
            };
        } else if val.starts_with('"') || val.starts_with('\'') {
            let mut parts = vec![val.as_str()];
            parts.extend(more.iter().map(|l| l.trim()));
            val = unquote(&parts.into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join(" "));
        } else if !val.is_empty() {
            let mut parts = vec![val.as_str()];
            parts.extend(more.iter().map(|l| l.trim()));
            let mut joined = parts.into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join(" ");
            if let Some(h) = joined.find(" #") {
                joined.truncate(h);
            }
            val = joined;
        }
        let val = val.trim().to_string();
        match fields.iter_mut().find(|(k, _)| *k == key) {
            Some(f) => f.1 = val,
            None => fields.push((key, val)),
        }
    }
    (fields, lines[end + 1..].join("\n"))
}

pub(super) fn field<'a>(fields: &'a [(String, String)], key: &str) -> &'a str {
    fields.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str()).unwrap_or_default()
}

/// The first meaningful line of a markdown body (a command without a description).
pub(super) fn first_line(body: &str) -> String {
    for l in body.split('\n') {
        let s = l.trim_start_matches('#').trim();
        if !s.is_empty() {
            return cut(s, 200);
        }
    }
    String::new()
}

pub(super) fn names(dir: &Path) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    let mut out: Vec<String> = rd.filter_map(|e| e.ok()?.file_name().into_string().ok()).filter(|n| !n.starts_with('.')).collect();
    out.sort();
    out.truncate(MAX_ENTRIES);
    out
}

pub(super) fn is_file(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_file())
}

pub(super) fn is_dir(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_dir())
}

/// A folder of extensions and its scope.
pub(super) type Dir = (PathBuf, &'static str);

/// A rules file: label, path, scope, listed only when present.
pub(super) struct Rules {
    label: &'static str,
    path: PathBuf,
    scope: &'static str,
    if_exists: bool,
}

#[derive(Default)]
pub(super) struct Layout {
    skills: Vec<Dir>,
    rules: Vec<Rules>,
    agents: Vec<Dir>,
    commands: Vec<Dir>,
    /// From the agent's own config: agents listed in it, plugins and their skills.
    more_agents: Vec<Value>,
    plugins: Vec<Value>,
}

pub(super) fn rules(label: &'static str, path: PathBuf, scope: &'static str) -> Rules {
    Rules { label, path, scope, if_exists: false }
}

pub(super) fn layout(kind: &str, project: Option<&Path>) -> Layout {
    let mut l = Layout::default();
    match kind {
        CLAUDE => {
            let base = claude_dir();
            for (dirs, name) in [(&mut l.skills, "skills"), (&mut l.agents, "agents"), (&mut l.commands, "commands")] {
                dirs.push((base.join(name), "user"));
                if let Some(pr) = project {
                    dirs.push((pr.join(".claude").join(name), "project"));
                }
            }
            l.rules.push(rules("CLAUDE.md", base.join("CLAUDE.md"), "user"));
            if let Some(pr) = project {
                l.rules.push(rules("CLAUDE.md", pr.join("CLAUDE.md"), "project"));
                l.rules.push(rules("CLAUDE.local.md", pr.join("CLAUDE.local.md"), "project"));
            }
            // installed plugins (plugins/installed_plugins.json, v1 or v2) and their skills
            let enabled =
                crate::harness::read_json(&base.join("settings.json")).map(|s| s["enabledPlugins"].clone()).unwrap_or(Value::Null);
            if let Some(installed) = crate::harness::read_json(&base.join("plugins").join("installed_plugins.json"))
                .and_then(|j| j.get("plugins").and_then(Value::as_object).cloned())
            {
                for (id, v) in installed {
                    let rec = if v.is_array() { v[0].clone() } else { v };
                    let Some(install) = rec["installPath"].as_str() else { continue };
                    l.plugins.push(json!({
                        "name": id, "version": rec["version"].as_str(), "enabled": enabled[&id] == true, "path": install, "scope": "plugin",
                    }));
                    l.skills.push((PathBuf::from(install).join("skills"), "plugin"));
                }
            }
        }
        CODEX => {
            let base = codex_home();
            l.skills.push((base.join("skills"), "user"));
            if let Some(pr) = project {
                l.skills.push((pr.join(".codex").join("skills"), "project"));
                l.skills.push((pr.join(".agents").join("skills"), "project"));
            }
            l.rules.push(rules("AGENTS.md", base.join("AGENTS.md"), "user"));
            l.rules.push(Rules { label: "AGENTS.override.md", path: base.join("AGENTS.override.md"), scope: "user", if_exists: true });
            if let Some(pr) = project {
                l.rules.push(rules("AGENTS.md", pr.join("AGENTS.md"), "project"));
            }
            l.commands.push((base.join("prompts"), "user"));
        }
        OPENCODE => {
            let base = opencode_dir();
            for (dirs, name) in [(&mut l.skills, "skill"), (&mut l.agents, "agent"), (&mut l.commands, "command")] {
                dirs.push((base.join(name), "user"));
                dirs.push((base.join(format!("{name}s")), "user"));
                if let Some(pr) = project {
                    dirs.push((pr.join(".opencode").join(name), "project"));
                    dirs.push((pr.join(".opencode").join(format!("{name}s")), "project"));
                }
            }
            l.rules.push(rules("AGENTS.md", base.join("AGENTS.md"), "user"));
            if let Some(pr) = project {
                l.rules.push(rules("AGENTS.md", pr.join("AGENTS.md"), "project"));
            }
            // agents and plugins listed in the config itself
            let file = opencode_file();
            let cfg = crate::harness::read_json(&file).unwrap_or(Value::Null);
            if let Some(agents) = cfg["agent"].as_object() {
                for (name, a) in agents {
                    if !a.is_object() {
                        continue;
                    }
                    l.more_agents.push(json!({
                        "name": name,
                        "description": a["description"].as_str().map(|d| cut(d, 1000)).unwrap_or_default(),
                        "model": a["model"].as_str(),
                        "path": p(&file),
                        "scope": "user",
                    }));
                }
            }
            for name in cfg["plugin"].as_array().into_iter().flatten().filter_map(Value::as_str) {
                l.plugins.push(json!({ "name": name, "version": null, "enabled": true, "path": p(&file), "scope": "user" }));
            }
        }
        _ => {
            let base = grok_home();
            for (dirs, name) in [(&mut l.skills, "skills"), (&mut l.agents, "agents"), (&mut l.commands, "commands")] {
                dirs.push((base.join(name), "user"));
                if let Some(pr) = project {
                    dirs.push((pr.join(".grok").join(name), "project"));
                }
            }
            l.rules.push(rules("AGENTS.md", base.join("AGENTS.md"), "user"));
            if let Some(pr) = project {
                l.rules.push(rules("AGENTS.md", pr.join("AGENTS.md"), "project"));
            }
        }
    }
    l
}

pub(super) fn skills_in((dir, scope): &Dir) -> Vec<Value> {
    names(dir)
        .into_iter()
        .filter_map(|n| {
            let path = dir.join(&n).join("SKILL.md");
            if !is_file(&path) {
                return None;
            }
            let (f, _) = parse_frontmatter(&read_head(&path, HEAD_BYTES));
            let name = Some(field(&f, "name")).filter(|s| !s.is_empty()).unwrap_or(&n);
            Some(json!({ "name": name, "description": cut(field(&f, "description"), 1000), "path": p(&path), "scope": scope }))
        })
        .collect()
}

pub(super) fn agents_in((dir, scope): &Dir) -> Vec<Value> {
    names(dir)
        .into_iter()
        .filter_map(|n| {
            let path = dir.join(&n);
            let stem = n.strip_suffix(".md")?;
            if !is_file(&path) {
                return None;
            }
            let (f, _) = parse_frontmatter(&read_head(&path, HEAD_BYTES));
            let name = Some(field(&f, "name")).filter(|s| !s.is_empty()).unwrap_or(stem);
            let model = Some(field(&f, "model")).filter(|s| !s.is_empty());
            Some(json!({ "name": name, "description": cut(field(&f, "description"), 1000), "model": model, "path": p(&path), "scope": scope }))
        })
        .collect()
}

pub(super) fn commands_in((dir, scope): &Dir) -> Vec<Value> {
    let one = |path: &Path, name: String| {
        let (f, body) = parse_frontmatter(&read_head(path, HEAD_BYTES));
        let d = field(&f, "description");
        let description = if d.is_empty() { first_line(&body) } else { d.to_string() };
        json!({ "name": name, "description": cut(&description, 1000), "path": p(path), "scope": scope })
    };
    let mut out = vec![];
    for n in names(dir) {
        let path = dir.join(&n);
        if let Some(stem) = n.strip_suffix(".md")
            && is_file(&path)
        {
            out.push(one(&path, stem.to_string()));
        } else if is_dir(&path) {
            // a sub-folder is a namespace: frontend/review.md → frontend:review
            for m in names(&path) {
                if let Some(stem) = m.strip_suffix(".md")
                    && is_file(&path.join(&m))
                {
                    out.push(one(&path.join(&m), format!("{n}:{stem}")));
                }
            }
        }
    }
    out
}

pub(super) fn rules_of(l: &Layout) -> Vec<Value> {
    l.rules
        .iter()
        .filter_map(|r| {
            let exists = is_file(&r.path);
            if r.if_exists && !exists {
                return None;
            }
            let size = if exists { std::fs::metadata(&r.path).map(|m| m.len()).unwrap_or(0) } else { 0 };
            let preview =
                if exists { Some(cut(&read_head(&r.path, 4096), 300).trim_end().to_string()).filter(|s| !s.is_empty()) } else { None };
            Some(json!({ "label": r.label, "path": p(&r.path), "scope": r.scope, "exists": exists, "size": size, "preview": preview }))
        })
        .collect()
}

pub(super) fn extensions_of(kind: &str, project: Option<&Path>) -> Value {
    let l = layout(kind, project);
    let mut agents: Vec<Value> = l.agents.iter().flat_map(agents_in).collect();
    agents.extend(l.more_agents.iter().cloned());
    json!({
        "kind": kind,
        "skills": l.skills.iter().flat_map(skills_in).collect::<Vec<_>>(),
        "rules": rules_of(&l),
        "agents": agents,
        "commands": l.commands.iter().flat_map(commands_in).collect::<Vec<_>>(),
        "plugins": l.plugins,
    })
}

/// Only a rules file the listing names can be opened or saved — never an arbitrary path.
pub(super) fn listed_rules(kind: &str, project: Option<&Path>, path: Option<&str>) -> ApiResult<PathBuf> {
    let path = path.filter(|p| !p.is_empty()).ok_or_else(|| HttpError::bad_request("path is required"))?;
    let l = layout(kind, project);
    rules_of(&l)
        .iter()
        .find(|r| r["path"] == path)
        .map(|_| PathBuf::from(path))
        .ok_or_else(|| HttpError::new(403, "Not a rules file of this agent"))
}
