//! Run (like Run/Workflows in Replit): named project commands, each with one run at a time,
//! output in the hub terminal, app port → preview. Shell — an interactive shell of a project
//! or group; lives until it is closed or nobody watches it for ~30 minutes; reconnect by
//! task id. Processes run in the project environment (environments.rs): on the server,
//! sandboxed or in a container; project secrets are in the environment.
//!
//! A port of `runs.ts` and the run, shell and preview routes of `routes/environment.ts`.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use hubness_core::{JobInfo, JobStatus, Project, RunConfig, RunState, RunSuggestion, RuntimeEnvironment};
use regex::Regex;
use rusqlite::{Connection, OptionalExtension, Row};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::config::Config;
use crate::environments::{self, Scope};
use crate::error::{ApiResult, HttpError};
use crate::http::{JsonBody, LenientBody, double_option};
use crate::preview::{self, Target};
use crate::terminal::{self, JobOpts};
use crate::{AppState, catalog, secrets, workspaces};

// ---------- storage ----------

mod repo {
    use super::*;

    fn to_run(r: &Row) -> rusqlite::Result<RunConfig> {
        let port: Option<i64> = r.get("port")?;
        Ok(RunConfig {
            id: r.get("id")?,
            project_id: r.get("project_id")?,
            name: r.get("name")?,
            command: r.get("command")?,
            port: port.and_then(|p| u16::try_from(p).ok()),
            primary: r.get::<_, i64>("is_primary")? != 0,
            docker: r.get::<_, i64>("docker")? != 0,
            position: r.get("position")?,
        })
    }

    pub fn list(db: &Connection, project_id: &str) -> rusqlite::Result<Vec<RunConfig>> {
        let mut st = db.prepare("SELECT * FROM run_configs WHERE project_id = ? ORDER BY position, rowid")?;
        st.query_map([project_id], to_run)?.collect()
    }

    pub fn get(db: &Connection, id: &str) -> rusqlite::Result<Option<RunConfig>> {
        db.query_row("SELECT * FROM run_configs WHERE id = ?", [id], to_run).optional()
    }

    pub fn create(
        db: &Connection,
        project_id: &str,
        name: &str,
        command: &str,
        port: Option<u16>,
        primary: bool,
        docker: bool,
    ) -> rusqlite::Result<RunConfig> {
        let id = crate::db::new_id();
        let pos: i64 =
            db.query_row("SELECT COALESCE(MAX(position), -1) + 1 FROM run_configs WHERE project_id = ?", [project_id], |r| r.get(0))?;
        let first = list(db, project_id)?.is_empty();
        db.execute(
            "INSERT INTO run_configs (id, project_id, name, command, port, is_primary, docker, position) VALUES (?, ?, ?, ?, ?, 0, ?, ?)",
            rusqlite::params![id, project_id, name, command, port, docker as i64, pos],
        )?;
        if primary || first {
            set_primary(db, project_id, &id)?;
        }
        Ok(get(db, &id)?.expect("just inserted"))
    }

    pub fn update(db: &Connection, cur: &RunConfig, v: &Valid) -> rusqlite::Result<()> {
        db.execute(
            "UPDATE run_configs SET name = ?, command = ?, port = ?, docker = ? WHERE id = ?",
            rusqlite::params![
                v.name.as_deref().unwrap_or(&cur.name),
                v.command.as_deref().unwrap_or(&cur.command),
                v.port.unwrap_or(cur.port),
                v.docker.unwrap_or(cur.docker) as i64,
                cur.id
            ],
        )?;
        Ok(())
    }

    pub fn set_primary(db: &Connection, project_id: &str, id: &str) -> rusqlite::Result<()> {
        db.execute("UPDATE run_configs SET is_primary = (id = ?) WHERE project_id = ?", [id, project_id])?;
        Ok(())
    }

    pub fn delete(db: &Connection, id: &str) -> rusqlite::Result<()> {
        let cur = get(db, id)?;
        db.execute("DELETE FROM run_configs WHERE id = ?", [id])?;
        // the primary one was deleted — the first remaining one becomes primary
        if let Some(cur) = cur.filter(|c| c.primary)
            && let Some(next) = list(db, &cur.project_id)?.first()
        {
            set_primary(db, &cur.project_id, &next.id)?;
        }
        Ok(())
    }
}

// ---------- commands ----------

const NAME_MAX: usize = 60;
const COMMAND_MAX: usize = 2000;

