//! Skills across the agents (as `skills.ts`): the matrix "skill × agent" read from each
//! agent's skills folders, and copying, creating, importing (from a git repository) and
//! removing skills in the agents' user folders. A skill is a folder with SKILL.md, keyed by
//! the folder name; the format is the same for every agent, so a folder is copied as is.
//! Removed skills are moved into the hub trash (<data>/trash/skills), never deleted.
//! Project and plugin skills are only shown.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_json::{Value, json};

use crate::error::{ApiResult, HttpError};
use crate::harness::LABELS;

use super::items::{Dir, HEAD_BYTES, cut, field, is_dir, is_file, layout, names, parse_frontmatter, read_head};
use super::p;

const MAX_FILES: usize = 500;
const MAX_BYTES: u64 = 50 * 1024 * 1024;
const MAX_DEPTH: usize = 8;
const MAX_SKILL_MD: u64 = 1024 * 1024;
const IMPORT_TTL: Duration = Duration::from_secs(15 * 60);
const CLONE_TIMEOUT: Duration = Duration::from_secs(120);
const SCOPES: [&str; 3] = ["user", "project", "plugin"];

pub(super) fn name_ok(s: &str) -> bool {
    (1..=64).contains(&s.len()) && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn check_name(name: &str) -> ApiResult<()> {
    if name_ok(name) { Ok(()) } else { Err(HttpError::bad_request("name: letters, digits, - and _ only (at most 64)")) }
}

fn now_ms() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
}

// ---------- where the agents keep skills ----------

fn skill_dirs(kind: &str, project: Option<&Path>) -> Vec<Dir> {
    layout(kind, project).skills
}

/// The user skills folder the hub writes: the last one that exists, else the last one named.
fn write_dir(kind: &str) -> Option<PathBuf> {
    let dirs: Vec<PathBuf> = skill_dirs(kind, None).into_iter().filter(|(_, s)| *s == "user").map(|(d, _)| d).collect();
    dirs.iter().rev().find(|d| is_dir(d)).or(dirs.last()).cloned()
}

struct Found {
    agent: &'static str,
    scope: &'static str,
    /// The skill folder.
    dir: PathBuf,
    name: String,
}

fn scan(project: Option<&Path>) -> Vec<Found> {
    let mut out = vec![];
    for (kind, _, _) in LABELS {
        for (dir, scope) in skill_dirs(kind, project) {
            for n in names(&dir) {
                if is_file(&dir.join(&n).join("SKILL.md")) {
                    out.push(Found { agent: kind, scope, dir: dir.join(&n), name: n });
                }
            }
        }
    }
    out
}

fn rank(scope: &str) -> usize {
    SCOPES.iter().position(|s| *s == scope).unwrap_or(SCOPES.len())
}

/// The best copy: user over project over plugin, agents in registry order.
fn best<'a>(all: impl IntoIterator<Item = &'a Found>) -> Option<&'a Found> {
    let mut v: Vec<&Found> = all.into_iter().collect();
    v.sort_by_key(|f| rank(f.scope));
    v.first().copied()
}

fn description(dir: &Path) -> String {
    let (f, _) = parse_frontmatter(&read_head(&dir.join("SKILL.md"), HEAD_BYTES));
    cut(field(&f, "description"), 1000)
}

fn matrix(found: &[Found]) -> Value {
    let mut names: Vec<&str> = found.iter().map(|f| f.name.as_str()).collect();
    names.sort();
    names.dedup();
    let skills: Vec<Value> = names
        .into_iter()
        .map(|name| {
            let all: Vec<&Found> = found.iter().filter(|f| f.name == name).collect();
            let entries: Vec<Value> = all.iter().map(|f| json!({ "agent": f.agent, "scope": f.scope, "path": p(&f.dir) })).collect();
            let mut agents = serde_json::Map::new();
            for (kind, _, _) in LABELS {
                agents.insert(kind.to_string(), json!(best(all.iter().copied().filter(|f| f.agent == *kind)).map(|f| f.scope)));
            }
            json!({
                "name": name,
                "description": best(all.iter().copied()).map(|f| description(&f.dir)).unwrap_or_default(),
                "agents": agents,
                "entries": entries,
            })
        })
        .collect();
    let agents: Vec<Value> = LABELS
        .iter()
        .map(|(kind, label, _)| {
            let dir = write_dir(kind);
            json!({ "kind": kind, "label": label, "supported": dir.is_some(), "dir": dir.as_deref().map(p) })
        })
        .collect();
    json!({ "skills": skills, "agents": agents })
}

