//! The Settings tab of a project (the web app's ProjectSettings and Overview): name,
//! description, AI instructions, the repository and its links, groups, the environment
//! (local / Docker, image, sandbox, secrets for agents, the container), the manifest
//! (.hub/hub.yaml) and removing the project (with or without its folder).

use crossterm::event::{KeyCode, KeyEvent};
use hubness_client::Method;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use serde_json::{Value, json};

use super::form::{Form, Row, value};
use crate::core::{Action, Ctx, Handled, Route, Screen};
use crate::screens::session::enc;
use crate::ui::Theme;

#[derive(Clone, PartialEq)]
enum Act {
    Name,
    Description,
    Instructions,
    Url(String),
    Group(String),
    AddGroup,
    NewGroup,
    EnvWhere,
    EnvImage,
    EnvRemoveContainer,
    EnvSandbox,
    EnvShare,
    ManifestCreate,
    Remove,
    DeleteFiles,
}

/// What deleting the folder loses, read from `git status` (the web app's risk list).
pub fn delete_risks(status: Option<&Value>, has_remote: bool) -> Vec<String> {
    let mut out = vec![];
    if let Some(st) = status {
        let files = st["files"].as_array().map(Vec::len).unwrap_or(0);
        if files > 0 {
            out.push(format!("uncommitted files: {files}"));
        }
        if let Some(n) = st["ahead"].as_u64().filter(|n| *n > 0) {
            out.push(format!("unpushed commits: {n}"));
        }
        if let Some(b) = st["branch"].as_str()
            && st["upstream"].is_null()
        {
            out.push(format!("branch “{b}” was never pushed"));
        }
        if let Some(n) = st["stashes"].as_u64().filter(|n| *n > 0) {
            out.push(format!("stash entries: {n}"));
        }
    }
    if !has_remote {
        out.push("the repository has no remote — the whole history".into());
    }
    out
}

/// The last component of a path (what the hub wants as the delete confirmation).
fn folder_name(path: &str) -> String {
    path.split('/').rfind(|s| !s.is_empty()).unwrap_or(path).to_string()
}

pub struct SettingsTab {
    id: String,
    project: Value,
    env: Value,
    manifest: Value,
    groups: Vec<Value>,
    form: Form<Act>,
}

impl SettingsTab {
    pub fn new(id: &str) -> Self {
        Self { id: id.to_string(), project: Value::Null, env: Value::Null, manifest: Value::Null, groups: vec![], form: Form::default() }
    }

    fn path(&self, rest: &str) -> String {
        format!("/projects/{}{rest}", self.id)
    }

    fn normal(&self) -> bool {
        self.project["kind"] != "system"
    }

    fn reload(&self, ctx: &mut Ctx) {
        ctx.get("project", self.path(""));
        ctx.get("env", self.path("/environment"));
        ctx.get("manifest", self.path("/manifest"));
        ctx.get("groups", "/groups");
    }

