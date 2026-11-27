//! What a signed-in member may do (admins, the hub's own processes and a hub without accounts
//! may do everything). See `hubness_core::Permissions`.
//!
//! Two layers:
//! - `enforce`, an `/api` middleware after `auth::guard`, answers 403 for whole routes the
//!   member has no access to: by section (git, files, terminal…) and by the project, group,
//!   session or card in the path. Anything it doesn't know is admin-only.
//! - lists and streams (projects, groups, sessions, the global stream) are filtered in their
//!   handlers with `Access`, and creating a session checks the scope, agent and model there.

use axum::extract::Request;
use axum::http::Method;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use hubness_core::{Permissions, ProjectGroup, Section, Session};
use rusqlite::{Connection, OptionalExtension};

use crate::auth::{Caller, Principal};
use crate::error::{ApiResult, HttpError};

/// The access of one request.
#[derive(Clone, Debug)]
pub enum Access {
    Full,
    Member { user_id: String, perms: Permissions },
}

impl Access {
    pub fn of(caller: Option<&Caller>) -> Self {
        match caller.and_then(|c| c.principal.as_ref()) {
            Some(Principal::User { user, .. }) if !user.is_admin() => {
                Access::Member { user_id: user.id.clone(), perms: user.permissions.clone().unwrap_or_default() }
            }
            // `None`: tests that build the router without the guard
            _ => Access::Full,
        }
    }

    pub fn is_full(&self) -> bool {
        matches!(self, Access::Full)
    }

    /// The signed-in member's id (`None` for full access).
    pub fn user_id(&self) -> Option<&str> {
        match self {
            Access::Full => None,
            Access::Member { user_id, .. } => Some(user_id),
        }
    }

    pub fn section(&self, s: Section) -> bool {
        match self {
            Access::Full => true,
            Access::Member { perms, .. } => perms.sections.contains(&s),
        }
    }

    pub fn project(&self, id: &str) -> bool {
        match self {
            Access::Full => true,
            Access::Member { perms, .. } => perms.projects.as_ref().is_none_or(|l| l.iter().any(|p| p == id)),
        }
    }

    /// A group is open when every one of its projects is (a group session works in all of them).
    pub fn group(&self, g: &ProjectGroup) -> bool {
        self.group_projects(&g.project_ids)
    }

    fn group_projects(&self, ids: &[String]) -> bool {
        match self {
            Access::Full => true,
            Access::Member { perms, .. } => perms.projects.is_none() || (!ids.is_empty() && ids.iter().all(|p| self.project(p))),
        }
    }

    fn group_id(&self, db: &Connection, id: &str) -> rusqlite::Result<bool> {
        if self.is_full() {
            return Ok(true);
        }
        let mut st = db.prepare("SELECT project_id FROM group_members WHERE group_id = ?")?;
        let ids = st.query_map([id], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        let exists: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM project_groups WHERE id = ?)", [id], |r| r.get(0))?;
        // an unknown group: let the handler answer 404
        Ok(!exists || self.group_projects(&ids))
    }

    /// Project and group sessions follow their scope; a regular chat belongs to whoever
    /// started it (older chats without an owner are for admins).
    pub fn session(&self, db: &Connection, s: &Session) -> rusqlite::Result<bool> {
        self.scope(db, s.project_id.as_deref(), s.group_id.as_deref(), &s.id)
    }

    fn scope(&self, db: &Connection, project: Option<&str>, group: Option<&str>, session_id: &str) -> rusqlite::Result<bool> {
        let Access::Member { user_id, .. } = self else { return Ok(true) };
        if let Some(p) = project {
            return Ok(self.project(p));
        }
        if let Some(g) = group {
            return self.group_id(db, g);
        }
        Ok(owner(db, session_id)?.as_deref() == Some(user_id.as_str()))
    }

    /// May the member start or switch to this agent and model?
    pub fn agent(&self, kind: &str, model: Option<&str>) -> ApiResult<()> {
        let Access::Member { perms, .. } = self else { return Ok(()) };
        if perms.agents.as_ref().is_some_and(|l| !l.iter().any(|a| a == kind)) {
            return Err(HttpError::new(403, format!("You have no access to the agent {kind}")));
        }
        if let Some(models) = &perms.models {
            match model {
                Some(m) if models.iter().any(|x| x == m) => {}
                Some(m) => return Err(HttpError::new(403, format!("You have no access to the model {m}"))),
                None => return Err(HttpError::new(403, "Pick one of the models you have access to")),
            }
        }
        Ok(())
    }
}

fn owner(db: &Connection, session_id: &str) -> rusqlite::Result<Option<String>> {
    Ok(db.query_row("SELECT created_by FROM sessions WHERE id = ?", [session_id], |r| r.get(0)).optional()?.flatten())
}

