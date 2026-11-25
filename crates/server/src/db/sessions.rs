//! Sessions and their event feed (tables `sessions`, `events`).

use hubness_core::{AgentConfig, HubEvent, Session, SessionStatus};
use rusqlite::{Connection, OptionalExtension, Row};

use super::{new_id, now};

/// Notice added to a session cut off by a hub restart (same text as the TS server).
pub const INTERRUPTED_NOTICE: &str = "Interrupted: hubness restarted. Send a message to continue.";

fn from_row(r: &Row) -> rusqlite::Result<Session> {
    let agent: String = r.get("agent")?;
    Ok(Session {
        id: r.get("id")?,
        project_id: r.get("project_id")?,
        group_id: r.get("group_id")?,
        workspace_id: r.get("workspace_id")?,
        card_id: r.get("card_id")?,
        title: r.get("title")?,
        agent: serde_json::from_str(&agent).unwrap_or(AgentConfig { kind: agent, model: None, permission_mode: None, account: None }),
        agent_session_id: r.get("agent_session_id")?,
        cwd: r.get("cwd")?,
        status: SessionStatus::parse(&r.get::<_, String>("status")?),
        archived_at: r.get("archived_at")?,
        created_at: r.get("created_at")?,
        updated_at: r.get("updated_at")?,
    })
}

/// Which sessions to list.
#[derive(Default)]
pub struct Filter<'a> {
    pub project_id: Option<&'a str>,
    pub group_id: Option<&'a str>,
    /// Only plain chats (no project, no group).
    pub chats: bool,
    pub archived: Archived,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub enum Archived {
    /// Only active sessions.
    #[default]
    No,
    /// Only the archive.
    Only,
    /// Both (checks that must see every session).
    All,
}

/// Sessions newest first (the archive: most recently archived first).
pub fn list(db: &Connection, f: Filter) -> rusqlite::Result<Vec<Session>> {
    let arch = match f.archived {
        Archived::All => "",
        Archived::Only => " AND archived_at IS NOT NULL",
        Archived::No => " AND archived_at IS NULL",
    };
    let order = if f.archived == Archived::Only { "archived_at DESC" } else { "updated_at DESC" };
    let (cond, arg) = if let Some(g) = f.group_id {
        ("group_id = ?", Some(g))
    } else if let Some(p) = f.project_id {
        ("project_id = ?", Some(p))
    } else if f.chats {
        ("project_id IS NULL AND group_id IS NULL", None)
    } else {
        ("1 = 1", None)
    };
    let mut st = db.prepare(&format!("SELECT * FROM sessions WHERE {cond}{arch} ORDER BY {order}"))?;
    match arg {
        Some(a) => st.query_map([a], from_row)?.collect(),
        None => st.query_map([], from_row)?.collect(),
    }
}

pub fn get(db: &Connection, id: &str) -> rusqlite::Result<Option<Session>> {
    db.query_row("SELECT * FROM sessions WHERE id = ?", [id], from_row).optional()
}

pub struct New<'a> {
    pub id: Option<String>,
    pub project_id: Option<&'a str>,
    pub group_id: Option<&'a str>,
    pub workspace_id: Option<&'a str>,
    pub title: &'a str,
    pub agent: &'a AgentConfig,
    pub cwd: Option<&'a str>,
}

pub fn create(db: &Connection, s: New) -> rusqlite::Result<Session> {
    let id = s.id.unwrap_or_else(new_id);
    let t = now();
    db.execute(
        "INSERT INTO sessions (id, project_id, group_id, workspace_id, card_id, title, agent, cwd, status, created_at, updated_at)
         VALUES (?, ?, ?, ?, NULL, ?, ?, ?, 'idle', ?, ?)",
        (&id, s.project_id, s.group_id, s.workspace_id, s.title, agent_json(s.agent), s.cwd, &t, &t),
    )?;
    Ok(get(db, &id)?.expect("session just inserted"))
}

fn agent_json(a: &AgentConfig) -> String {
    serde_json::to_string(a).expect("agent config serializes")
}

#[derive(Default)]
pub struct Patch {
    pub title: Option<String>,
    pub agent: Option<AgentConfig>,
}