    fn rebuild(&mut self) {
        let t = Theme::get();
        let p = &self.project;
        let s = |v: &Value| v.as_str().unwrap_or_default().to_string();
        let field = |label: &str, value: Line<'static>, act: Option<Act>| Row::Field { label: label.into(), value, act };
        let mono = |v: &Value| Line::raw(v.as_str().unwrap_or("—").to_string());
        let mut rows = vec![
            Row::Head("Project".into()),
            field("Name", value(&s(&p["name"]), "—"), Some(Act::Name)),
            field("Description", value(&s(&p["description"]), "What this service does"), Some(Act::Description)),
            field("AI instructions", value(&s(&p["instructions"]), "none"), Some(Act::Instructions)),
            Row::Text("Added to every session of this project, on top of AGENTS.md in the repository.".into()),
        ];

        // repository
        let r = &p["repo"];
        rows.push(Row::Head("Repository".into()));
        rows.push(field("Folder", mono(&p["path"]), None));
        rows.push(field("Provider", Line::raw(r["providerLabel"].as_str().unwrap_or("no remote").to_string()), None));
        if r["remoteUrl"].is_string() {
            rows.push(field("origin", mono(&r["remoteUrl"]), None));
        }
        rows.push(field("Branch", mono(&r["branch"]), None));
        rows.push(field("Default branch", mono(&r["defaultBranch"]), None));
        if let Some(langs) = r["languages"].as_array().filter(|l| !l.is_empty()) {
            let text: Vec<String> = langs
                .iter()
                .map(|l| format!("{} {:.1}%", l["name"].as_str().unwrap_or_default(), l["percent"].as_f64().unwrap_or(0.0)))
                .collect();
            rows.push(field("Languages", Line::raw(text.join(" · ")), None));
        }
        if let Some(url) = r["links"]["repo"].as_str() {
            let label = format!("Open on {}", r["providerLabel"].as_str().unwrap_or("the provider"));
            rows.push(field(&label, Line::styled(url.to_string(), t.accent_style()), Some(Act::Url(url.into()))));
        }
        if let Some(url) = r["links"]["newPullRequest"].as_str() {
            let label = if r["provider"] == "gitlab" { "New merge request" } else { "New pull request" };
            rows.push(field(label, Line::styled(url.to_string(), t.accent_style()), Some(Act::Url(url.into()))));
        }

        // groups
        rows.push(Row::Head("In groups".into()));
        let member: Vec<Value> = p["groups"].as_array().cloned().unwrap_or_default();
        if member.is_empty() {
            rows.push(Row::Text("Not in any group yet.".into()));
        }
        for g in &member {
            rows.push(field(
                &s(&g["name"]),
                Line::styled("Enter open · x remove from the group", t.muted_style()),
                Some(Act::Group(s(&g["id"]))),
            ));
        }
        rows.push(field("Add to a group…", Line::raw(""), Some(Act::AddGroup)));
        rows.push(field("New group with it…", Line::raw(""), Some(Act::NewGroup)));

        // environment
        if self.normal() && self.env.is_object() {
            let env = &self.env;
            let rt = &env["runtime"];
            rows.push(Row::Head("Environment".into()));
            let docker = rt["environment"] == "docker";
            rows.push(field(
                "Run and Shell in",
                Line::raw(if docker { "Docker container" } else { "This server" }.to_string()),
                Some(Act::EnvWhere),
            ));
            rows.push(Row::Text(if env["dockerAvailable"] == true {
                "Agents always run on the server (their CLIs and sign-ins live there).".into()
            } else {
                "Agents always run on the server. Docker isn’t available on this server.".into()
            }));
            if docker {
                rows.push(field("Image", mono(&rt["image"]), Some(Act::EnvImage)));
                if let Some(img) = env["devcontainerImage"].as_str() {
                    rows.push(Row::Text(format!("From .devcontainer: {img}")));
                }
                let c = &env["container"];
                if c.is_object() {
                    let state = c["state"].as_str().unwrap_or("absent");
                    let label = match state {
                        "running" => "running".to_string(),
                        "stopped" => "stopped".to_string(),
                        _ => "not created yet — starts with the first Run or Shell".to_string(),
                    };
                    let color = if state == "running" { t.ok } else { t.muted };
                    let value = Line::from(vec![
                        Span::styled("● ", Style::default().fg(color)),
                        Span::raw(format!("{} ", s(&c["name"]))),
                        Span::styled(label, t.muted_style()),
                        Span::styled(if state != "absent" { "   Enter removes it" } else { "" }, t.muted_style()),
                    ]);
                    rows.push(field("Container", value, (state != "absent").then_some(Act::EnvRemoveContainer)));
                }
                rows.push(Row::Text(
                    "One container per project, not root, no Docker socket. For Preview the app must listen on 0.0.0.0.".into(),
                ));
            }
            let onoff = |b: bool| Line::raw(if b { "[x] on" } else { "[ ] off" }.to_string());
            rows.push(field("Sandbox", onoff(rt["sandbox"] == true), Some(Act::EnvSandbox)));
            rows.push(Row::Text(if env["sandboxAvailable"] == true {
                "Agents, Run and Shell see only this repository: the system is read-only; other projects, hubness data, SSH/cloud keys and the Docker socket are hidden.".into()
            } else {
                "bubblewrap (bwrap) isn’t installed on this server.".into()
            }));
            rows.push(field("Secrets to agents", onoff(rt["shareSecretsWithAgents"] == true), Some(Act::EnvShare)));
            rows.push(Row::Text(
                "Agents get project secrets as environment variables — the model may see them. Changing the environment stops what is running in this project.".into(),
            ));
        }

        // manifest
        let m = &self.manifest;
        if m.is_object() {
            rows.push(Row::Head("Manifest".into()));
            if m["exists"] == true {
                rows.push(field("File", mono(&m["path"]), None));
                if let Some(e) = m["error"].as_str() {
                    rows.push(Row::Text(format!("Can’t read .hub/hub.yaml: {e}")));
                }
                let mf = &m["manifest"];
                if let Some(links) = mf["links"].as_object() {
                    for (name, url) in links {
                        let url = url.as_str().unwrap_or_default().to_string();
                        let act = url.starts_with("http").then(|| Act::Url(url.clone()));
                        rows.push(field(&format!("link {name}"), Line::styled(url, t.accent_style()), act));
                    }
                }
                let list = |v: &Value| v.as_array().into_iter().flatten().filter_map(|x| x.as_str()).map(String::from).collect::<Vec<_>>();
                for host in list(&mf["ssh"]) {
                    rows.push(field("ssh", Line::raw(host), None));
                }
                for svc in list(&mf["services"]) {
                    let st = m["services"].as_array().into_iter().flatten().find(|x| x["name"] == svc.as_str());
                    let state = st.map(|x| format!("  {} {}", s(&x["state"]), s(&x["status"]))).unwrap_or_default();
                    rows.push(field("container", Line::from(vec![Span::raw(svc), Span::styled(state, t.muted_style())]), None));
                }
                for u in list(&mf["units"]) {
                    rows.push(field("unit", Line::raw(u), None));
                }
                for d in list(&mf["docs"]) {
                    rows.push(field("doc", Line::raw(d), None));
                }
                if let Some(n) = mf["notes"].as_str().filter(|n| !n.trim().is_empty()) {
                    rows.push(field("Notes", Line::raw(""), None));
                    rows.push(Row::Text(n.to_string()));
                }
                rows.push(Row::Text("Agents get the same information as context. Edit .hub/hub.yaml in the Files tab.".into()));
            } else {
                rows.push(Row::Text(
                    "No project manifest yet. The manifest is shared memory for you and the agents: dashboard links, SSH hosts, containers, docs."
                        .into(),
                ));
                rows.push(field("Create manifest", Line::raw(""), Some(Act::ManifestCreate)));
            }
        }

        // removal
        if self.normal() && p.is_object() {
            rows.push(Row::Head("Danger zone".into()));
            rows.push(field(
                "Remove from hubness",
                Line::styled("files stay on disk; sessions of this project are deleted", t.muted_style()),
                Some(Act::Remove),
            ));
            rows.push(field(
                "Delete with files",
                Line::styled("removes the project and deletes its folder — can’t be undone", Style::default().fg(t.danger)),
                Some(Act::DeleteFiles),
            ));
        }
        self.form.set(rows);
    }