/// Remember who started a session (nothing for full access without an account).
pub fn set_owner(db: &Connection, session_id: &str, caller: Option<&Caller>) -> rusqlite::Result<()> {
    if let Some(user) = caller.and_then(|c| c.user()) {
        db.execute("UPDATE sessions SET created_by = ? WHERE id = ?", (&user.id, session_id))?;
    }
    Ok(())
}

/// What a route needs from a member.
#[derive(Debug, PartialEq)]
enum Need {
    Nothing,
    Admin,
    Area(Section),
    Project(String, Option<Section>),
    Group(String, Option<Section>),
    Session(String),
    Card(String),
    /// Every project in the `projects` query parameter.
    Projects,
    /// `/users/{id}/password`: one's own, or an admin.
    SelfOr(String),
}

/// The rule for a route (`path` without the `/api` prefix).
fn need(method: &Method, path: &str) -> Need {
    use Need::*;
    // nested under `/api`, the router sees the path without the prefix; accept both
    let path = path.strip_prefix("/api").unwrap_or(path);
    let segs: Vec<&str> = path.trim_matches('/').split('/').filter(|s| !s.is_empty()).collect();
    let get = method == Method::GET;
    match segs.as_slice() {
        ["health"] | ["auth", ..] => Nothing,
        ["users", id, "password"] => SelfOr(id.to_string()),
        ["users", ..] => Admin,
        ["settings"] if get => Nothing,
        ["settings"] => Area(Section::Settings),
        // the list is filtered by the handler
        ["projects"] if get => Nothing,
        ["projects", "discover" | "import" | "new-repo" | "clone"] => Admin,
        ["projects", id] if get => Project(id.to_string(), None),
        ["projects", _] => Admin,
        ["projects", id, rest @ ..] => match project_section(method, rest) {
            Ok(s) => Project(id.to_string(), s),
            Err(()) => Admin,
        },
        ["groups"] if get => Nothing,
        ["groups", id] | ["groups", id, "context"] if get => Group(id.to_string(), None),
        ["groups", id, "shell" | "shells"] => Group(id.to_string(), Some(Section::Terminal)),
        ["groups", id, "ssh"] if get => Group(id.to_string(), None),
        ["groups", ..] => Admin,
        // filtered or checked by the handlers
        ["sessions"] | ["sessions", "archive"] | ["stream"] => Nothing,
        ["sessions", id, ..] => Session(id.to_string()),
        ["cards", id, ..] => Card(id.to_string()),
        ["watch"] => Projects,
        ["jobs", ..] => Area(Section::Terminal),
        ["agents"] | ["agents", _, "models"] if get => Nothing,
        ["harness", ..] => Area(Section::Harness),
        ["system", ..] | ["ask"] => Area(Section::System),
        ["git-credentials", ..] => Area(Section::Settings),
        ["ssh", ..] => Area(Section::System),
        ["voice"] if get => Nothing,
        ["voice", "transcribe"] => Nothing,
        ["voice", ..] => Area(Section::Settings),
        // the host's whole file system, and everything not listed above
        _ => Admin,
    }
}

/// The section a project sub-route needs (`Err` — admins only).
fn project_section(method: &Method, rest: &[&str]) -> Result<Option<Section>, ()> {
    let get = method == Method::GET;
    Ok(match rest {
        ["git", ..] => Some(Section::Git),
        // the file list feeds @-mentions in the chat composer
        ["files"] if get => None,
        ["file" | "files" | "fs" | "search" | "tree", ..] => Some(Section::Files),
        ["manifest"] | ["ssh"] if get => None,
        ["manifest", ..] => Some(Section::Files),
        ["runs" | "shell" | "shells" | "preview" | "previews" | "environment", ..] => Some(Section::Terminal),
        ["cards", ..] => Some(Section::Kanban),
        _ => return Err(()),
    })
}

fn forbidden(msg: &str) -> Response {
    HttpError::new(403, msg.to_string()).into_response()
}

/// `/api` middleware: refuse what the member has no access to (see the module docs).
pub async fn enforce(req: Request, next: Next) -> Response {
    let caller = req.extensions().get::<Caller>().cloned();
    let access = Access::of(caller.as_ref());
    if access.is_full() {
        return next.run(req).await;
    }
    let rule = need(req.method(), req.uri().path());
    let ok = match check(&access, &rule, req.uri().query()) {
        Ok(ok) => ok,
        Err(e) => return e.into_response(),
    };
    if !ok {
        return forbidden(match rule {
            Need::Area(_) | Need::Project(_, Some(_)) | Need::Group(_, Some(_)) => "You have no access to this section",
            Need::Admin | Need::SelfOr(_) => "Only an admin can do this",
            _ => "You have no access to this",
        });
    }
    next.run(req).await
}