pub(super) fn skills_matrix(project: Option<&Path>) -> Value {
    matrix(&scan(project))
}

fn agent_or_404(kind: &str) -> ApiResult<(&'static str, &'static str)> {
    LABELS.iter().find(|(k, _, _)| *k == kind).map(|(k, l, _)| (*k, *l)).ok_or_else(|| HttpError::not_found("Unknown agent"))
}

// ---------- skill folders ----------

struct TreeFile {
    rel: String,
    abs: PathBuf,
    size: u64,
}

fn sorted_entries(dir: &Path) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    let mut v: Vec<String> = rd.filter_map(|e| e.ok()?.file_name().into_string().ok()).collect();
    v.sort();
    v
}

/// The files of a skill folder (sorted, `.git` skipped). Symlinks are followed only to files
/// inside the folder itself; anything else is left out. Over the limits: a 413, or with
/// `lenient` the listing is cut.
fn tree(root: &Path, lenient: bool) -> ApiResult<Vec<TreeFile>> {
    let real = std::fs::canonicalize(root)?;
    let mut out = vec![];
    let mut bytes = 0u64;
    fn walk(real: &Path, dir: &Path, rel: &str, depth: usize, lenient: bool, out: &mut Vec<TreeFile>, bytes: &mut u64) -> ApiResult<bool> {
        for n in sorted_entries(dir) {
            if n == ".git" {
                continue;
            }
            let abs = dir.join(&n);
            let r = if rel.is_empty() { n.clone() } else { format!("{rel}/{n}") };
            let Ok(st) = std::fs::symlink_metadata(&abs) else { continue };
            let mut file = abs.clone();
            if st.file_type().is_symlink() {
                let Ok(target) = std::fs::canonicalize(&abs) else { continue };
                if !target.starts_with(real) || target == real || !is_file(&target) {
                    continue;
                }
                file = target;
            } else if st.is_dir() {
                if depth < MAX_DEPTH && !walk(real, &abs, &r, depth + 1, lenient, out, bytes)? {
                    return Ok(false);
                }
                continue;
            } else if !st.is_file() {
                continue;
            }
            let size = std::fs::metadata(&file).map(|m| m.len()).unwrap_or(0);
            if out.len() >= MAX_FILES || *bytes + size > MAX_BYTES {
                if lenient {
                    return Ok(false);
                }
                return Err(HttpError::new(413, format!("The skill is too large (at most {MAX_FILES} files, 50 MB)")));
            }
            *bytes += size;
            out.push(TreeFile { rel: r, abs: file, size });
        }
        Ok(true)
    }
    walk(&real, &real, "", 0, lenient, &mut out, &mut bytes)?;
    Ok(out)
}

fn same_tree(a: &Path, b: &Path) -> bool {
    let (Ok(x), Ok(y)) = (tree(a, false), tree(b, false)) else { return false };
    x.len() == y.len()
        && x.iter().zip(&y).all(|(f, g)| {
            f.rel == g.rel && f.size == g.size && matches!((std::fs::read(&f.abs), std::fs::read(&g.abs)), (Ok(m), Ok(n)) if m == n)
        })
}

enum Src {
    File(PathBuf),
    Text(String),
}

/// Write `files` as the folder `<dir>/<name>`: into a temp folder first, then renamed into place.
fn write_skill(dir: &Path, name: &str, files: &[(String, Src)]) -> ApiResult<()> {
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".hubness-{name}-{}-{}", std::process::id(), now_ms()));
    let res = (|| -> std::io::Result<()> {
        std::fs::create_dir_all(&tmp)?;
        for (rel, src) in files {
            let dest = tmp.join(rel);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            match src {
                Src::File(f) => {
                    std::fs::copy(f, &dest)?;
                }
                Src::Text(t) => std::fs::write(&dest, t)?,
            }
        }
        std::fs::rename(&tmp, dir.join(name))
    })();
    if let Err(e) = res {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(e.into());
    }
    Ok(())
}