/// Update fields; `updated_at` is always bumped.
pub fn update(db: &Connection, id: &str, p: Patch) -> rusqlite::Result<Option<Session>> {
    if let Some(t) = &p.title {
        db.execute("UPDATE sessions SET title = ? WHERE id = ?", (t, id))?;
    }
    if let Some(a) = &p.agent {
        db.execute("UPDATE sessions SET agent = ? WHERE id = ?", (agent_json(a), id))?;
    }
    db.execute("UPDATE sessions SET updated_at = ? WHERE id = ?", (now(), id))?;
    get(db, id)
}

/// Move to the archive or back. Does not bump `updated_at`.
pub fn set_archived(db: &Connection, id: &str, archived: bool) -> rusqlite::Result<Option<Session>> {
    db.execute("UPDATE sessions SET archived_at = ? WHERE id = ?", (archived.then(now), id))?;
    get(db, id)
}

pub fn delete(db: &Connection, id: &str) -> rusqlite::Result<()> {
    db.execute("DELETE FROM sessions WHERE id = ?", [id])?;
    Ok(())
}

// ---------- events ----------

pub fn events(db: &Connection, session_id: &str) -> rusqlite::Result<Vec<HubEvent>> {
    let mut st = db.prepare("SELECT data FROM events WHERE session_id = ? ORDER BY seq")?;
    let rows = st.query_map([session_id], |r| r.get::<_, String>(0))?;
    let mut out = vec![];
    for data in rows {
        if let Ok(v) = serde_json::from_str(&data?) {
            out.push(v);
        }
    }
    Ok(out)
}

pub fn next_seq(db: &Connection, session_id: &str) -> rusqlite::Result<i64> {
    let m: i64 = db.query_row("SELECT COALESCE(MAX(seq), 0) FROM events WHERE session_id = ?", [session_id], |r| r.get(0))?;
    Ok(m + 1)
}

/// Insert or replace an event by its `uid`.
pub fn put_event(db: &Connection, session_id: &str, ev: &HubEvent) -> rusqlite::Result<()> {
    let uid = ev.get("uid").and_then(|v| v.as_str()).unwrap_or_default();
    let seq = ev.get("seq").and_then(|v| v.as_i64()).unwrap_or_default();
    db.execute(
        "INSERT INTO events (session_id, uid, seq, data) VALUES (?, ?, ?, ?)
         ON CONFLICT(session_id, uid) DO UPDATE SET data = excluded.data",
        (session_id, uid, seq, ev.to_string()),
    )?;
    Ok(())
}

/// After a restart no agent is running: sessions cut off mid-run go back to idle and get a
/// notice in the feed (once).
pub fn recover_interrupted(db: &Connection) -> rusqlite::Result<()> {
    let ids: Vec<String> = {
        let mut st = db.prepare("SELECT id FROM sessions WHERE status IN ('running', 'waiting')")?;
        st.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?
    };
    db.execute("UPDATE sessions SET status = 'idle' WHERE status IN ('running', 'waiting')", [])?;
    for id in ids {
        let last = events(db, &id)?.pop();
        let already = last.as_ref().is_some_and(|e| e["kind"] == "notice" && e["text"] == INTERRUPTED_NOTICE);
        if already {
            continue;
        }
        let ev = serde_json::json!({
            "kind": "notice",
            "level": "info",
            "text": INTERRUPTED_NOTICE,
            "uid": format!("restart:{}", jiff::Timestamp::now().as_millisecond()),
            "seq": next_seq(db, &id)?,
            "ts": now(),
        });
        put_event(db, &id, &ev)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interrupted_sessions_get_one_notice() {
        let db = Connection::open_in_memory().unwrap();
        crate::db::init(&db).unwrap();
        let agent = AgentConfig { kind: "codex".into(), model: None, permission_mode: None, account: None };
        let s = create(&db, New { id: None, project_id: None, group_id: None, workspace_id: None, title: "t", agent: &agent, cwd: None })
            .unwrap();
        db.execute("UPDATE sessions SET status = 'running' WHERE id = ?", [&s.id]).unwrap();
        recover_interrupted(&db).unwrap();
        assert_eq!(get(&db, &s.id).unwrap().unwrap().status, SessionStatus::Idle);
        db.execute("UPDATE sessions SET status = 'waiting' WHERE id = ?", [&s.id]).unwrap();
        recover_interrupted(&db).unwrap();
        let evs = events(&db, &s.id).unwrap();
        assert_eq!(evs.len(), 1, "the notice is not repeated");
        assert_eq!(evs[0]["text"], INTERRUPTED_NOTICE);
        assert_eq!(evs[0]["seq"], 1);
    }
}
