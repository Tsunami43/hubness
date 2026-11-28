//! Session turns — a port of `runtime.ts`: starts the agent for a message, persists and
//! broadcasts its events, tracks running turns for stop and approvals.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use hubness_core::{GlobalStreamMsg, HubEvent, Session, SessionStatus, SessionStreamMsg};
use rusqlite::Connection;
use serde_json::{Map, Value, json};

use crate::agents::{self, ApprovalDecision, RunHandle, StopReason};
use crate::db::{self, groups, kv, projects, sessions as repo};
use crate::environments::{self, Wrap};
use crate::error::{ApiResult, HttpError};
use crate::sessions::truncate;
use crate::{AppState, catalog, context, paths, workspaces};

/// Default titles — replaced by the first message (Russian ones are for old records).
const DEFAULT_TITLES: &[&str] = &["New session", "New chat", "Новая сессия", "Новый чат"];

/// A running agent process and its turn. Once the turn has answered (`Turn::answered`) the
/// process may linger — Claude Code waits for its background tasks before it exits; the
/// session is idle for the user then.
#[derive(Clone)]
struct Active {
    handle: Arc<dyn RunHandle>,
    turn: Arc<Turn>,
}

/// Running turns by session id.
#[derive(Default, Clone)]
pub struct Runtime {
    active: Arc<Mutex<HashMap<String, Active>>>,
    /// Sessions whose turn is being set up (between the "is it busy?" check and the agent's
    /// start): a second message in that window is refused like one to a running turn.
    starting: Arc<Mutex<std::collections::HashSet<String>>>,
}

/// A session reserved for a turn that is starting; released when dropped.
struct Starting {
    set: Arc<Mutex<std::collections::HashSet<String>>>,
    id: String,
}

impl Drop for Starting {
    fn drop(&mut self) {
        self.set.lock().unwrap_or_else(|e| e.into_inner()).remove(&self.id);
    }
}

impl Runtime {
    /// The agent is at work in the session (a process that only lingers after its answer is not).
    pub fn is_active(&self, session_id: &str) -> bool {
        self.active.lock().expect("runtime lock").get(session_id).is_some_and(|a| !a.turn.answered.load(Ordering::SeqCst))
    }

    /// Stop a process that lingers after its answer, quietly: nothing it says reaches the feed.
    fn retire(&self, session_id: &str) {
        let Some(a) = self.active.lock().expect("runtime lock").remove(session_id) else { return };
        a.turn.retired.store(true, Ordering::SeqCst);
        a.handle.stop(StopReason::User);
    }

    /// Reserve the session for a new turn: `None` while one runs or is starting.
    fn reserve(&self, session_id: &str) -> Option<Starting> {
        let mut starting = self.starting.lock().unwrap_or_else(|e| e.into_inner());
        if self.is_active(session_id) || !starting.insert(session_id.to_string()) {
            return None;
        }
        Some(Starting { set: self.starting.clone(), id: session_id.to_string() })
    }

    fn handle(&self, session_id: &str) -> Option<Arc<dyn RunHandle>> {
        self.active.lock().expect("runtime lock").get(session_id).map(|a| a.handle.clone())
    }

    /// Stop the session's agent. One that has already answered is stopped quietly.
    pub fn stop(&self, session_id: &str) -> bool {
        let Some(a) = self.active.lock().expect("runtime lock").get(session_id).cloned() else { return false };
        if a.turn.answered.load(Ordering::SeqCst) {
            self.retire(session_id);
        } else {
            a.handle.stop(StopReason::User);
        }
        true
    }

    pub fn respond(&self, session_id: &str, request_id: &str, d: ApprovalDecision) -> bool {
        self.handle(session_id).is_some_and(|h| h.respond(request_id, d))
    }