/// Fields of a run command (name/command limits and the port range are checked by
/// `validate` with specific messages).
#[derive(Deserialize, Default)]
pub struct RunFields {
    name: Option<String>,
    command: Option<String>,
    #[serde(default, deserialize_with = "double_option")]
    port: Option<Option<Value>>,
    primary: Option<bool>,
    docker: Option<bool>,
}

#[derive(Default)]
pub struct Valid {
    name: Option<String>,
    command: Option<String>,
    /// `Some(None)` — clear the port.
    port: Option<Option<u16>>,
    docker: Option<bool>,
}

/// JavaScript's `Number(string)` for the forms a port field takes.
fn js_number(s: &str) -> f64 {
    let t = s.trim();
    if t.is_empty() { 0.0 } else { t.parse().unwrap_or(f64::NAN) }
}

fn validate(b: &RunFields, partial: bool) -> ApiResult<Valid> {
    let mut out = Valid { docker: b.docker, ..Default::default() };
    if b.name.is_some() || !partial {
        match &b.name {
            Some(n) if !n.trim().is_empty() && n.encode_utf16().count() <= NAME_MAX => out.name = Some(n.trim().to_string()),
            _ => return Err(HttpError::bad_request(format!("Name is required (max {NAME_MAX})"))),
        }
    }
    if b.command.is_some() || !partial {
        match &b.command {
            Some(c) if !c.trim().is_empty() && c.encode_utf16().count() <= COMMAND_MAX => out.command = Some(c.trim().to_string()),
            _ => return Err(HttpError::bad_request("Command is required")),
        }
    }
    if let Some(port) = &b.port {
        out.port = Some(match port {
            None => None,
            Some(Value::String(s)) if s.is_empty() => None,
            Some(v) => {
                let n = match v {
                    Value::Number(n) => n.as_f64().unwrap_or(f64::NAN),
                    Value::String(s) => js_number(s),
                    _ => return Err(HttpError::bad_request("port: Invalid input")),
                };
                if n.fract() != 0.0 || !(1.0..=65535.0).contains(&n) {
                    return Err(HttpError::bad_request("Port must be 1–65535"));
                }
                Some(n as u16)
            }
        });
    }
    Ok(out)
}

fn run_or_404(db: &Connection, p: &Project, id: &str) -> ApiResult<RunConfig> {
    match repo::get(db, id)? {
        Some(r) if r.project_id == p.id => Ok(r),
        _ => Err(HttpError::not_found("Run command not found")),
    }
}

pub fn create_run(db: &Connection, p: &Project, b: &RunFields) -> ApiResult<RunState> {
    let v = validate(b, false)?;
    let name = v.name.unwrap_or_default();
    let command = v.command.unwrap_or_default();
    let r = repo::create(db, &p.id, &name, &command, v.port.flatten(), b.primary == Some(true), v.docker.unwrap_or(false))?;
    Ok(state(p, r))
}

pub fn update_run(db: &Connection, p: &Project, id: &str, b: &RunFields) -> ApiResult<RunState> {
    let cur = run_or_404(db, p, id)?;
    repo::update(db, &cur, &validate(b, true)?)?;
    if b.primary == Some(true) {
        repo::set_primary(db, &p.id, id)?;
    }
    Ok(state(p, repo::get(db, &cur.id)?.ok_or_else(|| HttpError::not_found("Run command not found"))?))
}

pub fn delete_run(db: &Connection, p: &Project, id: &str) -> ApiResult<()> {
    let r = run_or_404(db, p, id)?;
    stop(p, &r);
    repo::delete(db, id)?;
    Ok(())
}

// ---------- suggestions ----------

fn package_manager(dir: &Path) -> &'static str {
    if dir.join("pnpm-lock.yaml").exists() {
        "pnpm"
    } else if dir.join("yarn.lock").exists() {
        "yarn"
    } else if dir.join("bun.lockb").exists() || dir.join("bun.lock").exists() {
        "bun"
    } else {
        "npm"
    }
}

/// How to run Python: the project environment (uv/poetry/pipenv), else the system python.
fn python_runner(dir: &Path) -> &'static str {
    if dir.join("uv.lock").exists() {
        "uv run "
    } else if dir.join("poetry.lock").exists() {
        "poetry run "
    } else if dir.join("Pipfile.lock").exists() {
        "pipenv run "
    } else {
        ""
    }
}

fn read(f: &Path) -> String {
    std::fs::read_to_string(f).unwrap_or_default()
}