    fn run(&mut self, act: Act, ctx: &mut Ctx) {
        let p = &self.project;
        let s = |k: &str| p[k].as_str().unwrap_or_default().to_string();
        match act {
            Act::Name => ctx.prompt("name", "Project name", s("name")),
            Act::Description => ctx.prompt_multiline("description", "Description (Enter saves, Alt+Enter newline)", s("description")),
            Act::Instructions => {
                ctx.prompt_multiline("instructions", "Instructions for AI agents (Enter saves, Alt+Enter newline)", s("instructions"))
            }
            Act::Url(url) => ctx.open_url(url),
            Act::Group(id) => ctx.go(Route::Group(id)),
            Act::AddGroup => {
                let member: Vec<&str> = p["groups"].as_array().into_iter().flatten().filter_map(|g| g["id"].as_str()).collect();
                let items: Vec<(String, String)> = self
                    .groups
                    .iter()
                    .filter(|g| !member.contains(&g["id"].as_str().unwrap_or_default()))
                    .map(|g| (g["id"].as_str().unwrap_or_default().to_string(), g["name"].as_str().unwrap_or_default().to_string()))
                    .collect();
                if items.is_empty() {
                    ctx.info("no other groups — create one with “New group with it…”");
                } else {
                    ctx.pick("add-group", "Add to a group", items);
                }
            }
            Act::NewGroup => ctx.prompt("new-group", "New group with this project — name", ""),
            Act::EnvWhere => {
                let docker = self.env["runtime"]["environment"] == "docker";
                if !docker && self.env["dockerAvailable"] != true {
                    return ctx.error("Docker isn’t available on this server");
                }
                let next = if docker { "local" } else { "docker" };
                ctx.confirm(
                    &format!("env-where:{next}"),
                    "Where Run and Shell execute",
                    format!(
                        "Switch to {}? Changing the environment stops what is running in this project.",
                        if docker { "this server" } else { "a Docker container" }
                    ),
                    false,
                );
            }
            Act::EnvImage => ctx.prompt("env-image", "Container image (the project folder is mounted at /workspace)", self.env["runtime"]["image"].as_str().unwrap_or_default()),
            Act::EnvRemoveContainer => ctx.confirm("env-remove", "Remove container", "Remove the project container? It is created again with the next Run or Shell.", true),
            Act::EnvSandbox => {
                let on = self.env["runtime"]["sandbox"] == true;
                if !on && self.env["sandboxAvailable"] != true {
                    return ctx.error("bubblewrap (bwrap) isn’t installed on this server");
                }
                ctx.send("env", Method::PUT, self.path("/environment"), json!({ "sandbox": !on }));
            }
            Act::EnvShare => {
                let on = self.env["runtime"]["shareSecretsWithAgents"] == true;
                ctx.send("env", Method::PUT, self.path("/environment"), json!({ "shareSecretsWithAgents": !on }));
            }
            Act::ManifestCreate => ctx.send("manifest", Method::POST, self.path("/manifest"), Value::Null),
            Act::Remove => ctx.confirm(
                "remove",
                "Remove from hubness",
                format!("Remove project “{}” from hubness? The folder on disk stays.", s("name")),
                true,
            ),
            Act::DeleteFiles => ctx.confirm(
                "files1",
                "Delete project from disk",
                format!(
                    "The project folder will be deleted from disk for good, with everything in it:\n{}\n\nSessions of this project are deleted too.",
                    s("path")
                ),
                true,
            ),
        }
    }