    /// Hub exit: the feed shows "interrupted by restart", not "stopped" (nothing for an answered turn).
    pub fn stop_all(&self) {
        for a in self.active.lock().expect("runtime lock").values() {
            if a.turn.answered.load(Ordering::SeqCst) {
                a.turn.retired.store(true, Ordering::SeqCst);
            }
            a.handle.stop(StopReason::Shutdown);
        }
    }
}

/// What an agent adapter gets for one turn (the TS `RunContext`).
pub struct Turn {
    state: AppState,
    pub session: Mutex<Session>,
    pub cwd: String,
    pub prompt: String,
    /// Files attached to this message (paths are already in `prompt`).
    pub attachments: Vec<hubness_core::Attachment>,
    pub extra_dirs: Vec<String>,
    pub system_prompt: Option<String>,
    pub wrap: Wrap,
    /// Extra environment for the agent process (project secrets, if allowed).
    pub env: Vec<(String, String)>,
    /// Hub MCP servers (open_pane).
    pub mcp_servers: Vec<crate::mcp::McpServer>,
    seq: Mutex<i64>,
    cache: Mutex<HashMap<String, HubEvent>>,
    log: Mutex<Option<File>>,
    log_path: std::path::PathBuf,
    /// The agent has answered (a `result`); it may still come back on its own (a background
    /// task finished), which makes it running again.
    answered: AtomicBool,
    /// Replaced by a newer turn or stopped after its answer: whatever it still says is dropped.
    retired: AtomicBool,
    /// The last event before the answer was an error notice.
    last_failed: AtomicBool,
}

/// Event kinds that mean the agent is at work again (new events, not updates of old ones).
const WORK_KINDS: &[&str] = &["text", "thinking", "tool", "approval"];

impl Turn {
    pub fn session(&self) -> Session {
        self.session.lock().expect("turn lock").clone()
    }

    /// The session id (without cloning the whole session, as `session()` does).
    fn session_id(&self) -> String {
        self.session.lock().expect("turn lock").id.clone()
    }

    fn stored(&self, uid: &str) -> Option<HubEvent> {
        if let Some(e) = self.cache.lock().expect("turn lock").get(uid) {
            return Some(e.clone());
        }
        self.stored_in_db(uid)
    }

    fn stored_in_db(&self, uid: &str) -> Option<HubEvent> {
        let db = self.state.db().ok()?;
        let sid = self.session_id();
        db.query_row("SELECT data FROM events WHERE session_id = ? AND uid = ?", [&sid, uid], |r| r.get::<_, String>(0))
            .ok()
            .and_then(|d| serde_json::from_str(&d).ok())
    }

    pub fn get(&self, uid: &str) -> Option<HubEvent> {
        self.stored(uid)
    }

    /// Create or replace an event. `persist = false`: broadcast only (delta streaming).
    pub fn put(&self, uid: &str, body: Value, persist: bool) {
        if self.retired.load(Ordering::SeqCst) {
            return;
        }
        let kind = body["kind"].as_str().unwrap_or_default().to_string();
        let failed = kind == "notice" && body["level"] == "error";
        // only seq and ts of the previous version are kept: read them without cloning it
        // (a streamed message is replaced on every delta and keeps growing)
        let seq_ts = |p: &Value| (p["seq"].as_i64(), p["ts"].as_str().map(String::from));
        let cached = self.cache.lock().expect("turn lock").get(uid).map(seq_ts);
        let (seq, ts) = cached.or_else(|| self.stored_in_db(uid).as_ref().map(seq_ts)).unwrap_or_default();
        let is_new = seq.is_none();
        let seq = seq.unwrap_or_else(|| {
            let mut s = self.seq.lock().expect("turn lock");
            let v = *s;
            *s += 1;
            v
        });
        let ts = ts.unwrap_or_else(db::now);
        // body first, then the service fields (the key order of the TS server)
        let mut ev = Map::new();
        if let Value::Object(b) = body {
            ev.extend(b);
        }
        ev.insert("uid".into(), json!(uid));
        ev.insert("seq".into(), json!(seq));
        ev.insert("ts".into(), json!(ts));
        let ev = Value::Object(ev);
        self.cache.lock().expect("turn lock").insert(uid.to_string(), ev.clone());
        let sid = self.session_id();
        if persist && let Ok(db) = self.state.db() {
            let _ = repo::put_event(&db, &sid, &ev);
        }
        self.state.bus.emit_session(&sid, SessionStreamMsg::Event { event: ev });
        if kind == "result" {
            // the agent has answered: the turn is over for the user, even if the process lingers
            self.answered.store(true, Ordering::SeqCst);
            finish_turn(&self.state, &sid, self.last_failed.load(Ordering::SeqCst));
        } else {
            self.last_failed.store(failed, Ordering::SeqCst);
            if is_new && WORK_KINDS.contains(&kind.as_str()) && self.answered.swap(false, Ordering::SeqCst) {
                // it came back on its own (a background task finished)
                set_status(&self.state, &sid, SessionStatus::Running);
            }
        }
    }