fn readdir(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .map(|it| it.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default()
}

/// FastAPI/Flask app file: look for `FastAPI(` / `Flask(` in typical places.
fn find_app(dir: &Path, marker: &str) -> Option<(String, String)> {
    for f in ["main.py", "app.py", "server.py", "app/main.py", "src/main.py", "api/main.py"] {
        if read(&dir.join(f)).contains(marker) {
            return Some((f.to_string(), f.trim_end_matches(".py").replace('/', ".")));
        }
    }
    None
}

fn suggestion(name: &str, command: String, source: &str) -> RunSuggestion {
    RunSuggestion { name: name.into(), command, source: source.into(), port: None, docker: None }
}

/// Commands from the repository. Python goes through the project environment (`uv run …`),
/// otherwise `python main.py` doesn't see dependencies (No module named …). Web frameworks
/// get a dev server with a port right away (for preview). compose gets the Docker flag.
pub fn suggest_runs(db: &Connection, p: &Project) -> ApiResult<Vec<RunSuggestion>> {
    let dir = Path::new(&p.path);
    let mut out: Vec<RunSuggestion> = vec![];
    if let Ok(text) = std::fs::read_to_string(dir.join("package.json"))
        && let Ok(pkg) = serde_json::from_str::<Value>(&text)
    {
        let mut names: Vec<String> = pkg.get("scripts").and_then(Value::as_object).map(|m| m.keys().cloned().collect()).unwrap_or_default();
        let pm = package_manager(dir);
        let order = ["dev", "start", "serve", "preview", "test", "build", "lint"];
        let rank = |n: &str| order.iter().position(|o| *o == n).map(|i| i + 1).unwrap_or(99);
        names.sort_by_key(|n| rank(n)); // stable, like Array.prototype.sort
        for n in names.iter().take(12) {
            let command = if pm == "npm" && n != "start" && n != "test" { format!("npm run {n}") } else { format!("{pm} {n}") };
            out.push(suggestion(n, command, "package.json"));
        }
    }

    // Python
    let pyproject = read(&dir.join("pyproject.toml"));
    let py = format!("{pyproject}\n{}", read(&dir.join("requirements.txt")));
    let run = python_runner(dir);
    let has = |dep: &str| {
        Regex::new(&format!(r#"(?im)["'\s^]{}([\[<>=~!"' ,]|$)"#, regex::escape(dep))).map(|re| re.is_match(&py)).unwrap_or(false)
    };
    let fastapi = if has("fastapi") { find_app(dir, "FastAPI(") } else { None };
    if let Some((file, module)) = &fastapi {
        let cli = Regex::new(r"(?i)fastapi\[standard\]|fastapi-cli").unwrap().is_match(&py);
        let command = if cli {
            format!("{run}fastapi dev {file}")
        } else {
            let with = if run == "uv run " && !has("uvicorn") { "--with uvicorn " } else { "" };
            format!("{run}{with}uvicorn {module}:app --reload")
        };
        out.push(RunSuggestion { port: Some(8000), ..suggestion("dev", command, "FastAPI") });
    }
    let flask = if has("flask") { find_app(dir, "Flask(") } else { None };
    if let Some((_, module)) = &flask {
        out.push(RunSuggestion { port: Some(5000), ..suggestion("dev", format!("{run}flask --app {module} run --debug"), "Flask") });
    }
    if dir.join("manage.py").exists() {
        out.push(RunSuggestion { port: Some(8000), ..suggestion("django", format!("{run}python manage.py runserver"), "manage.py") });
    }
    // packages with __main__.py → python -m package
    let entries = readdir(dir);
    let pkgs: Vec<&String> = entries.iter().filter(|d| !d.starts_with('.') && dir.join(d).join("__main__.py").exists()).collect();
    for d in pkgs.iter().take(3) {
        out.push(suggestion(d, format!("{run}python -m {d}"), &format!("{d}/__main__.py")));
    }
    let py_files = !py.trim().is_empty() || !pkgs.is_empty() || entries.iter().any(|f| f.ends_with(".py"));
    let tests_dir = readdir(&dir.join("tests")).iter().any(|f| f.ends_with(".py"));
    let test_file = entries.iter().any(|f| f.starts_with("test_") && f.ends_with(".py"));
    if py_files && (has("pytest") || tests_dir || test_file) {
        out.push(if has("pytest") {
            suggestion("test", format!("{run}pytest"), "pytest")
        } else {
            suggestion("test", format!("{run}python -m unittest -v"), "unittest")
        });
    }
    if let Some(start) = pyproject.find("[project.scripts]") {
        let body = &pyproject[start + "[project.scripts]".len()..];
        let body = &body[..body.find("\n[").unwrap_or(body.len())];
        for c in Regex::new(r"(?m)^\s*([A-Za-z0-9_.-]+)\s*=").unwrap().captures_iter(body) {
            out.push(suggestion(&c[1], format!("{run}{}", &c[1]), "pyproject.toml"));
        }
    }
    if fastapi.is_none() && flask.is_none() {
        for f in ["main.py", "app.py"] {
            if dir.join(f).exists() {
                let command = if run.is_empty() { format!("python {f}") } else { format!("{run}python {f}") };
                out.push(suggestion(f.trim_end_matches(".py"), command, f));
            }
        }
    }

    if let Ok(make) = std::fs::read_to_string(dir.join("Makefile")) {
        let target = Regex::new(r"^([a-zA-Z][A-Za-z0-9_-]*):").unwrap();
        let mut seen = HashSet::new();
        let targets: Vec<String> = make
            .split('\n')
            .filter_map(|l| target.captures(l).filter(|c| !l[c[0].len()..].starts_with('=')).map(|c| c[1].to_string()))
            .filter(|t| seen.insert(t.clone()))
            .collect();
        for t in targets.iter().take(8) {
            out.push(suggestion(t, format!("make {t}"), "Makefile"));
        }
    }
    if let Some(f) = ["compose.yaml", "compose.yml", "docker-compose.yml", "docker-compose.yaml"].into_iter().find(|f| dir.join(f).exists())
    {
        for (name, command) in [
            ("compose up", "docker compose up --build"),
            ("compose logs", "docker compose logs -f --tail=200"),
            ("compose down", "docker compose down"),
        ] {
            out.push(RunSuggestion { docker: Some(true), ..suggestion(name, command.into(), f) });
        }
    }
    if dir.join("go.mod").exists() {
        out.push(suggestion("go", "go run .".into(), "go.mod"));
    }
    if dir.join("Cargo.toml").exists() {
        out.push(suggestion("cargo", "cargo run".into(), "Cargo.toml"));
    }
    let known: HashSet<String> = repo::list(db, &p.id)?.into_iter().map(|r| r.command).collect();
    let mut seen = HashSet::new();
    Ok(out.into_iter().filter(|s| !known.contains(&s.command) && seen.insert(s.command.clone())).collect())
}

// ---------- run ----------

#[derive(Default)]
struct Running {
    job_id: String,
    detected_port: Option<u16>,
    tail: String,
}

/// Run command id → its current run.
static RUNNING: LazyLock<Mutex<HashMap<String, Arc<Mutex<Running>>>>> = LazyLock::new(Default::default);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// http://localhost:5173, http://127.0.0.1:8000/, http://0.0.0.0:3000, http://[::1]:4000
static PORT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"https?://(?:localhost|127\.0\.0\.1|0\.0\.0\.0|\[::1?\]|\d{1,3}(?:\.\d{1,3}){3})(?::(\d{2,5}))").unwrap());
static OSC_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\x1b\][^\x07]*(\x07|\x1b\\)").unwrap());
static CSI_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\x1b\[[0-9;?]*[a-zA-Z]").unwrap());

fn strip_ansi(s: &str) -> String {
    CSI_RE.replace_all(&OSC_RE.replace_all(s, ""), "").into_owned()
}

/// The app port printed by a command (`http://localhost:5173`).
fn detect_port(text: &str) -> Option<u16> {
    PORT_RE.captures(text).and_then(|c| c.get(1)).and_then(|m| m.as_str().parse().ok()).filter(|p| *p != 0)
}

fn preview_key(project_id: &str, run_id: &str) -> String {
    format!("{project_id}:{run_id}")
}

/// Where the preview proxy connects: the container address (docker), else the server.
fn preview_target(docker: bool, project_id: &str, port: u16) -> Target {
    let project_id = project_id.to_string();
    Arc::new(move || if docker { environments::container_ip(&project_id).map(|ip| (ip, port)) } else { Some(("127.0.0.1".into(), port)) })
}

fn state(p: &Project, r: RunConfig) -> RunState {
    let cur = lock(&RUNNING).get(&r.id).cloned();
    let (job, detected) = match &cur {
        Some(c) => {
            let c = lock(c);
            (terminal::job_info(&c.job_id), c.detected_port)
        }
        None => (None, None),
    };
    let active_port = r.port.or(detected);
    let running = job.as_ref().is_some_and(|j| j.status == JobStatus::Running);
    let preview_port = if running { preview::preview_port(&preview_key(&p.id, &r.id)) } else { None };
    RunState { config: r, job, active_port, preview_port }
}

pub fn list_runs(db: &Connection, p: &Project) -> ApiResult<Vec<RunState>> {
    Ok(repo::list(db, &p.id)?.into_iter().map(|r| state(p, r)).collect())
}

fn is_running(run_id: &str) -> bool {
    let cur = lock(&RUNNING).get(run_id).cloned();
    cur.is_some_and(|c| terminal::job_info(&lock(&c).job_id).is_some_and(|j| j.status == JobStatus::Running))
}

/// Terminal size of a job / shell (characters).
#[derive(Deserialize, Default)]
pub struct TermSize {
    cols: Option<f64>,
    rows: Option<f64>,
    fresh: Option<bool>,
}

impl TermSize {
    fn check(&self) -> ApiResult<()> {
        for (name, v, max) in [("cols", self.cols, 1000.0), ("rows", self.rows, 500.0)] {
            if let Some(v) = v
                && (v.fract() != 0.0 || !(1.0..=max).contains(&v))
            {
                return Err(HttpError::bad_request(format!("{name}: expected an integer from 1 to {max}")));
            }
        }
        Ok(())
    }
}

/// Start a run command (one run at a time: a running one is returned as is).
pub fn start_run(db: &Connection, config: &Config, p: &Project, id: &str, size: &TermSize) -> ApiResult<RunState> {
    let r = run_or_404(db, p, id)?;
    if is_running(id) {
        return Ok(state(p, r));
    }
    // Docker commands (compose) run without the sandbox: the Docker socket is hidden inside it
    let env = secrets::env(db, &config.data_dir, &p.id)?;
    let spec = environments::exec_command(db, config, &Scope::Project(p.clone()), Some(&r.command), env, r.docker)?;
    let docker = environments::runtime_of(db, p)?.environment == RuntimeEnvironment::Docker;
    let entry = Arc::new(Mutex::new(Running::default()));
    let key = preview_key(&p.id, &r.id);
    let host = config.host.clone();
    let handle = tokio::runtime::Handle::current();
    let on_output = {
        let (entry, key, host, project_id, fixed) = (entry.clone(), key.clone(), host.clone(), p.id.clone(), r.port);
        move |data: &str| {
            let mut e = lock(&entry);
            if e.detected_port.is_some() || fixed.is_some() {
                return;
            }
            let joined = format!("{}{}", e.tail, strip_ansi(data));
            let start = joined.char_indices().rev().nth(3999).map(|(i, _)| i).unwrap_or(0);
            e.tail = joined[start..].to_string();
            if let Some(port) = detect_port(&e.tail) {
                e.detected_port = Some(port);
                let (key, host, target) = (key.clone(), host.clone(), preview_target(docker, &project_id, port));
                handle.spawn(async move { preview::ensure_preview(&host, &key, target) });
            }
        }
    };
    let on_exit = {
        let key = key.clone();
        move |_: Option<i32>| preview::close_preview(&key)
    };
    let job = terminal::start_job(JobOpts {
        title: format!("{} · {}", p.name, r.name),
        command: spec.command,
        display: Some(r.command.clone()),
        env: spec.env,
        cwd: Some(spec.cwd),
        cols: size.cols,
        rows: size.rows,
        on_output: Some(Box::new(on_output)),
        on_exit: Some(Box::new(on_exit)),
        on_kill: spec.on_kill,
    });
    lock(&entry).job_id = job.id;
    lock(&RUNNING).insert(r.id.clone(), entry);
    if let Some(port) = r.port {
        preview::ensure_preview(&host, &key, preview_target(docker, &p.id, port));
    }
    Ok(state(p, r))
}

fn stop(p: &Project, r: &RunConfig) {
    let cur = lock(&RUNNING).get(&r.id).cloned();
    if let Some(c) = cur {
        let job_id = lock(&c).job_id.clone();
        terminal::kill_job(&job_id);
    }
    preview::close_preview(&preview_key(&p.id, &r.id));
}

pub fn stop_run(db: &Connection, p: &Project, id: &str) -> ApiResult<RunState> {
    let r = run_or_404(db, p, id)?;
    stop(p, &r);
    Ok(state(p, r))
}

/// Stop everything of a project (project removed from the hub, environment changed).
pub fn stop_project_processes(db: &Connection, project_id: &str) -> ApiResult<()> {
    for r in repo::list(db, project_id)? {
        let cur = lock(&RUNNING).get(&r.id).cloned();
        if let Some(c) = cur {
            let job_id = lock(&c).job_id.clone();
            terminal::kill_job(&job_id);
        }
    }
    preview::close_previews_with_prefix(&format!("{project_id}:"));
    let shells = lock(&SHELLS).get(&format!("project:{project_id}")).cloned().unwrap_or_default();
    for sh in shells {
        terminal::kill_job(&sh.job_id);
    }
    Ok(())
}

// ---------- shell ----------

#[derive(Clone)]
struct Shell {
    job_id: String,
    last_seen: Instant,
}

/// A scope can have several shells (tabs); each lives until it is closed or nobody watches
/// it for ~30 minutes.
static SHELLS: LazyLock<Mutex<HashMap<String, Vec<Shell>>>> = LazyLock::new(Default::default);
const SHELL_IDLE: Duration = Duration::from_secs(30 * 60);

fn shell_running(job_id: &str) -> bool {
    terminal::job_info(job_id).is_some_and(|j| j.status == JobStatus::Running)
}

/// Close shells nobody watched for a while; forget exited ones (checked every minute).
fn start_idle_reaper() {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        std::thread::spawn(|| {
            loop {
                std::thread::sleep(Duration::from_secs(60));
                let mut kill = vec![];
                {
                    let mut shells = lock(&SHELLS);
                    let now = Instant::now();
                    for list in shells.values_mut() {
                        list.retain(|s| shell_running(&s.job_id));
                        for s in list.iter_mut() {
                            if terminal::job_listeners(&s.job_id) > 0 {
                                s.last_seen = now;
                            } else if now.duration_since(s.last_seen) > SHELL_IDLE {
                                kill.push(s.job_id.clone());
                            }
                        }
                    }
                    shells.retain(|_, list| !list.is_empty());
                }
                for id in kill {
                    terminal::kill_job(&id);
                }
            }
        });
    });
}