    fn gone(&self, ctx: &mut Ctx) {
        ctx.shared.projects.retain(|p| p["id"] != self.id.as_str());
        ctx.shared.sessions.retain(|s| s["projectId"] != self.id.as_str());
        ctx.go(Route::Projects);
    }
}

impl Screen for SettingsTab {
    fn title(&self) -> String {
        "Settings".into()
    }

    fn on_show(&mut self, ctx: &mut Ctx) {
        self.reload(ctx);
    }

    fn actions(&self) -> Vec<Action> {
        let mut a = vec![
            Action::new("n", "Rename"),
            Action::new("d", "Description"),
            Action::new("I", "AI instructions"),
            Action::new("o", "Open the repository in the browser"),
            Action::new("a", "Add to a group"),
            Action::new("N", "New group with this project"),
        ];
        if matches!(self.form.current(), Some(Act::Group(_))) {
            a.push(Action::new("x", "Remove from the selected group"));
        }
        if self.normal() {
            a.extend([
                Action::new("w", "Where Run and Shell execute (local / Docker)"),
                Action::new("S", "Toggle the sandbox"),
                Action::new("s", "Toggle secrets for agents"),
            ]);
        }
        if self.manifest["exists"] == false {
            a.push(Action::new("c", "Create the manifest (.hub/hub.yaml)"));
        }
        if self.normal() {
            a.extend([Action::new("D", "Remove from hubness"), Action::new("X", "Delete with files")]);
        }
        a.push(Action::new("r", "Reload"));
        a
    }