    /// Partially update an existing event (a `null` field removes it).
    pub fn patch(&self, uid: &str, patch: Value, persist: bool) {
        let Some(Value::Object(mut prev)) = self.stored(uid) else { return };
        for k in ["uid", "seq", "ts"] {
            prev.shift_remove(k);
        }
        if let Value::Object(p) = patch {
            for (k, v) in p {
                if v.is_null() {
                    prev.shift_remove(&k);
                } else {
                    prev.insert(k, v);
                }
            }
        }
        self.put(uid, Value::Object(prev), persist);
    }

    pub fn set_status(&self, status: SessionStatus) {
        if self.retired.load(Ordering::SeqCst) {
            return;
        }
        if matches!(status, SessionStatus::Running | SessionStatus::Waiting) {
            self.answered.store(false, Ordering::SeqCst);
        }
        set_status(&self.state, &self.session_id(), status);
    }

    pub fn set_agent_session_id(&self, id: &str) {
        let mut s = self.session.lock().expect("turn lock");
        if s.agent_session_id.as_deref() == Some(id) {
            return;
        }
        if let Ok(db) = self.state.db() {
            let _ = db.execute("UPDATE sessions SET agent_session_id = ?, updated_at = ? WHERE id = ?", (id, db::now(), &s.id));
        }
        s.agent_session_id = Some(id.to_string());
    }

    /// Raw agent output, appended to the run's log file. A failed write never breaks the turn.
    pub fn raw(&self, line: &str) {
        let mut log = self.log.lock().expect("turn lock");
        if log.is_none() {
            *log = OpenOptions::new().create(true).append(true).open(&self.log_path).ok();
        }
        if let Some(f) = log.as_mut() {
            let _ = writeln!(f, "{line}");
        }
    }

    /// Full session history (for agents without their own memory).
    pub fn history(&self) -> Vec<HubEvent> {
        let sid = self.session_id();
        self.state.db().ok().and_then(|db| repo::events(&db, &sid).ok()).unwrap_or_default()
    }

    pub fn kv_set(&self, key: &str, value: &Value) {
        if let Ok(db) = self.state.db() {
            let _ = kv::set(&db, key, value);
        }
    }

    pub fn kv_get<T: serde::de::DeserializeOwned>(&self, key: &str) -> Option<T> {
        self.state.db().ok().and_then(|db| kv::get(&db, key).ok().flatten())
    }

    fn last_is_error(&self) -> bool {
        let sid = self.session_id();
        let Ok(db) = self.state.db() else { return false };
        let last: Option<String> =
            db.query_row("SELECT data FROM events WHERE session_id = ? ORDER BY seq DESC LIMIT 1", [&sid], |r| r.get(0)).ok();
        last.and_then(|d| serde_json::from_str::<Value>(&d).ok()).is_some_and(|e| e["kind"] == "notice" && e["level"] == "error")
    }
}

fn announce(state: &AppState, s: &Session) {
    state.bus.emit_session(&s.id, SessionStreamMsg::Session { session: s.clone() });
    state.bus.emit_global(GlobalStreamMsg::Session { session: s.clone() });
}