fn check(access: &Access, rule: &Need, query: Option<&str>) -> ApiResult<bool> {
    let section = |s: &Option<Section>| s.is_none_or(|s| access.section(s));
    Ok(match rule {
        Need::Nothing => true,
        Need::Admin => false,
        Need::SelfOr(id) => access.user_id() == Some(id.as_str()),
        Need::Area(s) => access.section(*s),
        Need::Project(id, s) => access.project(id) && section(s),
        Need::Group(id, s) => section(s) && access.group_id(&*db()?, id)?,
        Need::Session(id) => {
            let db = db()?;
            let row = db
                .query_row("SELECT project_id, group_id FROM sessions WHERE id = ?", [id], |r| {
                    Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?))
                })
                .optional()?;
            // an unknown session: the handler answers 404
            match row {
                Some((p, g)) => access.scope(&db, p.as_deref(), g.as_deref(), id)?,
                None => true,
            }
        }
        Need::Card(id) => {
            let project: Option<String> = db()?.query_row("SELECT project_id FROM cards WHERE id = ?", [id], |r| r.get(0)).optional()?;
            access.section(Section::Kanban) && project.is_none_or(|p| access.project(&p))
        }
        Need::Projects => {
            let ids = query
                .into_iter()
                .flat_map(|q| q.split('&'))
                .filter_map(|kv| kv.strip_prefix("projects="))
                .flat_map(|v| v.split("%2C").flat_map(|v| v.split(',')))
                .filter(|v| !v.is_empty())
                .collect::<Vec<_>>();
            access.section(Section::Files) && ids.iter().all(|p| access.project(p))
        }
    })
}

fn db() -> ApiResult<std::sync::MutexGuard<'static, Connection>> {
    crate::auth::db_conn().ok_or_else(|| HttpError::new(503, "Accounts are not available on this server"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(perms: Permissions) -> Access {
        Access::Member { user_id: "u".into(), perms }
    }

    #[test]
    fn routes_map_to_rules() {
        let g = Method::GET;
        let p = Method::POST;
        assert_eq!(need(&g, "/api/projects"), Need::Nothing);
        assert_eq!(need(&p, "/api/projects/import"), Need::Admin);
        assert_eq!(need(&Method::DELETE, "/api/projects/1"), Need::Admin);
        assert_eq!(need(&g, "/api/projects/1/git/status"), Need::Project("1".into(), Some(Section::Git)));
        assert_eq!(need(&g, "/api/projects/1/files"), Need::Project("1".into(), None));
        assert_eq!(need(&Method::PUT, "/api/projects/1/file"), Need::Project("1".into(), Some(Section::Files)));
        assert_eq!(need(&g, "/api/projects/1/secrets"), Need::Admin);
        assert_eq!(need(&p, "/api/projects/1/shell"), Need::Project("1".into(), Some(Section::Terminal)));
        assert_eq!(need(&g, "/api/sessions/s/stream"), Need::Session("s".into()));
        assert_eq!(need(&g, "/api/system/overview"), Need::Area(Section::System));
        assert_eq!(need(&g, "/api/harness"), Need::Area(Section::Harness));
        assert_eq!(need(&Method::PUT, "/api/settings"), Need::Area(Section::Settings));
        assert_eq!(need(&g, "/api/fs/browse"), Need::Admin);
        assert_eq!(need(&g, "/api/users"), Need::Admin);
        assert_eq!(need(&Method::PUT, "/api/users/u/password"), Need::SelfOr("u".into()));
        assert_eq!(need(&g, "/api/something-new"), Need::Admin, "unknown routes are admin-only");
    }

    #[test]
    fn member_permissions() {
        let m = member(Permissions {
            projects: Some(vec!["a".into(), "b".into()]),
            sections: vec![Section::Git],
            agents: Some(vec!["claude".into()]),
            models: None,
        });
        assert!(m.project("a") && !m.project("c"));
        assert!(m.section(Section::Git) && !m.section(Section::Terminal));
        let group = |ids: &[&str]| ProjectGroup {
            id: "g".into(),
            name: String::new(),
            description: String::new(),
            instructions: String::new(),
            folder: None,
            project_ids: ids.iter().map(|s| s.to_string()).collect(),
            created_at: String::new(),
            updated_at: String::new(),
        };
        assert!(m.group(&group(&["a", "b"])));
        assert!(!m.group(&group(&["a", "c"])), "a group needs all its projects");
        assert!(!m.group(&group(&[])));
        assert!(m.agent("claude", None).is_ok());
        assert_eq!(m.agent("codex", None).unwrap_err().status.as_u16(), 403);

        let m = member(Permissions { models: Some(vec!["opus".into()]), ..Default::default() });
        assert!(m.agent("claude", Some("opus")).is_ok());
        assert!(m.agent("claude", Some("haiku")).is_err() && m.agent("claude", None).is_err());
        assert!(!m.project("a"), "a new member has no projects");
        assert!(Access::Full.agent("x", None).is_ok() && Access::Full.project("x"));
    }
}