/// Open shells of a scope (for tabs, reconnecting after a page reload).
pub fn list_shells(scope: &Scope) -> Vec<JobInfo> {
    let list = lock(&SHELLS).get(&scope.key()).cloned().unwrap_or_default();
    list.iter().filter_map(|s| terminal::job_info(&s.job_id)).filter(|i| i.status == JobStatus::Running).collect()
}

/// Scope shell: fresh — one more (tab), else the first open one or a new one. Old ones are
/// not closed.
pub fn open_shell(db: &Connection, config: &Config, scope: &Scope, size: &TermSize) -> ApiResult<JobInfo> {
    start_idle_reaper();
    let open = list_shells(scope);
    if size.fresh != Some(true)
        && let Some(first) = open.first()
    {
        return Ok(first.clone());
    }
    let env = match scope {
        Scope::Project(p) => secrets::env(db, &config.data_dir, &p.id)?,
        Scope::Group { .. } => vec![],
    };
    let spec = environments::exec_command(db, config, scope, None, env, false)?;
    let job = terminal::start_job(JobOpts {
        title: format!("Shell {}", open.len() + 1),
        command: spec.command,
        display: Some(spec.label),
        env: spec.env,
        cwd: Some(spec.cwd),
        cols: size.cols,
        rows: size.rows,
        on_kill: spec.on_kill,
        ..Default::default()
    });
    lock(&SHELLS).entry(scope.key()).or_default().push(Shell { job_id: job.id.clone(), last_seen: Instant::now() });
    Ok(job)
}