    fn on_action(&mut self, key: &str, ctx: &mut Ctx) {
        let act = match key {
            "n" => Act::Name,
            "d" => Act::Description,
            "I" => Act::Instructions,
            "o" => match self.project["repo"]["links"]["repo"].as_str() {
                Some(u) => Act::Url(u.into()),
                None => return ctx.info("the repository has no web page (no known provider)"),
            },
            "a" => Act::AddGroup,
            "N" => Act::NewGroup,
            "x" => {
                if let Some(Act::Group(gid)) = self.form.current() {
                    let name = self.groups.iter().find(|g| g["id"] == gid.as_str()).and_then(|g| g["name"].as_str()).unwrap_or("the group");
                    ctx.confirm(&format!("leave:{gid}"), "Remove from the group", format!("Remove this project from {name}?"), false);
                }
                return;
            }
            "w" => Act::EnvWhere,
            "S" => Act::EnvSandbox,
            "s" => Act::EnvShare,
            "c" => Act::ManifestCreate,
            "D" => Act::Remove,
            "X" => Act::DeleteFiles,
            "r" => return self.reload(ctx),
            _ => return,
        };
        self.run(act, ctx);
    }

    fn on_key(&mut self, k: KeyEvent, ctx: &mut Ctx) -> Handled {
        match k.code {
            KeyCode::Down | KeyCode::Char('j') => self.form.step(1),
            KeyCode::Up | KeyCode::Char('k') => self.form.step(-1),
            KeyCode::PageDown => self.form.step(10),
            KeyCode::PageUp => self.form.step(-10),
            KeyCode::Enter => {
                if let Some(a) = self.form.current() {
                    self.run(a, ctx);
                }
            }
            _ => return Handled::No,
        }
        Handled::Yes
    }

