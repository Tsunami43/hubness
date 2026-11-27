//! Session turns — a port of `runtime.ts`: starts the agent for a message, persists and
//! broadcasts its events, tracks running turns for stop and approvals.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
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

/// Running turns by session id.
#[derive(Default, Clone)]
pub struct Runtime {
    active: Arc<Mutex<HashMap<String, Arc<dyn RunHandle>>>>,
}

impl Runtime {
    pub fn is_active(&self, session_id: &str) -> bool {
        self.active.lock().expect("runtime lock").contains_key(session_id)
    }

    fn handle(&self, session_id: &str) -> Option<Arc<dyn RunHandle>> {
        self.active.lock().expect("runtime lock").get(session_id).cloned()
    }

    pub fn stop(&self, session_id: &str) -> bool {
        match self.handle(session_id) {
            Some(h) => {
                h.stop(StopReason::User);
                true
            }
            None => false,
        }
    }

    pub fn respond(&self, session_id: &str, request_id: &str, d: ApprovalDecision) -> bool {
        self.handle(session_id).is_some_and(|h| h.respond(request_id, d))
    }

    /// Hub exit: the feed shows "interrupted by restart", not "stopped".
    pub fn stop_all(&self) {
        for h in self.active.lock().expect("runtime lock").values() {
            h.stop(StopReason::Shutdown);
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
}

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
        // only seq and ts of the previous version are kept: read them without cloning it
        // (a streamed message is replaced on every delta and keeps growing)
        let seq_ts = |p: &Value| (p["seq"].as_i64(), p["ts"].as_str().map(String::from));
        let cached = self.cache.lock().expect("turn lock").get(uid).map(seq_ts);
        let (seq, ts) = cached.or_else(|| self.stored_in_db(uid).as_ref().map(seq_ts)).unwrap_or_default();
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

/// Start a turn: record the user message, launch the agent, return the updated session.
pub fn send_message(state: &AppState, session_id: &str, text: &str, attachments: Vec<hubness_core::Attachment>) -> ApiResult<Session> {
    let rt = &state.runtime;
    let (turn, session, ssh) = {
        let db = state.db()?;
        let cfg = state.config();
        let mut session = repo::get(&db, session_id)?.ok_or_else(|| HttpError::not_found("Session not found"))?;
        if rt.is_active(session_id) {
            return Err(HttpError::new(409, "The agent is still working — wait for it or stop it"));
        }
        if text.trim().is_empty() && attachments.is_empty() {
            return Err(HttpError::bad_request("Empty message"));
        }
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
        let ssh = crate::ssh::session_setup(&db, cfg, &owners, session_id)?;
        let scope_ctx = match (&ssh, scope_ctx) {
            (Some(s), Some(c)) => Some(format!("{c}\n\n{}", s.context)),
            (Some(s), None) => Some(s.context.clone()),
            (None, c) => c,
        };
        let mut env = env;
        if let Some(s) = &ssh {
            env.extend(s.env.iter().cloned());
        }
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
        // the sandbox hides the hub's folders: show it the session's ssh folder, read-only
        let wrap = match (wrap, &ssh) {
            (Wrap { bwrap: Some(mut args) }, Some(s)) => {
                args.extend(["--ro-bind".to_string(), s.mount.clone(), s.mount.clone()]);
                Wrap { bwrap: Some(args) }
            }
            (w, _) => w,
        };
        let turn = Arc::new(Turn {
            state: state.clone(),
            session: Mutex::new(session.clone()),
            // the agent is told where attached files are; images also go to the model directly
            prompt: match handoff {
                Some(h) => format!("{h}\n{text}"),
                None => text.to_string(),
            } + &crate::attachments::note(&attachments),
            attachments: attachments.clone(),
            cwd,
            // a group session sees exactly the member repositories (real paths, not symlinks)
            extra_dirs: members.iter().map(|p| p.path.clone()).collect(),
            system_prompt: context::session_system_prompt(&hub_instructions, scope_ctx),
            wrap,
            env,
            mcp_servers: state.self_url.get().and_then(|url| crate::mcp::hub_mcp(url, session_id)).into_iter().collect(),
            seq: Mutex::new(repo::next_seq(&db, session_id)?),
            cache: Mutex::default(),
            log: Mutex::default(),
            log_path: Path::new(&cfg.logs_dir).join(format!("{session_id}.jsonl")),
        });
        (turn, session, ssh)
    };

    let mut user = json!({ "kind": "user", "text": text });
    if !attachments.is_empty() {
        user["attachments"] = json!(attachments);
    }
    turn.put(&format!("u:{}", now_ms()), user, true);
    let paths: Vec<&str> = attachments.iter().map(|a| a.path.as_str()).collect();
    turn.raw(&json!({ "hub": "user", "ts": db::now(), "text": text, "attachments": paths }).to_string());
    set_status(state, session_id, SessionStatus::Running);

    match agents::start(&session.agent.kind, turn.clone()) {
        Err(message) => {
            turn.put(&format!("err:{}", now_ms()), json!({ "kind": "notice", "level": "error", "text": message }), true);
            set_status(state, session_id, SessionStatus::Error);
        }
        Ok((handle, done)) => {
            rt.active.lock().expect("runtime lock").insert(session_id.to_string(), handle);
            let state = state.clone();
            let id = session_id.to_string();
            tokio::spawn(async move {
                let _ = done.await;
                // the turn is over: its ssh-agent and config go away
                drop(ssh);
                state.runtime.active.lock().expect("runtime lock").remove(&id);
                *turn.log.lock().expect("turn lock") = None;
                let failed = turn.last_is_error();
                set_status(&state, &id, if failed { SessionStatus::Error } else { SessionStatus::Idle });
                // a session started from a kanban card: when it finishes, the card goes to review
                if !failed && let Ok(db) = state.db() {
                    let card: Option<String> = repo::get(&db, &id).ok().flatten().and_then(|s| s.card_id);
                    if let Some(card) = card {
                        let _ = crate::kanban::card_to_review(&db, &card);
                    }
                }
            });
        }
    }
    let db = state.db()?;
    Ok(repo::get(&db, session_id)?.expect("session exists"))
}