// ---------- routes ----------

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/projects/{id}/runs", get(list_route).post(create_route))
        .route("/projects/{id}/runs/suggest", get(suggest_route))
        .route("/projects/{id}/runs/{rid}", patch(update_route).delete(delete_route))
        .route("/projects/{id}/runs/{rid}/start", post(start_route))
        .route("/projects/{id}/runs/{rid}/stop", post(stop_route))
        .route("/projects/{id}/shell", post(project_shell))
        .route("/projects/{id}/shells", get(project_shells))
        .route("/groups/{id}/shell", post(group_shell))
        .route("/groups/{id}/shells", get(group_shells))
        .route("/projects/{id}/preview", post(preview_route))
}

type Id = UrlPath<String>;
type Ids = UrlPath<(String, String)>;

async fn list_route(State(s): State<AppState>, UrlPath(id): Id) -> ApiResult<Json<Vec<RunState>>> {
    s.with(|db, _| list_runs(db, &catalog::project_or_404(db, &id)?))
}

async fn suggest_route(State(s): State<AppState>, UrlPath(id): Id) -> ApiResult<Json<Vec<RunSuggestion>>> {
    s.with(|db, _| suggest_runs(db, &catalog::project_or_404(db, &id)?))
}

async fn create_route(
    State(s): State<AppState>,
    UrlPath(id): Id,
    JsonBody(b): JsonBody<RunFields>,
) -> ApiResult<(StatusCode, Json<RunState>)> {
    let r = s.with(|db, _| create_run(db, &catalog::project_or_404(db, &id)?, &b))?;
    Ok((StatusCode::CREATED, r))
}