    fn on_task(&mut self, key: &str, result: anyhow::Result<Value>, ctx: &mut Ctx) {
        if key == "files-status" {
            // the risks are read right before asking; a failed status still lets the user decide
            let status = result.ok();
            let folder = folder_name(self.project["path"].as_str().unwrap_or_default());
            let risks = delete_risks(status.as_ref(), self.project["repo"]["remoteUrl"].is_string());
            let mut msg = format!("Last check: delete the folder “{folder}” for good?\n");
            if !risks.is_empty() {
                msg.push_str("\nWill be lost — it is not saved anywhere else:\n");
                for r in risks {
                    msg.push_str(&format!("• {r}\n"));
                }
            }
            msg.push_str("\nOther local branches that were not pushed are lost too.");
            ctx.confirm("files2", "Delete project from disk", msg, true);
            return;
        }
        let Some(v) = ctx.ok(result) else { return };
        let patch = |field: &str, v: &Value| v.as_str().map(|s| json!({ field: s }));
        match key {
            "project" => self.project = v,
            "env" => self.env = v,
            "manifest" => self.manifest = v,
            "groups" => {
                self.groups = v.as_array().cloned().unwrap_or_default();
                ctx.shared.groups = self.groups.clone();
            }
            "name" => {
                if let Some(b) = patch("name", &v).filter(|_| v.as_str().is_some_and(|s| !s.trim().is_empty())) {
                    ctx.send("saved", Method::PATCH, self.path(""), b);
                }
            }
            "description" | "instructions" => {
                if let Some(b) = patch(key, &v) {
                    ctx.send("saved", Method::PATCH, self.path(""), b);
                }
            }
            "saved" => {
                ctx.info("saved");
                if let Some(p) = ctx.shared.projects.iter_mut().find(|p| p["id"] == self.id.as_str()) {
                    *p = v;
                }
                self.reload(ctx);
            }
            "add-group" => {
                if let Some(gid) = v.as_str() {
                    self.set_member(gid, true, ctx);
                }
            }
            k if k.starts_with("leave:") && v == Value::Bool(true) => {
                let gid = k[6..].to_string();
                self.set_member(&gid, false, ctx);
            }
            "new-group" => {
                if let Some(name) = v.as_str().map(str::trim).filter(|s| !s.is_empty()) {
                    ctx.send("group-saved", Method::POST, "/groups", json!({ "name": name, "projectIds": [self.id] }));
                }
            }
            "group-saved" => {
                ctx.info("groups updated");
                self.reload(ctx);
            }
            k if k.starts_with("env-where:") && v == Value::Bool(true) => {
                ctx.send("env", Method::PUT, self.path("/environment"), json!({ "environment": &k[10..] }));
            }
            "env-image" => {
                if let Some(img) = v.as_str().map(str::trim).filter(|s| !s.is_empty()) {
                    ctx.send("env", Method::PUT, self.path("/environment"), json!({ "image": img }));
                }
            }
            "env-remove" if v == Value::Bool(true) => {
                ctx.send("env", Method::POST, self.path("/environment/container/remove"), Value::Null);
            }
            "remove" if v == Value::Bool(true) => ctx.send("removed", Method::DELETE, self.path(""), Value::Null),
            "files1" if v == Value::Bool(true) => ctx.get("files-status", self.path("/git/status")),
            "files2" if v == Value::Bool(true) => {
                let folder = folder_name(self.project["path"].as_str().unwrap_or_default());
                ctx.send("removed", Method::DELETE, self.path(&format!("?files=1&confirm={}", enc(&folder))), Value::Null);
            }
            "removed" => {
                ctx.info(match v["deleted"].as_str() {
                    Some(d) => format!("project removed, {d} deleted"),
                    None => "project removed from hubness".to_string(),
                });
                self.gone(ctx);
            }
            _ => {}
        }
        self.rebuild();
    }

    fn draw(&mut self, f: &mut Frame, area: Rect, _ctx: &Ctx) {
        if self.form.rows.is_empty() {
            self.rebuild();
        }
        self.form.draw(f, area, "settings — j/k move · Enter change / open");
    }
}

impl SettingsTab {
    fn set_member(&mut self, gid: &str, add: bool, ctx: &mut Ctx) {
        let Some(g) = self.groups.iter().find(|g| g["id"] == gid) else { return };
        let mut ids: Vec<String> = g["projectIds"].as_array().into_iter().flatten().filter_map(|x| x.as_str().map(String::from)).collect();
        ids.retain(|x| x != &self.id);
        if add {
            ids.push(self.id.clone());
        }
        ctx.send("group-saved", Method::PATCH, format!("/groups/{gid}"), json!({ "projectIds": ids }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn risks() {
        let st = json!({ "files": [{}, {}], "ahead": 3, "branch": "dev", "upstream": null, "stashes": 1 });
        let r = delete_risks(Some(&st), false);
        assert_eq!(r.len(), 5);
        assert_eq!(r[0], "uncommitted files: 2");
        let clean = json!({ "files": [], "ahead": 0, "branch": "main", "upstream": "origin/main", "stashes": 0 });
        assert!(delete_risks(Some(&clean), true).is_empty());
        assert_eq!(delete_risks(None, true).len(), 0);
    }

    #[test]
    fn folders() {
        assert_eq!(folder_name("/home/u/hub/projects/api/"), "api");
        assert_eq!(folder_name("/x/y"), "y");
    }
}