fn tree_files(root: &Path) -> ApiResult<Vec<(String, Src)>> {
    Ok(tree(root, false)?.into_iter().map(|f| (f.rel, Src::File(f.abs))).collect())
}

/// A copy that keeps symlinks as symlinks (moving into the trash across file systems).
fn copy_verbatim(from: &Path, to: &Path) -> std::io::Result<()> {
    let st = std::fs::symlink_metadata(from)?;
    if st.file_type().is_symlink() {
        std::os::unix::fs::symlink(std::fs::read_link(from)?, to)
    } else if st.is_dir() {
        std::fs::create_dir_all(to)?;
        for e in std::fs::read_dir(from)? {
            let e = e?;
            copy_verbatim(&e.path(), &to.join(e.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(from, to).map(|_| ())
    }
}

/// Move a skill folder (or a symlink to one) into the hub trash.
fn to_trash(data_dir: &Path, path: &Path, agent: &str, name: &str) -> ApiResult<PathBuf> {
    let dir = data_dir.join("trash").join("skills");
    std::fs::create_dir_all(&dir)?;
    let mut target = dir.join(format!("{}-{agent}-{name}", now_ms()));
    let mut i = 2;
    while target.exists() {
        target = dir.join(format!("{}-{agent}-{name}-{i}", now_ms()));
        i += 1;
    }
    if let Err(e) = std::fs::rename(path, &target) {
        if e.raw_os_error() != Some(libc::EXDEV) {
            return Err(e.into());
        }
        copy_verbatim(path, &target)?;
        if std::fs::symlink_metadata(path)?.is_dir() {
            std::fs::remove_dir_all(path)?;
        } else {
            std::fs::remove_file(path)?;
        }
    }
    Ok(target)
}

struct Target {
    agent: &'static str,
    label: &'static str,
    dir: PathBuf,
}

fn writable(kind: &str) -> ApiResult<Target> {
    let (agent, label) = agent_or_404(kind)?;
    let dir =
        write_dir(agent).ok_or_else(|| HttpError::bad_request(format!("{label}: the hub doesn't know where this agent keeps skills")))?;
    Ok(Target { agent, label, dir })
}

fn targets_of(kinds: &[String]) -> ApiResult<Vec<Target>> {
    if kinds.is_empty() {
        return Err(HttpError::bad_request("agents: pick at least one agent"));
    }
    if kinds.len() > 20 {
        return Err(HttpError::bad_request("agents: at most 20"));
    }
    let mut out: Vec<Target> = vec![];
    for k in kinds {
        if out.iter().any(|t| t.agent == k) {
            continue;
        }
        out.push(writable(k)?);
    }
    Ok(out)
}

// ---------- reading ----------

pub(super) fn skill_content(name: &str, agent: Option<&str>, project: Option<&Path>) -> ApiResult<Value> {
    check_name(name)?;
    let found = scan(project);
    let all = found.iter().filter(|f| f.name == name);
    let pick = match agent.filter(|a| !a.is_empty()) {
        Some(a) => {
            let (k, _) = agent_or_404(a)?;
            best(all.filter(|f| f.agent == k))
        }
        None => best(all),
    };
    let pick = pick.ok_or_else(|| HttpError::not_found("Skill not found"))?;
    let md = pick.dir.join("SKILL.md");
    if std::fs::metadata(&md)?.len() > MAX_SKILL_MD {
        return Err(HttpError::new(413, "SKILL.md is too large to show"));
    }
    let content = String::from_utf8_lossy(&std::fs::read(&md)?).into_owned();
    let files: Vec<Value> = tree(&pick.dir, true)?.into_iter().map(|f| json!({ "path": f.rel, "size": f.size })).collect();
    Ok(json!({ "name": name, "agent": pick.agent, "scope": pick.scope, "path": p(&pick.dir), "content": content, "files": files }))
}

// ---------- changes (user folders only) ----------

/// Copy a skill to one more agent, from the agent that has it (user copies first).
pub(super) fn skill_connect(name: &str, kind: &str, project: Option<&Path>) -> ApiResult<Value> {
    check_name(name)?;
    let found = scan(project);
    let t = writable(kind)?;
    if found.iter().any(|f| f.agent == t.agent && f.scope == "user" && f.name == name) {
        return Ok(matrix(&found));
    }
    let from = best(found.iter().filter(|f| f.name == name)).ok_or_else(|| HttpError::not_found("Skill not found"))?;
    let dest = t.dir.join(name);
    if std::fs::symlink_metadata(&dest).is_ok() {
        return Err(HttpError::new(409, format!("{} exists but is not a skill — move it away first", p(&dest))));
    }
    write_skill(&t.dir, name, &tree_files(&from.dir)?)?;
    Ok(skills_matrix(project))
}

/// Move one agent's user copy into the hub trash.
pub(super) fn skill_disconnect(data_dir: &Path, name: &str, kind: &str, project: Option<&Path>) -> ApiResult<Value> {
    check_name(name)?;
    let found = scan(project);
    let (agent, _) = agent_or_404(kind)?;
    let own: Vec<&Found> = found.iter().filter(|f| f.agent == agent && f.scope == "user" && f.name == name).collect();
    if own.is_empty() {
        if let Some(other) = found.iter().find(|f| f.agent == agent && f.name == name) {
            let from = if other.scope == "plugin" { "a plugin" } else { "the project" };
            return Err(HttpError::new(409, format!("It comes from {from} ({}) — the hub doesn't change it", p(&other.dir))));
        }
        return Err(HttpError::not_found("This agent does not have the skill"));
    }
    for f in own {
        to_trash(data_dir, &f.dir, agent, name)?;
    }
    Ok(skills_matrix(project))
}

/// Move every user copy into the hub trash.
pub(super) fn skill_delete(data_dir: &Path, name: &str, project: Option<&Path>) -> ApiResult<Value> {
    check_name(name)?;
    let own: Vec<Found> = scan(project).into_iter().filter(|f| f.scope == "user" && f.name == name).collect();
    if own.is_empty() {
        return Err(HttpError::not_found("Skill not found"));
    }
    for f in &own {
        to_trash(data_dir, &f.dir, f.agent, name)?;
    }
    Ok(skills_matrix(project))
}

fn conflict(label: &str, name: &str) -> HttpError {
    HttpError::new(
        409,
        format!("{label} already has a different skill “{name}” — pass replace to swap it (the old one goes to the hub trash)"),
    )
}

/// Put a skill folder into each target; an existing different skill of the same name is a 409
/// unless `replace` (it then goes to the trash). All targets are checked before any is written.
fn install(
    data_dir: &Path,
    name: &str,
    files: &[(String, Src)],
    source: Option<&Path>,
    targets: &[Target],
    replace: bool,
) -> ApiResult<()> {
    let mut plan: Vec<(&Target, Option<PathBuf>)> = vec![];
    for t in targets {
        let path = t.dir.join(name);
        if !path.exists() {
            plan.push((t, None));
            continue;
        }
        if let Some(src) = source
            && same_tree(src, &path)
        {
            continue;
        }
        if source.is_none()
            && let [(_, Src::Text(text))] = files
            && std::fs::read_to_string(path.join("SKILL.md")).is_ok_and(|t| t == *text)
            && tree(&path, true).is_ok_and(|f| f.len() == 1)
        {
            continue;
        }
        if !replace {
            return Err(conflict(t.label, name));
        }
        plan.push((t, Some(path)));
    }
    for (t, existing) in plan {
        if let Some(e) = existing {
            to_trash(data_dir, &e, t.agent, name)?;
        }
        write_skill(&t.dir, name, files)?;
    }
    Ok(())
}

fn skill_markdown(name: &str, description: &str, body: &str) -> String {
    let nl = if body.ends_with('\n') { "" } else { "\n" };
    format!("---\nname: {name}\ndescription: {}\n---\n\n{body}{nl}", serde_json::to_string(description).expect("json"))
}

#[derive(Deserialize)]
pub(super) struct SkillBody {
    name: String,
    description: String,
    body: String,
    agents: Vec<String>,
    replace: Option<bool>,
}

pub(super) fn skill_create(data_dir: &Path, b: SkillBody, project: Option<&Path>) -> ApiResult<Value> {
    check_name(&b.name)?;
    let description = b.description.trim();
    if description.is_empty() {
        return Err(HttpError::bad_request("description: say when the agent should use the skill"));
    }
    if description.chars().count() > 1024 {
        return Err(HttpError::bad_request("description: at most 1024 characters"));
    }
    if b.body.len() as u64 > MAX_SKILL_MD {
        return Err(HttpError::new(413, "body: at most 1 MB"));
    }
    let targets = targets_of(&b.agents)?;
    let files = vec![("SKILL.md".to_string(), Src::Text(skill_markdown(&b.name, description, &b.body)))];
    install(data_dir, &b.name, &files, None, &targets, b.replace == Some(true))?;
    Ok(skills_matrix(project))
}

// ---------- import from a git repository ----------

fn imports_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("tmp").join("skill-imports")
}

fn bad_url() -> HttpError {
    HttpError::bad_request("url: an https:// git URL or owner/repo")
}

fn repo_part_ok(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

/// owner/repo → https://github.com/owner/repo; https URLs as given (a GitHub page URL → its repo).
pub(super) fn import_url(raw: &str) -> ApiResult<String> {
    let s = raw.trim();
    if let Some((a, b)) = s.split_once('/')
        && repo_part_ok(a)
        && repo_part_ok(b)
    {
        return Ok(format!("https://github.com/{s}"));
    }
    if s.encode_utf16().count() > 2000 || s.chars().any(|c| c.is_whitespace() || c == '\0') {
        return Err(bad_url());
    }
    if let Some(rest) = s.strip_prefix("https://github.com/") {
        let parts: Vec<&str> = rest.splitn(3, '/').collect();
        if parts.len() == 3 && repo_part_ok(parts[0]) && repo_part_ok(parts[1]) && !parts[2].is_empty() {
            return Ok(format!("https://github.com/{}/{}", parts[0], parts[1]));
        }
    }
    if let Some(rest) = s.strip_prefix("https://")
        && let Some((host, path)) = rest.split_once('/')
        && !host.is_empty()
        && !path.is_empty()
    {
        return Ok(s.to_string());
    }
    if std::env::var("HUBNESS_ALLOW_FILE_GIT").as_deref() == Ok("1") && s.starts_with("file:///") && s.len() > "file:///".len() {
        return Ok(s.to_string());
    }
    Err(bad_url())
}

/// Remove clones older than the TTL.
fn sweep_imports(data_dir: &Path) {
    let dir = imports_dir(data_dir);
    for n in names(&dir) {
        let path = dir.join(n);
        let old =
            std::fs::metadata(&path).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age > IMPORT_TTL);
        if old {
            let _ = std::fs::remove_dir_all(&path);
        }
    }
}

struct Candidate {
    name: String,
    description: String,
    path: String,
    abs: PathBuf,
}

/// Skill folders in a clone (depth-first, sorted; a skill folder is not searched further).
fn find_skills(root: &Path) -> Vec<Candidate> {
    fn walk(dir: &Path, rel: &str, depth: usize, out: &mut Vec<Candidate>) {
        if out.len() >= 200 {
            return;
        }
        let md = dir.join("SKILL.md");
        if std::fs::symlink_metadata(&md).is_ok_and(|m| m.is_file()) {
            let (f, _) = parse_frontmatter(&read_head(&md, HEAD_BYTES));
            let name = if rel.is_empty() { field(&f, "name").to_string() } else { rel.rsplit('/').next().unwrap_or_default().to_string() };
            if name_ok(&name) && !out.iter().any(|c| c.name == name) {
                out.push(Candidate {
                    name,
                    description: cut(field(&f, "description"), 1000),
                    path: if rel.is_empty() { ".".into() } else { rel.to_string() },
                    abs: dir.to_path_buf(),
                });
            }
            return;
        }
        if depth >= 6 {
            return;
        }
        for n in sorted_entries(dir) {
            if n == ".git" || n == "node_modules" {
                continue;
            }
            let path = dir.join(&n);
            if std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir()) {
                walk(&path, &if rel.is_empty() { n.clone() } else { format!("{rel}/{n}") }, depth + 1, out);
            }
        }
    }
    let mut out = vec![];
    walk(root, "", 0, &mut out);
    out
}

pub(super) async fn skill_import_preview(data_dir: &Path, raw: &str) -> ApiResult<Value> {
    let url = import_url(raw)?;
    sweep_imports(data_dir);
    let id = uuid::Uuid::new_v4().to_string();
    let dir = imports_dir(data_dir).join(&id);
    std::fs::create_dir_all(&dir)?;
    let repo = dir.join("repo");
    let allow = format!("protocol.{}.allow=always", if url.starts_with("file://") { "file" } else { "https" });
    let repo_s = p(&repo);
    let args = [
        "-c",
        "core.symlinks=false",
        "-c",
        "protocol.allow=never",
        "-c",
        allow.as_str(),
        "clone",
        "--depth",
        "1",
        "--quiet",
        "--no-tags",
        "--",
        url.as_str(),
        repo_s.as_str(),
    ];
    let out = crate::git::run(&dir, &args, CLONE_TIMEOUT, &[]).await;
    if out.code != 0 {
        let _ = std::fs::remove_dir_all(&dir);
        let why = out.stderr.lines().rfind(|l| !l.trim().is_empty()).unwrap_or("failed").trim().to_string();
        return Err(HttpError::new(422, format!("git clone failed: {}", cut(&why, 300))));
    }
    let skills: Vec<Value> =
        find_skills(&repo).into_iter().map(|c| json!({ "name": c.name, "description": c.description, "path": c.path })).collect();
    if skills.is_empty() {
        let _ = std::fs::remove_dir_all(&dir);
        return Err(HttpError::new(422, "No skills found in the repository (folders with a SKILL.md)"));
    }
    Ok(json!({ "id": id, "url": url, "skills": skills }))
}

#[derive(Deserialize)]
pub(super) struct ImportBody {
    id: String,
    skills: Vec<String>,
    agents: Vec<String>,
    replace: Option<bool>,
}

pub(super) fn skill_import(data_dir: &Path, b: ImportBody, project: Option<&Path>) -> ApiResult<Value> {
    if b.id.len() != 36 || !b.id.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c) || c == '-') {
        return Err(HttpError::bad_request("id: bad import id"));
    }
    sweep_imports(data_dir);
    let dir = imports_dir(data_dir).join(&b.id);
    if !is_dir(&dir.join("repo")) {
        return Err(HttpError::not_found("The import has expired — look the repository up again"));
    }
    if b.skills.is_empty() {
        return Err(HttpError::bad_request("skills: pick at least one skill"));
    }
    let found = find_skills(&dir.join("repo"));
    let mut chosen: Vec<&Candidate> = vec![];
    for n in &b.skills {
        if chosen.iter().any(|c| c.name == *n) {
            continue;
        }
        chosen.push(found.iter().find(|f| f.name == *n).ok_or_else(|| HttpError::not_found(format!("No skill “{n}” in the repository")))?);
    }
    let targets = targets_of(&b.agents)?;
    let replace = b.replace == Some(true);
    let mut plans = vec![];
    for c in chosen {
        plans.push((c, tree_files(&c.abs)?));
    }
    // check every skill before writing any: a conflict leaves everything as it was
    if !replace {
        for (c, _) in &plans {
            for t in &targets {
                let path = t.dir.join(&c.name);
                if path.exists() && !same_tree(&c.abs, &path) {
                    return Err(conflict(t.label, &c.name));
                }
            }
        }
    }
    for (c, files) in &plans {
        install(data_dir, &c.name, files, Some(&c.abs), &targets, replace)?;
    }
    let _ = std::fs::remove_dir_all(&dir);
    Ok(skills_matrix(project))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_urls() {
        assert_eq!(import_url("anthropics/skills").ok().as_deref(), Some("https://github.com/anthropics/skills"));
        assert_eq!(import_url("https://github.com/a/b/tree/main/x").ok().as_deref(), Some("https://github.com/a/b"));
        assert_eq!(import_url("https://git.example/a.git").ok().as_deref(), Some("https://git.example/a.git"));
        assert!(import_url("http://git.example/a.git").is_err());
        assert!(import_url("ext::sh -c id").is_err());
        assert!(import_url("https://host").is_err());
    }

    #[test]
    fn markdown() {
        assert_eq!(skill_markdown("n", "say \"hi\"", "Body"), "---\nname: n\ndescription: \"say \\\"hi\\\"\"\n---\n\nBody\n");
    }
}