async fn update_route(State(s): State<AppState>, UrlPath((id, rid)): Ids, JsonBody(b): JsonBody<RunFields>) -> ApiResult<Json<RunState>> {
    s.with(|db, _| update_run(db, &catalog::project_or_404(db, &id)?, &rid, &b))
}

async fn delete_route(State(s): State<AppState>, UrlPath((id, rid)): Ids) -> ApiResult<Json<Value>> {
    s.with(|db, _| {
        delete_run(db, &catalog::project_or_404(db, &id)?, &rid)?;
        Ok(json!({ "ok": true }))
    })
}

async fn start_route(
    State(s): State<AppState>,
    UrlPath((id, rid)): Ids,
    LenientBody(size): LenientBody<TermSize>,
) -> ApiResult<Json<RunState>> {
    size.check()?;
    s.with(|db, cfg| start_run(db, cfg, &catalog::project_or_404(db, &id)?, &rid, &size))
}

async fn stop_route(State(s): State<AppState>, UrlPath((id, rid)): Ids) -> ApiResult<Json<RunState>> {
    s.with(|db, _| stop_run(db, &catalog::project_or_404(db, &id)?, &rid))
}

fn group_scope(db: &Connection, config: &Config, id: &str) -> ApiResult<Scope> {
    let group = catalog::group_or_404(db, id)?;
    let projects = catalog::group_projects(db, &group)?;
    let cwd = workspaces::group_workspace(db, config, &group.id, &projects)?.path;
    Ok(Scope::Group { group, projects, cwd })
}