/// The turn is over (answered, or its process is gone): idle or error; a session started from
/// a kanban card sends the card to review.
fn finish_turn(state: &AppState, id: &str, failed: bool) {
    set_status(state, id, if failed { SessionStatus::Error } else { SessionStatus::Idle });
    if !failed && let Ok(db) = state.db() {
        let card: Option<String> = repo::get(&db, id).ok().flatten().and_then(|s| s.card_id);
        if let Some(card) = card {
            let _ = crate::kanban::card_to_review(&db, &card);
        }
    }
}

pub fn set_status(state: &AppState, id: &str, status: SessionStatus) {
    let session = state.db().ok().and_then(|db| {
        let st = serde_json::to_value(status).ok()?.as_str()?.to_string();
        db.execute("UPDATE sessions SET status = ?, updated_at = ? WHERE id = ?", (st, db::now(), id)).ok()?;
        repo::get(&db, id).ok().flatten()
    });
    if let Some(s) = session {
        announce(state, &s);
    }
}

/// The session's working folder: the project repository, the group workspace, or the chat's
/// Inbox folder (old chats from before Inbox: `<data>/chats/<id>`).
pub fn session_cwd(db: &Connection, cfg: &crate::Config, s: &Session) -> ApiResult<String> {
    if let Some(pid) = &s.project_id {
        return Ok(projects::get(db, pid)?.ok_or_else(|| HttpError::not_found("Session project not found"))?.path);
    }
    if let Some(gid) = &s.group_id {
        let g = groups::get(db, gid)?.ok_or_else(|| HttpError::not_found("Session group not found"))?;
        return Ok(workspaces::group_workspace(db, cfg, &g.id, &catalog::group_projects(db, &g)?)?.path);
    }
    let dir = match &s.cwd {
        Some(c) => std::path::PathBuf::from(c),
        None => cfg.chats_dir.join(&s.id),
    };
    std::fs::create_dir_all(&dir)?;
    Ok(paths::to_string(&dir))
}

fn now_ms() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}

/// Run blocking work (processes, short waits) so that an async worker isn't held up by it.
fn off_the_async_threads<T>(f: impl FnOnce() -> T) -> T {
    match tokio::runtime::Handle::try_current() {
        Ok(h) if h.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => tokio::task::block_in_place(f),
        _ => f(),
    }
}