/// Project shell: an already open one is returned (reconnect); fresh opens a new one.
async fn project_shell(State(s): State<AppState>, UrlPath(id): Id, LenientBody(size): LenientBody<TermSize>) -> ApiResult<Json<JobInfo>> {
    size.check()?;
    s.with(|db, cfg| open_shell(db, cfg, &Scope::Project(catalog::project_or_404(db, &id)?), &size))
}

async fn group_shell(State(s): State<AppState>, UrlPath(id): Id, LenientBody(size): LenientBody<TermSize>) -> ApiResult<Json<JobInfo>> {
    size.check()?;
    s.with(|db, cfg| open_shell(db, cfg, &group_scope(db, cfg, &id)?, &size))
}

async fn project_shells(State(s): State<AppState>, UrlPath(id): Id) -> ApiResult<Json<Vec<JobInfo>>> {
    s.with(|db, _| Ok(list_shells(&Scope::Project(catalog::project_or_404(db, &id)?))))
}

async fn group_shells(State(s): State<AppState>, UrlPath(id): Id) -> ApiResult<Json<Vec<JobInfo>>> {
    s.with(|db, cfg| Ok(list_shells(&group_scope(db, cfg, &id)?)))
}

#[derive(Deserialize)]
struct PreviewBody {
    port: Option<Value>,
}

/// Preview of a port something is already listening on (Shell, docker compose…).
async fn preview_route(State(s): State<AppState>, UrlPath(id): Id, JsonBody(b): JsonBody<PreviewBody>) -> ApiResult<Json<Value>> {
    // like z.coerce.number().int().min(1).max(65535)
    let n = match &b.port {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(f64::NAN),
        Some(Value::String(s)) => js_number(s),
        Some(Value::Bool(v)) => f64::from(u8::from(*v)),
        Some(Value::Null) => 0.0,
        _ => f64::NAN,
    };
    if n.fract() != 0.0 || !(1.0..=65535.0).contains(&n) {
        return Err(HttpError::bad_request("port: expected an integer from 1 to 65535"));
    }
    let port = n as u16;
    let (p, docker, host) = {
        let db = s.db()?;
        let p = catalog::project_or_404(&db, &id)?;
        let docker = environments::runtime_of(&db, &p)?.environment == RuntimeEnvironment::Docker;
        (p, docker, s.config().host.clone())
    };
    let pp = preview::ensure_preview(&host, &format!("{}:port:{port}", p.id), preview_target(docker, &p.id, port))
        .ok_or_else(|| HttpError::new(503, "No free preview ports (HUBNESS_PREVIEW_PORTS)"))?;
    Ok(Json(json!({ "previewPort": pp })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hubness_core::ProjectKind;

    fn project(dir: &Path) -> (Connection, Project) {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::init(&conn).unwrap();
        let p = crate::db::projects::create(&conn, "p", &crate::paths::to_string(dir), ProjectKind::Normal).unwrap();
        (conn, p)
    }

    #[test]
    fn suggestions_from_package_json_makefile_and_python() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        std::fs::write(d.join("package.json"), r#"{"scripts":{"build":"x","lint":"x","dev":"x","test":"x","zzz":"x"}}"#).unwrap();
        std::fs::write(d.join("pnpm-lock.yaml"), "").unwrap();
        std::fs::write(d.join("Makefile"), "VAR := 1\nall: x\nbuild:\n\t@echo\nX=1\nall:\n").unwrap();
        std::fs::write(d.join("pyproject.toml"), "[project]\ndependencies = [\"fastapi>=0.1\"]\n[project.scripts]\nmycli = \"a:b\"\n")
            .unwrap();
        std::fs::write(d.join("uv.lock"), "").unwrap();
        std::fs::write(d.join("main.py"), "app = FastAPI()\n").unwrap();
        let (db, p) = project(d);
        let s = suggest_runs(&db, &p).unwrap();
        let cmds: Vec<&str> = s.iter().map(|s| s.command.as_str()).collect();
        assert_eq!(
            cmds,
            [
                "pnpm dev",
                "pnpm test",
                "pnpm build",
                "pnpm lint",
                "pnpm zzz",
                "uv run --with uvicorn uvicorn main:app --reload",
                "uv run mycli",
                "make all",
                "make build"
            ]
        );
        assert_eq!(s[5].port, Some(8000));
        repo::create(&db, &p.id, "dev", "pnpm dev", None, false, false).unwrap();
        assert!(!suggest_runs(&db, &p).unwrap().iter().any(|s| s.command == "pnpm dev"), "known commands are not suggested");
    }

    #[test]
    fn ports_are_detected_in_output() {
        assert_eq!(detect_port(&strip_ansi("\x1b[32m  ➜  Local:   http://localhost:\x1b[1m5173\x1b[22m/")), Some(5173));
        assert_eq!(detect_port("Uvicorn running on http://127.0.0.1:8000 (Press CTRL+C)"), Some(8000));
        assert_eq!(detect_port("listening on http://[::1]:4000"), Some(4000));
        assert_eq!(detect_port("see https://example.com:8443"), None);
    }

    #[test]
    fn run_config_validation_and_primary() {
        let tmp = tempfile::tempdir().unwrap();
        let (db, p) = project(tmp.path());
        let f = |v: Value| serde_json::from_value::<RunFields>(v).unwrap();
        assert!(create_run(&db, &p, &f(json!({ "name": " ", "command": "x" }))).is_err());
        assert!(create_run(&db, &p, &f(json!({ "name": "a", "command": "x", "port": 70000 }))).is_err());
        assert!(create_run(&db, &p, &f(json!({ "name": "a", "command": "x", "port": "abc" }))).is_err());
        let a = create_run(&db, &p, &f(json!({ "name": " a ", "command": "x", "port": "3000" }))).unwrap();
        assert_eq!((a.config.name.as_str(), a.config.port, a.config.primary), ("a", Some(3000), true));
        let b = create_run(&db, &p, &f(json!({ "name": "b", "command": "y", "primary": true }))).unwrap();
        assert!(b.config.primary && !list_runs(&db, &p).unwrap()[0].config.primary);
        let a2 = update_run(&db, &p, &a.config.id, &f(json!({ "port": "" }))).unwrap();
        assert_eq!(a2.config.port, None);
        delete_run(&db, &p, &b.config.id).unwrap();
        assert!(list_runs(&db, &p).unwrap()[0].config.primary, "the next one becomes primary");
    }
}