/// Start a turn: record the user message, launch the agent, return the updated session.
pub fn send_message(state: &AppState, session_id: &str, text: &str, attachments: Vec<hubness_core::Attachment>) -> ApiResult<Session> {
    let rt = &state.runtime;
    // what the turn is made of, gathered under the database lock; the SSH setup (processes,
    // waiting) happens after it is released
    let (session, parts, (ssh_plan, ssh_untested), starting) = {
        let db = state.db()?;
        let cfg = state.config();
        let mut session = repo::get(&db, session_id)?.ok_or_else(|| HttpError::not_found("Session not found"))?;
        let Some(starting) = rt.reserve(session_id) else {
            return Err(HttpError::new(409, "The agent is still working — wait for it or stop it"));
        };
        if text.trim().is_empty() && attachments.is_empty() {
            return Err(HttpError::bad_request("Empty message"));
        }
        // the previous turn's process still waits for its background tasks: the new turn replaces it
        rt.retire(session_id);
        let project = match &session.project_id {
            Some(id) => projects::get(&db, id)?,
            None => None,
        };
        let group = match &session.group_id {
            Some(id) => groups::get(&db, id)?,
            None => None,
        };
        let env = match &project {
            Some(p) if environments::saved_runtime(&db, &p.id)?.share_secrets_with_agents == Some(true) => {
                crate::secrets::env(&db, &cfg.data_dir, &p.id)?
            }
            _ => vec![],
        };
        if session.title.is_empty() || DEFAULT_TITLES.contains(&session.title.as_str()) {
            let source = if text.trim().is_empty() {
                attachments.iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join(", ")
            } else {
                text.to_string()
            };
            let t = source.split_whitespace().collect::<Vec<_>>().join(" ");
            let title = if t.encode_utf16().count() > 60 { format!("{}…", truncate(&t, 57)) } else { t };
            session = repo::update(&db, session_id, repo::Patch { title: Some(title), agent: None })?.expect("session exists");
        }
        if let Some(p) = &project {
            db.execute("UPDATE projects SET updated_at = ? WHERE id = ?", (db::now(), &p.id))?;
        }
        if let Some(g) = &group {
            db.execute("UPDATE project_groups SET updated_at = ? WHERE id = ?", (db::now(), &g.id))?;
        }
        let cwd = session_cwd(&db, cfg, &session)?;
        // after a chat is moved into a project, the agent gets a conversation summary (once)
        let handoff: Option<String> = db.query_row("SELECT handoff FROM sessions WHERE id = ?", [session_id], |r| r.get(0))?;
        if handoff.is_some() {
            db.execute("UPDATE sessions SET handoff = NULL WHERE id = ?", [session_id])?;
        }
        let members = match &group {
            Some(g) => catalog::group_projects(&db, g)?,
            None => vec![],
        };
        let scope_ctx = match (&project, &group) {
            (Some(p), _) => context::project_context(p),
            (None, Some(g)) => Some(context::group_context(g, &members)),
            _ => None,
        };
        // SSH connections of the project (a group: its own and its repositories'), for this turn
        let owners: Vec<crate::ssh::Owner> = match (&project, &group) {
            (Some(p), _) => vec![crate::ssh::Owner::Project(&p.id)],
            (None, Some(g)) => {
                std::iter::once(crate::ssh::Owner::Group(&g.id)).chain(members.iter().map(|p| crate::ssh::Owner::Project(&p.id))).collect()
            }
            _ => vec![],
        };
        // a broken setup (an unreadable key) costs the turn its SSH, not the turn
        let ssh_plan = crate::ssh::plan(&db, cfg, &owners, session_id).map_err(|e| e.message);
        // bound but not tested yet: not for the agent — the person is told
        let ssh_untested = crate::ssh::untested(&db, &owners).unwrap_or_default();
        let hub_instructions = catalog::settings(&db, cfg)?.session_instructions;
        let wrap = match (&project, &group) {
            (Some(p), _) if environments::project_sandbox(&db, p)? => {
                Wrap { bwrap: Some(environments::bwrap_args(&db, cfg, std::slice::from_ref(&p.path), &p.path)?) }
            }
            (None, Some(_)) if environments::sandbox_available() => {
                let repos: Vec<String> = members.iter().map(|p| p.path.clone()).collect();
                Wrap { bwrap: Some(environments::bwrap_args(&db, cfg, &repos, &cwd)?) }
            }
            _ => Wrap::default(),
        };
        let prompt = match handoff {
            Some(h) => format!("{h}\n{text}"),
            None => text.to_string(),
        } + &crate::attachments::note(&attachments);
        let parts = (prompt, cwd, members, scope_ctx, hub_instructions, wrap, env, repo::next_seq(&db, session_id)?);
        (session, parts, (ssh_plan, ssh_untested), starting)
    };
    let (prompt, cwd, members, scope_ctx, hub_instructions, wrap, mut env, seq) = parts;

    let (ssh, ssh_problem) = match ssh_plan {
        Ok(None) => (None, None),
        Ok(Some(plan)) => match off_the_async_threads(|| crate::ssh::start(plan)) {
            Ok(s) if s.failed.is_empty() => (Some(s), None),
            Ok(s) => {
                let problem = format!("SSH: the key of {} could not be loaded — not available in this turn.", s.failed.join(", "));
                (Some(s), Some(problem))
            }
            Err(e) => (None, Some(format!("SSH connections are not available in this turn: {e}"))),
        },
        Err(e) => (None, Some(format!("SSH connections are not available in this turn: {e}"))),
    };
    let scope_ctx = match (&ssh, scope_ctx) {
        (Some(s), Some(c)) => Some(format!("{c}\n\n{}", s.context)),
        (Some(s), None) => Some(s.context.clone()),
        (None, c) => c,
    };
    // the sandbox hides the hub's folders: show it the session's ssh folder, read-only
    let wrap = match (wrap, &ssh) {
        (Wrap { bwrap: Some(mut args) }, Some(s)) => {
            args.extend(["--ro-bind".to_string(), s.mount.clone(), s.mount.clone()]);
            Wrap { bwrap: Some(args) }
        }
        (w, _) => w,
    };
    if let Some(s) = &ssh {
        env.extend(s.env.iter().cloned());
    }
    let turn = Arc::new(Turn {
        state: state.clone(),
        session: Mutex::new(session.clone()),
        // the agent is told where attached files are; images also go to the model directly
        prompt,
        attachments: attachments.clone(),
        cwd,
        // a group session sees exactly the member repositories (real paths, not symlinks)
        extra_dirs: members.iter().map(|p| p.path.clone()).collect(),
        system_prompt: context::session_system_prompt(&hub_instructions, scope_ctx),
        wrap,
        env,
        mcp_servers: state.self_url.get().and_then(|url| crate::mcp::hub_mcp(url, session_id)).into_iter().collect(),
        seq: Mutex::new(seq),
        cache: Mutex::default(),
        log: Mutex::default(),
        log_path: Path::new(&state.config().logs_dir).join(format!("{session_id}.jsonl")),
        answered: AtomicBool::new(false),
        retired: AtomicBool::new(false),
        last_failed: AtomicBool::new(false),
    });

    let mut user = json!({ "kind": "user", "text": text });
    if !attachments.is_empty() {
        user["attachments"] = json!(attachments);
    }
    turn.put(&format!("u:{}", now_ms()), user, true);
    let paths: Vec<&str> = attachments.iter().map(|a| a.path.as_str()).collect();
    turn.raw(&json!({ "hub": "user", "ts": db::now(), "text": text, "attachments": paths }).to_string());
    if let Some(problem) = ssh_problem {
        turn.put(&format!("ssh:{}", now_ms()), json!({ "kind": "notice", "level": "error", "text": problem }), true);
    }
    if !ssh_untested.is_empty() {
        let text = crate::ssh::untested_notice(&ssh_untested);
        turn.put(&format!("ssh-untested:{}", now_ms()), json!({ "kind": "notice", "level": "info", "text": text }), true);
    }
    set_status(state, session_id, SessionStatus::Running);

    match agents::start(&session.agent.kind, turn.clone()) {
        Err(message) => {
            turn.put(&format!("err:{}", now_ms()), json!({ "kind": "notice", "level": "error", "text": message }), true);
            set_status(state, session_id, SessionStatus::Error);
        }
        Ok((handle, done)) => {
            rt.active.lock().expect("runtime lock").insert(session_id.to_string(), Active { handle, turn: turn.clone() });
            // the turn runs: the reservation has done its job
            drop(starting);
            let state = state.clone();
            let id = session_id.to_string();
            tokio::spawn(async move {
                let _ = done.await;
                // the process is gone: its ssh-agent and config go away
                drop(ssh);
                *turn.log.lock().expect("turn lock") = None;
                // a retired process leaves the session to the turn that replaced it
                {
                    let mut active = state.runtime.active.lock().expect("runtime lock");
                    if turn.retired.load(Ordering::SeqCst) || !active.get(&id).is_some_and(|a| Arc::ptr_eq(&a.turn, &turn)) {
                        return;
                    }
                    active.remove(&id);
                }
                finish_turn(&state, &id, turn.last_is_error());
            });
        }
    }
    let db = state.db()?;
    Ok(repo::get(&db, session_id)?.expect("session exists"))
}
