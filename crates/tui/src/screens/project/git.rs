//! The Git tab (the web app's lazygit-style git page): status with ahead / behind and a
//! half-done merge / rebase (continue, abort); changed files with their diff, stage /
//! unstage / discard, commit and stash; branches (switch, create, merge, rebase, delete);
//! the log with a commit's patch (branch here, cherry-pick, revert, reset); stashes (apply,
//! pop, drop); fetch / pull / push with sign-in when the remote refuses access.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use hubness_client::Method;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph, Wrap};
use serde_json::{Value, json};

use super::{Opener, enc};
use crate::core::{Action, Ctx, Handled, Screen};
use crate::ui::{Theme, block, code};

/// Changed files split like the web app: rows to stage (worktree changes, untracked,
/// conflicted) first, then staged rows. A file can appear in both.
pub fn file_rows(files: &[Value]) -> Vec<(Value, bool)> {
    let staged: Vec<&Value> = files.iter().filter(|f| f["index"] != " " && f["untracked"] != true && f["conflicted"] != true).collect();
    let unstaged: Vec<&Value> =
        files.iter().filter(|f| f["worktree"] != " " || f["untracked"] == true || f["conflicted"] == true).collect();
    unstaged.into_iter().map(|f| (f.clone(), false)).chain(staged.into_iter().map(|f| (f.clone(), true))).collect()
}

/// The two status letters a row shows (lazygit style): index side, worktree side.
pub fn status_letters(f: &Value, staged: bool) -> (String, String) {
    let s = |k: &str| f[k].as_str().unwrap_or(" ").to_string();
    if f["conflicted"] == true {
        ("U".into(), "U".into())
    } else if f["untracked"] == true {
        ("?".into(), "?".into())
    } else if staged {
        (s("index"), " ".into())
    } else {
        (" ".into(), s("worktree"))
    }
}

/// What a refused remote means for the user.
pub fn auth_message(auth: &Value) -> String {
    let host = auth["host"].as_str().unwrap_or("?");
    match (auth["protocol"].as_str(), auth["reason"].as_str()) {
        (_, Some("hostkey")) => format!("{host} is not a known ssh host (add it to known_hosts on the server)"),
        (Some("ssh"), _) => format!("{host} accepted no ssh key of the server"),
        (_, Some("rejected")) => format!("{host} refused the stored login — L to sign in again"),
        _ => format!("No login stored for {host} — L to sign in"),
    }
}

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
enum Panel {
    Files,
    Branches,
    Commits,
    Stash,
}

const PANELS: [Panel; 4] = [Panel::Files, Panel::Branches, Panel::Commits, Panel::Stash];

struct OpResult {
    name: String,
    ok: bool,
    output: String,
    auth: Value,
}

pub struct GitTab {
    id: String,
    opener: Opener,
    status: Value,
    branches: Vec<Value>,
    commits: Vec<Value>,
    more: bool,
    log_all: bool,
    log_limit: usize,
    stashes: Vec<Value>,
    panel: Panel,
    lists: [ListState; 4],
    detail: Vec<Line<'static>>,
    detail_id: String,
    detail_seq: u64,
    detail_scroll: usize,
    detail_height: usize,
    busy: Option<String>,
    last_op: Option<(String, String, Value)>,
    result: Option<OpResult>,
    /// Sign-in being collected: (host, kind, username).
    login: Option<(String, String, String)>,
    watching: bool,
}

impl GitTab {
    pub fn new(id: &str, opener: Opener) -> Self {
        Self {
            id: id.to_string(),
            opener,
            status: Value::Null,
            branches: vec![],
            commits: vec![],
            more: false,
            log_all: true,
            log_limit: 150,
            stashes: vec![],
            panel: Panel::Files,
            lists: Default::default(),
            detail: vec![],
            detail_id: String::new(),
            detail_seq: 0,
            detail_scroll: 0,
            detail_height: 20,
            busy: None,
            last_op: None,
            result: None,
            login: None,
            watching: false,
        }
    }

    fn api(&self, rest: &str) -> String {
        format!("/projects/{}/git{rest}", self.id)
    }

    fn load_status(&self, ctx: &mut Ctx) {
        ctx.get("status", self.api("/status"));
    }

    fn load_rest(&self, ctx: &mut Ctx) {
        ctx.get("branches", self.api("/branches"));
        ctx.get("log", self.api(&format!("/log?limit={}&all={}", self.log_limit, if self.log_all { 1 } else { 0 })));
        ctx.get("stashes", self.api("/stashes"));
    }

    fn files(&self) -> Vec<(Value, bool)> {
        file_rows(self.status["files"].as_array().map(Vec::as_slice).unwrap_or_default())
    }

    /// Local branches first, then remote ones.
    fn branch_rows(&self) -> Vec<Value> {
        let local = self.branches.iter().filter(|b| b["remote"] != true);
        let remote = self.branches.iter().filter(|b| b["remote"] == true);
        local.chain(remote).cloned().collect()
    }

    fn idx(p: Panel) -> usize {
        PANELS.iter().position(|x| *x == p).unwrap_or(0)
    }

    fn len(&self, p: Panel) -> usize {
        match p {
            Panel::Files => self.files().len(),
            Panel::Branches => self.branches.len(),
            Panel::Commits => self.commits.len(),
            Panel::Stash => self.stashes.len(),
        }
    }

    fn sel(&self, p: Panel) -> Option<usize> {
        let n = self.len(p);
        if n == 0 { None } else { Some(self.lists[Self::idx(p)].selected().unwrap_or(0).min(n - 1)) }
    }

    fn sel_file(&self) -> Option<(Value, bool)> {
        self.sel(Panel::Files).and_then(|i| self.files().get(i).cloned())
    }

    fn sel_branch(&self) -> Option<Value> {
        self.sel(Panel::Branches).and_then(|i| self.branch_rows().get(i).cloned())
    }

    fn sel_commit(&self) -> Option<Value> {
        self.sel(Panel::Commits).and_then(|i| self.commits.get(i).cloned())
    }

    fn sel_stash(&self) -> Option<Value> {
        self.sel(Panel::Stash).and_then(|i| self.stashes.get(i).cloned())
    }

    fn move_by(&mut self, d: isize) {
        let n = self.len(self.panel);
        if n == 0 {
            return;
        }
        let i = (self.sel(self.panel).unwrap_or(0) as isize + d).clamp(0, n as isize - 1) as usize;
        self.lists[Self::idx(self.panel)].select(Some(i));
    }

    fn cycle(&mut self, d: isize) {
        let i = (Self::idx(self.panel) as isize + d).rem_euclid(PANELS.len() as isize) as usize;
        self.panel = PANELS[i];
    }

    /// Load the main view for the selection (diff, patch); `force` re-reads it.
    fn load_detail(&mut self, force: bool, ctx: &mut Ctx) {
        let (id, path): (String, Option<String>) = match self.panel {
            Panel::Files => match self.sel_file() {
                Some((f, staged)) => {
                    let p = f["path"].as_str().unwrap_or_default().to_string();
                    let mut q = format!("/diff?path={}", enc(&p));
                    if staged {
                        q.push_str("&staged=1");
                    }
                    if f["untracked"] == true {
                        q.push_str("&untracked=1");
                    }
                    (format!("f:{staged}:{p}"), Some(self.api(&q)))
                }
                None => (String::new(), None),
            },
            Panel::Branches => (self.sel_branch().map(|b| format!("b:{}", b["name"])).unwrap_or_default(), None),
            Panel::Commits => match self.sel_commit() {
                Some(c) => {
                    let h = c["hash"].as_str().unwrap_or_default().to_string();
                    (format!("c:{h}"), Some(self.api(&format!("/show?rev={}", enc(&h)))))
                }
                None => (String::new(), None),
            },
            Panel::Stash => match self.sel_stash() {
                Some(s) => {
                    let r = s["ref"].as_str().unwrap_or_default().to_string();
                    (format!("s:{r}"), Some(self.api(&format!("/show?rev={}", enc(&r)))))
                }
                None => (String::new(), None),
            },
        };
        if id == self.detail_id && !force {
            return;
        }
        self.detail_id = id;
        self.detail_scroll = 0;
        self.detail_seq += 1;
        self.detail.clear();
        if let Some(path) = path {
            ctx.get(format!("detail:{}", self.detail_seq), path);
        }
    }

    fn run(&mut self, op: &str, label: &str, body: Value, ctx: &mut Ctx) {
        if let Some(b) = &self.busy {
            ctx.error(format!("wait for “{b}” to finish"));
            return;
        }
        self.busy = Some(label.to_string());
        self.last_op = Some((op.to_string(), label.to_string(), body.clone()));
        ctx.send("op", Method::POST, self.api(&format!("/{op}")), body);
    }

    fn header_lines(&self) -> Vec<Line<'static>> {
        let t = Theme::get();
        let s = &self.status;
        let mut out = vec![];
        let mut head = vec![
            Span::styled(format!(" {}", s["branch"].as_str().unwrap_or("HEAD (detached)")), t.accent_style().add_modifier(Modifier::BOLD)),
            match s["upstream"].as_str() {
                Some(u) => Span::styled(format!(" → {u}"), t.muted_style()),
                None => Span::styled(" no upstream", t.muted_style()),
            },
        ];
        if let Some(a) = s["ahead"].as_u64().filter(|a| *a > 0) {
            head.push(Span::styled(format!("  ↑{a}"), Style::default().fg(t.ok)));
        }
        if let Some(b) = s["behind"].as_u64().filter(|b| *b > 0) {
            head.push(Span::styled(format!("  ↓{b}"), Style::default().fg(t.warn)));
        }
        if let Some(b) = &self.busy {
            head.push(Span::styled(format!("   {b}…"), t.muted_style()));
        }
        out.push(Line::from(head));
        if let Some(state) = s["state"].as_str() {
            let conflicts = s["files"].as_array().into_iter().flatten().filter(|f| f["conflicted"] == true).count();
            let what = if conflicts > 0 {
                format!("{conflicts} conflicted files — resolve and stage them, then N continue")
            } else {
                "ready to continue — N continue".to_string()
            };
            out.push(Line::styled(format!(" {state} in progress: {what} · X abort"), Style::default().fg(t.warn)));
        }
        if let Some(r) = &self.result {
            let style = if r.ok { t.muted_style() } else { Style::default().fg(t.danger) };
            out.push(Line::styled(format!(" {} {}", if r.ok { "✓" } else { "✗" }, r.name), style));
            if !r.auth.is_null() {
                out.push(Line::styled(format!("   {}", auth_message(&r.auth)), Style::default().fg(t.warn)));
            }
            for l in r.output.lines().filter(|l| !l.trim().is_empty()).take(3) {
                out.push(Line::styled(format!("   {l}"), style));
            }
        }
        out
    }

    fn detail_title(&self) -> String {
        match self.panel {
            Panel::Files => self
                .sel_file()
                .map(|(f, s)| format!("{} ({})", f["path"].as_str().unwrap_or_default(), if s { "staged" } else { "changes" })),
            Panel::Branches => self.sel_branch().map(|b| b["name"].as_str().unwrap_or_default().to_string()),
            Panel::Commits => self
                .sel_commit()
                .map(|c| format!("{} {}", c["short"].as_str().unwrap_or_default(), c["subject"].as_str().unwrap_or_default())),
            Panel::Stash => self.sel_stash().map(|s| s["ref"].as_str().unwrap_or_default().to_string()),
        }
        .unwrap_or_else(|| "main".into())
    }

    /// Text above the diff of a commit / stash / branch.
    fn detail_head(&self) -> Vec<Line<'static>> {
        let t = Theme::get();
        match self.panel {
            Panel::Branches => {
                let Some(b) = self.sel_branch() else { return vec![] };
                let mut out = vec![];
                if b["current"] == true {
                    let up = b["upstream"].as_str().map(|u| format!("→ {u}")).unwrap_or_else(|| "no upstream".into());
                    out.push(Line::raw(format!("current branch · {up}")));
                } else {
                    let cur = self.status["branch"].as_str().unwrap_or("HEAD");
                    out.push(Line::styled(
                        format!(
                            "Enter switch · M merge into {cur} · r rebase {cur} onto it{}",
                            if b["remote"] == true { "" } else { " · d delete" }
                        ),
                        t.muted_style(),
                    ));
                }
                out.push(Line::styled(b["lastCommit"].as_str().unwrap_or_default().to_string(), t.muted_style()));
                out
            }
            Panel::Commits => {
                let Some(c) = self.sel_commit() else { return vec![] };
                let merge = c["parents"].as_array().is_some_and(|p| p.len() > 1);
                let refs: Vec<String> = c["refs"].as_array().into_iter().flatten().filter_map(|r| r.as_str().map(String::from)).collect();
                let mut out = vec![
                    Line::styled(c["hash"].as_str().unwrap_or_default().to_string(), Style::default().fg(t.warn)),
                    Line::styled(c["subject"].as_str().unwrap_or_default().to_string(), t.bold()),
                    Line::styled(
                        format!(
                            "{} · {}{}",
                            c["author"].as_str().unwrap_or_default(),
                            c["date"].as_str().unwrap_or_default(),
                            if merge { " · merge commit" } else { "" }
                        ),
                        t.muted_style(),
                    ),
                ];
                if !refs.is_empty() {
                    out.push(Line::styled(refs.join("  "), Style::default().fg(t.ok)));
                }
                out.push(Line::styled("b branch here · y cherry-pick · v revert · R reset…", t.muted_style()));
                out.push(Line::raw(""));
                out
            }
            Panel::Stash => {
                let Some(s) = self.sel_stash() else { return vec![] };
                vec![
                    Line::styled(
                        format!("{} · {}", s["message"].as_str().unwrap_or_default(), s["date"].as_str().unwrap_or_default()),
                        t.muted_style(),
                    ),
                    Line::styled("Enter apply · A pop · d drop", t.muted_style()),
                    Line::raw(""),
                ]
            }
            Panel::Files => match self.sel_file() {
                Some((f, staged)) => {
                    let mut hint = format!("Enter {} · o open in Files", if staged { "unstage" } else { "stage" });
                    if f["conflicted"] != true {
                        hint.push_str(" · d discard");
                    }
                    vec![Line::styled(hint, t.muted_style()), Line::raw("")]
                }
                None => vec![],
            },
        }
    }

    fn panel_items(&self, p: Panel) -> Vec<ListItem<'static>> {
        let t = Theme::get();
        match p {
            Panel::Files => self
                .files()
                .into_iter()
                .map(|(f, staged)| {
                    let (x, y) = status_letters(&f, staged);
                    let path = f["path"].as_str().unwrap_or_default().to_string();
                    let (dir, name) =
                        path.rsplit_once('/').map(|(d, n)| (format!("{d}/"), n.to_string())).unwrap_or((String::new(), path.clone()));
                    ListItem::new(Line::from(vec![
                        Span::styled(x, Style::default().fg(if f["conflicted"] == true { t.danger } else { t.ok })),
                        Span::styled(y, Style::default().fg(t.danger)),
                        Span::raw(" "),
                        Span::styled(dir, t.muted_style()),
                        Span::styled(name, if staged { Style::default().fg(t.ok) } else { Style::default() }),
                    ]))
                })
                .collect(),
            Panel::Branches => self
                .branch_rows()
                .into_iter()
                .map(|b| {
                    let current = b["current"] == true;
                    let name = b["name"].as_str().unwrap_or_default().to_string();
                    let style = if current { t.accent_style().add_modifier(Modifier::BOLD) } else { Style::default() };
                    let mut spans = vec![Span::styled(if current { "* " } else { "  " }, t.accent_style())];
                    match name.split_once('/').filter(|_| b["remote"] == true) {
                        Some((remote, rest)) => {
                            spans.push(Span::styled(format!("{remote}/"), t.muted_style()));
                            spans.push(Span::styled(rest.to_string(), style));
                        }
                        None => spans.push(Span::styled(name, style)),
                    }
                    let when = b["lastCommit"].as_str().unwrap_or_default().trim_end_matches(" ago").to_string();
                    spans.push(Span::styled(format!("  {when}"), t.muted_style()));
                    ListItem::new(Line::from(spans))
                })
                .collect(),
            Panel::Commits => self
                .commits
                .iter()
                .map(|c| {
                    let merge = c["parents"].as_array().is_some_and(|p| p.len() > 1);
                    let head = c["refs"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .any(|r| r == "HEAD" || r.as_str().is_some_and(|s| s.starts_with("HEAD -> ")));
                    let mut spans = vec![
                        Span::styled(format!("{} ", c["short"].as_str().unwrap_or_default()), Style::default().fg(t.warn)),
                        Span::styled(
                            if merge {
                                "◆ "
                            } else if head {
                                "● "
                            } else {
                                "○ "
                            },
                            t.accent_style(),
                        ),
                    ];
                    let refs: Vec<&str> =
                        c["refs"].as_array().into_iter().flatten().filter_map(|r| r.as_str()).filter(|r| *r != "origin/HEAD").collect();
                    for r in refs.iter().take(2) {
                        let label = r.trim_start_matches("HEAD -> ").trim_start_matches("tag: ");
                        let style = if r.starts_with("tag: ") {
                            Style::default().fg(t.warn)
                        } else if r.starts_with("HEAD") {
                            t.accent_style().add_modifier(Modifier::BOLD)
                        } else if r.contains('/') {
                            t.muted_style()
                        } else {
                            Style::default().fg(t.ok)
                        };
                        spans.push(Span::styled(format!("{}{label} ", if r.starts_with("HEAD -> ") { "*" } else { "" }), style));
                    }
                    if refs.len() > 2 {
                        spans.push(Span::styled(format!("+{} ", refs.len() - 2), t.muted_style()));
                    }
                    spans.push(Span::raw(c["subject"].as_str().unwrap_or_default().to_string()));
                    spans.push(Span::styled(format!("  {}", c["author"].as_str().unwrap_or_default()), t.muted_style()));
                    ListItem::new(Line::from(spans))
                })
                .collect(),
            Panel::Stash => self
                .stashes
                .iter()
                .map(|s| {
                    ListItem::new(Line::from(vec![
                        Span::styled(format!("{} ", s["ref"].as_str().unwrap_or_default().trim_start_matches("stash@")), t.muted_style()),
                        Span::raw(s["message"].as_str().unwrap_or_default().to_string()),
                        Span::styled(format!("  {}", s["date"].as_str().unwrap_or_default()), t.muted_style()),
                    ]))
                })
                .collect(),
        }
    }

    fn panel_title(&self, p: Panel) -> String {
        match p {
            Panel::Files => {
                let n = self.status["files"].as_array().map(Vec::len).unwrap_or(0);
                if n == 0 { "files — clean".into() } else { format!("files ({n})") }
            }
            Panel::Branches => format!("branches ({})", self.branches.iter().filter(|b| b["remote"] != true).count()),
            Panel::Commits => format!(
                "commits — {}{}",
                if self.log_all { "all branches" } else { self.status["branch"].as_str().unwrap_or("HEAD") },
                if self.more { " · m more" } else { "" }
            ),
            Panel::Stash => format!("stash ({})", self.stashes.len()),
        }
    }

    fn space(&mut self, ctx: &mut Ctx) {
        match self.panel {
            Panel::Files => {
                if let Some((f, staged)) = self.sel_file() {
                    let (op, label) = if staged { ("unstage", "Unstage") } else { ("stage", "Stage") };
                    self.run(op, label, json!({ "paths": [f["path"]] }), ctx);
                }
            }
            Panel::Branches => {
                if let Some(b) = self.sel_branch().filter(|b| b["current"] != true) {
                    let name = b["name"].as_str().unwrap_or_default().to_string();
                    self.run("checkout", &format!("Switch to {name}"), json!({ "branch": name, "create": false }), ctx);
                }
            }
            Panel::Stash => {
                if let Some(s) = self.sel_stash() {
                    let r = s["ref"].as_str().unwrap_or_default().to_string();
                    self.run("stash", &format!("Stash apply {r}"), json!({ "action": "apply", "rev": r }), ctx);
                }
            }
            Panel::Commits => {}
        }
    }
}

impl Screen for GitTab {
    fn title(&self) -> String {
        "Git".into()
    }

    fn on_show(&mut self, ctx: &mut Ctx) {
        self.load_status(ctx);
        self.load_rest(ctx);
        if !self.watching {
            ctx.stream("watch", format!("/watch?projects={}", enc(&self.id)));
            self.watching = true;
        }
    }

    fn actions(&self) -> Vec<Action> {
        let mut a = vec![Action::new("f", "Fetch"), Action::new("p", "Pull"), Action::new("P", "Push")];
        if self.status["state"].is_string() {
            a.push(Action::new("N", "Continue the merge / rebase"));
            a.push(Action::new("X", "Abort the merge / rebase"));
        }
        if self.result.as_ref().is_some_and(|r| !r.auth.is_null()) {
            a.push(Action::new("L", "Sign in to the git host and retry"));
        }
        match self.panel {
            Panel::Files => a.extend([
                Action::new("-", "Stage / unstage the file (also Enter)"),
                Action::new("a", "Stage all / unstage all"),
                Action::new("d", "Discard the file's changes"),
                Action::new("c", "Commit"),
                Action::new("s", "Stash all changes"),
                Action::new("o", "Open the file in Files"),
            ]),
            Panel::Branches => a.extend([
                Action::new("-", "Switch to the branch (also Enter)"),
                Action::new("n", "New branch"),
                Action::new("M", "Merge into the current branch"),
                Action::new("r", "Rebase the current branch onto it"),
                Action::new("d", "Delete the branch"),
            ]),
            Panel::Commits => a.extend([
                Action::new("b", "New branch at the commit"),
                Action::new("y", "Cherry-pick"),
                Action::new("v", "Revert"),
                Action::new("R", "Reset the branch to the commit…"),
                Action::new("A", if self.log_all { "Log: current branch only" } else { "Log: all branches" }),
                Action::new("m", "Load more commits"),
            ]),
            Panel::Stash => a.extend([Action::new("-", "Apply (also Enter)"), Action::new("A", "Pop"), Action::new("d", "Drop")]),
        }
        a
    }

    fn on_action(&mut self, key: &str, ctx: &mut Ctx) {
        let branch = self.status["branch"].as_str().map(String::from);
        match key {
            "f" => self.run("fetch", "Fetch", Value::Null, ctx),
            "p" => self.run("pull", "Pull", Value::Null, ctx),
            "P" => self.run("push", "Push", Value::Null, ctx),
            "N" => self.run("continue", "Continue", Value::Null, ctx),
            "X" => {
                let state = self.status["state"].as_str().unwrap_or("operation").to_string();
                ctx.confirm("abort", "Abort", format!("Abort the {state}? Changes made during it are lost."), true);
            }
            "L" => {
                if let Some(host) = self.result.as_ref().and_then(|r| r.auth["host"].as_str()) {
                    self.login = Some((host.to_string(), String::new(), String::new()));
                    ctx.pick(
                        "login-kind",
                        format!("Sign in to {host}"),
                        vec![("token".into(), "Access token (recommended)".into()), ("password".into(), "Login and password".into())],
                    );
                }
            }
            "-" => self.space(ctx),
            _ => {}
        }
        match (self.panel, key) {
            (Panel::Files, "a") => {
                let files = self.files();
                let unstaged: Vec<Value> = files.iter().filter(|(_, s)| !s).map(|(f, _)| f["path"].clone()).collect();
                if !unstaged.is_empty() {
                    self.run("stage", "Stage all", json!({ "paths": unstaged }), ctx);
                } else {
                    let staged: Vec<Value> = files.iter().map(|(f, _)| f["path"].clone()).collect();
                    if !staged.is_empty() {
                        self.run("unstage", "Unstage all", json!({ "paths": staged }), ctx);
                    }
                }
            }
            (Panel::Files, "d") => {
                if let Some((f, _)) = self.sel_file().filter(|(f, _)| f["conflicted"] != true) {
                    let p = f["path"].as_str().unwrap_or_default().to_string();
                    ctx.confirm(&format!("discard:{p}"), "Discard", format!("Discard the changes of {p}? This can't be undone."), true);
                }
            }
            (Panel::Files, "c") => {
                let files = self.files();
                let staged = files.iter().filter(|(_, s)| *s).count();
                let unstaged = files.len() - staged;
                if files.is_empty() {
                    ctx.info("nothing to commit");
                    return;
                }
                let what = if staged == 0 { format!("all {unstaged} changes") } else { format!("{staged} staged") };
                ctx.prompt_multiline("commit", format!("Commit {what} — Enter commits · Alt+Enter newline"), "");
            }
            (Panel::Files, "s") => {
                if !self.files().is_empty() {
                    ctx.prompt("stash-push", "Stash all changes — message (optional)", "");
                }
            }
            (Panel::Files, "o") => {
                if let Some((f, _)) = self.sel_file() {
                    self.opener.request(f["path"].as_str().unwrap_or_default(), None);
                }
            }
            (Panel::Branches, "n") => ctx.prompt("new-branch", "New branch (from the current one)", ""),
            (Panel::Branches, "M") => {
                if let (Some(b), Some(cur)) = (self.sel_branch().filter(|b| b["current"] != true), &branch) {
                    let name = b["name"].as_str().unwrap_or_default();
                    ctx.confirm(&format!("merge:{name}"), "Merge", format!("Merge {name} into {cur}?"), false);
                }
            }
            (Panel::Branches, "r") => {
                if let (Some(b), Some(cur)) = (self.sel_branch().filter(|b| b["current"] != true), &branch) {
                    let name = b["name"].as_str().unwrap_or_default();
                    ctx.confirm(&format!("rebase:{name}"), "Rebase", format!("Rebase {cur} onto {name}?"), false);
                }
            }
            (Panel::Branches, "d") => {
                if let Some(b) = self.sel_branch().filter(|b| b["current"] != true && b["remote"] != true) {
                    let name = b["name"].as_str().unwrap_or_default();
                    ctx.confirm(&format!("delete-branch:{name}"), "Delete branch", format!("Delete the branch {name}?"), true);
                }
            }
            (Panel::Commits, "b") => {
                if let Some(c) = self.sel_commit() {
                    ctx.prompt(
                        &format!("branch-at:{}", c["hash"].as_str().unwrap_or_default()),
                        format!("New branch at {}", c["short"].as_str().unwrap_or_default()),
                        "",
                    );
                }
            }
            (Panel::Commits, "y") => {
                if let Some(c) = self.sel_commit() {
                    let short = c["short"].as_str().unwrap_or_default();
                    self.run("cherry-pick", &format!("Cherry-pick {short}"), json!({ "rev": c["hash"] }), ctx);
                }
            }
            (Panel::Commits, "v") => {
                if let Some(c) = self.sel_commit() {
                    let short = c["short"].as_str().unwrap_or_default();
                    ctx.confirm(
                        &format!("revert:{}", c["hash"].as_str().unwrap_or_default()),
                        "Revert",
                        format!("Make a commit that reverts {short}?"),
                        false,
                    );
                }
            }
            (Panel::Commits, "R") => {
                if let Some(c) = self.sel_commit() {
                    let short = c["short"].as_str().unwrap_or_default();
                    let target = branch.clone().unwrap_or_else(|| "HEAD".into());
                    ctx.pick(
                        &format!("reset:{}", c["hash"].as_str().unwrap_or_default()),
                        format!("Reset {target} to {short}"),
                        vec![
                            ("soft".into(), "soft — keep the changes staged".into()),
                            ("mixed".into(), "mixed — keep the changes, unstaged".into()),
                            ("hard".into(), "hard — throw the changes away".into()),
                        ],
                    );
                }
            }
            (Panel::Commits, "A") => {
                self.log_all = !self.log_all;
                self.load_rest(ctx);
            }
            (Panel::Commits, "m") => {
                if self.more {
                    self.log_limit += 300;
                    self.load_rest(ctx);
                }
            }
            (Panel::Stash, "A") => {
                if let Some(s) = self.sel_stash() {
                    let r = s["ref"].as_str().unwrap_or_default().to_string();
                    self.run("stash", &format!("Stash pop {r}"), json!({ "action": "pop", "rev": r }), ctx);
                }
            }
            (Panel::Stash, "d") => {
                if let Some(s) = self.sel_stash() {
                    let r = s["ref"].as_str().unwrap_or_default();
                    ctx.confirm(&format!("drop:{r}"), "Drop stash", format!("Drop {r}? This can't be undone."), true);
                }
            }
            _ => {}
        }
    }

    fn on_key(&mut self, k: KeyEvent, ctx: &mut Ctx) -> Handled {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let page = self.detail_height.max(2) - 1;
        match k.code {
            KeyCode::Down | KeyCode::Char('j') => self.move_by(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_by(-1),
            // gg / G pick the first / last row; PageUp / PageDown (Ctrl+u / Ctrl+d) scroll the diff
            KeyCode::Home => self.move_by(-(isize::MAX / 2)),
            KeyCode::End => self.move_by(isize::MAX / 2),
            KeyCode::Tab | KeyCode::Char(']') => self.cycle(1),
            KeyCode::BackTab | KeyCode::Char('[') => self.cycle(-1),
            KeyCode::Enter | KeyCode::Char('-') => self.space(ctx),
            KeyCode::Char('J') => self.detail_scroll += 1,
            KeyCode::Char('K') => self.detail_scroll = self.detail_scroll.saturating_sub(1),
            KeyCode::PageDown => self.detail_scroll += page,
            KeyCode::Char('d') if ctrl => self.detail_scroll += page,
            KeyCode::PageUp => self.detail_scroll = self.detail_scroll.saturating_sub(page),
            KeyCode::Char('u') if ctrl => self.detail_scroll = self.detail_scroll.saturating_sub(page),
            _ => {
                return Handled::No;
            }
        }
        // a move may need the selection's diff
        self.load_detail(false, ctx);
        Handled::Yes
    }

    fn on_stream(&mut self, key: &str, msg: Value, ctx: &mut Ctx) {
        if key != "watch" {
            return;
        }
        if msg["type"] == "__ended" {
            self.watching = false;
            return;
        }
        self.load_status(ctx);
        if msg["git"] == true || msg["overflow"] == true {
            self.load_rest(ctx);
        }
    }

    fn on_task(&mut self, key: &str, result: anyhow::Result<Value>, ctx: &mut Ctx) {
        if let Some(n) = key.strip_prefix("detail:") {
            if n.parse::<u64>().ok() != Some(self.detail_seq) {
                return;
            }
            match result {
                Ok(v) => self.detail = code::diff(v["diff"].as_str().unwrap_or_default()),
                Err(e) => self.detail = vec![Line::styled(e.to_string(), Style::default().fg(Theme::get().danger))],
            }
            return;
        }
        match key {
            "status" => {
                if let Some(v) = ctx.ok(result) {
                    self.status = v;
                    let force = self.panel == Panel::Files;
                    self.load_detail(force, ctx);
                }
            }
            "branches" => {
                if let Some(v) = ctx.ok(result) {
                    self.branches = v.as_array().cloned().unwrap_or_default();
                    self.load_detail(false, ctx);
                }
            }
            "log" => {
                if let Some(v) = ctx.ok(result) {
                    self.commits = v["commits"].as_array().cloned().unwrap_or_default();
                    self.more = v["more"] == true;
                    self.load_detail(false, ctx);
                }
            }
            "stashes" => {
                if let Some(v) = ctx.ok(result) {
                    self.stashes = v.as_array().cloned().unwrap_or_default();
                    self.load_detail(false, ctx);
                }
            }
            "op" => {
                self.busy = None;
                let (op, label, body) = self.last_op.clone().unwrap_or_default();
                let r = match result {
                    Ok(v) => OpResult {
                        name: label.clone(),
                        ok: v["ok"] == true,
                        output: v["output"].as_str().unwrap_or_default().to_string(),
                        auth: v["auth"].clone(),
                    },
                    Err(e) => OpResult { name: label.clone(), ok: false, output: e.to_string(), auth: Value::Null },
                };
                if !r.ok {
                    ctx.error(format!("{label} failed"));
                } else {
                    ctx.info(format!("{label} — done"));
                }
                if op == "delete-branch" && !r.ok && r.output.to_lowercase().contains("not fully merged") && body["force"] != true {
                    let name = body["branch"].as_str().unwrap_or_default();
                    ctx.confirm(
                        &format!("force-delete:{name}"),
                        "Not merged",
                        format!("{name} is not fully merged. Delete it anyway?"),
                        true,
                    );
                }
                self.result = Some(r);
                self.load_status(ctx);
                self.load_rest(ctx);
            }
            _ => self.on_answer(key, result, ctx),
        }
    }

    fn draw(&mut self, f: &mut Frame, area: Rect, _ctx: &Ctx) {
        let t = Theme::get();
        if self.status.is_null() {
            f.render_widget(Paragraph::new("loading…").block(block("git", false)), area);
            return;
        }
        if self.status["isRepo"] == false {
            f.render_widget(Paragraph::new("This folder is not a git repository.").block(block("git", false)), area);
            return;
        }
        let header = self.header_lines();
        let [head, body] = Layout::vertical([Constraint::Length(header.len() as u16), Constraint::Min(5)]).areas(area);
        f.render_widget(Paragraph::new(header), head);

        let [left, right] = Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)]).areas(body);
        let constraints: Vec<Constraint> = PANELS
            .iter()
            .map(|p| if *p == self.panel { Constraint::Min(5) } else { Constraint::Length((self.len(*p) as u16 + 2).clamp(3, 6)) })
            .collect();
        let areas = Layout::vertical(constraints).split(left);
        for (i, p) in PANELS.iter().enumerate() {
            let focused = *p == self.panel;
            let items = self.panel_items(*p);
            let mut state = self.lists[i];
            if self.len(*p) > 0 {
                state.select(Some(self.sel(*p).unwrap_or(0)));
            }
            f.render_stateful_widget(
                List::new(items).block(block(self.panel_title(*p), focused)).highlight_style(if focused { t.selected() } else { t.bold() }),
                areas[i],
                &mut state,
            );
            self.lists[i] = state;
        }

        let mut lines = self.detail_head();
        if self.detail.is_empty() && !self.detail_id.is_empty() && self.panel != Panel::Branches {
            lines.push(Line::styled("loading…", t.muted_style()));
        }
        lines.extend(self.detail.iter().cloned());
        if self.detail_id.is_empty() && lines.is_empty() {
            lines.push(Line::styled("Tab / [ ] switch panels · j/k select · J/K or PgUp/PgDn scroll this view", t.muted_style()));
        }
        let height = right.height.saturating_sub(2) as usize;
        self.detail_height = height.max(1);
        let max = lines.len().saturating_sub(height);
        self.detail_scroll = self.detail_scroll.min(max);
        f.render_widget(
            Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .scroll((self.detail_scroll as u16, 0))
                .block(block(self.detail_title(), false)),
            right,
        );
    }
}

impl GitTab {
    /// Answers of the modals this tab opened.
    fn on_answer(&mut self, key: &str, result: anyhow::Result<Value>, ctx: &mut Ctx) {
        let Ok(v) = result else { return };
        let yes = v == Value::Bool(true);
        let text = v.as_str().map(|s| s.to_string());
        let (what, arg) = key.split_once(':').unwrap_or((key, ""));
        match what {
            "abort" if yes => self.run("abort", "Abort", Value::Null, ctx),
            "discard" if yes => self.run("discard", &format!("Discard {arg}"), json!({ "paths": [arg] }), ctx),
            "commit" => {
                if let Some(m) = text.filter(|m| !m.trim().is_empty()) {
                    let all = !self.files().iter().any(|(_, s)| *s);
                    self.run("commit", "Commit", json!({ "message": m, "all": all }), ctx);
                }
            }
            "stash-push" => {
                if let Some(m) = text {
                    self.run("stash-push", "Stash", json!({ "message": m.trim(), "untracked": true }), ctx);
                }
            }
            "new-branch" => {
                if let Some(n) = text.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()) {
                    self.run("checkout", &format!("Switch to {n}"), json!({ "branch": n, "create": true }), ctx);
                }
            }
            "merge" if yes => self.run("merge", &format!("Merge {arg}"), json!({ "branch": arg }), ctx),
            "rebase" if yes => {
                let cur = self.status["branch"].as_str().unwrap_or("HEAD").to_string();
                self.run("rebase", &format!("Rebase {cur} → {arg}"), json!({ "branch": arg }), ctx)
            }
            "delete-branch" if yes => self.run("delete-branch", "Delete branch", json!({ "branch": arg }), ctx),
            "force-delete" if yes => self.run("delete-branch", "Delete branch", json!({ "branch": arg, "force": true }), ctx),
            "branch-at" => {
                if let Some(n) = text.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()) {
                    self.run("branch-at", &format!("Switch to {n}"), json!({ "branch": n, "rev": arg }), ctx);
                }
            }
            "revert" if yes => self.run("revert", &format!("Revert {}", &arg[..arg.len().min(7)]), json!({ "rev": arg }), ctx),
            "reset" => {
                if let Some(mode) = text {
                    if mode == "hard" {
                        ctx.confirm(
                            &format!("reset-hard:{arg}"),
                            "reset --hard",
                            "Reset --hard throws away every uncommitted change. Continue?",
                            true,
                        );
                    } else {
                        self.run(
                            "reset",
                            &format!("Reset --{mode} {}", &arg[..arg.len().min(7)]),
                            json!({ "rev": arg, "mode": mode }),
                            ctx,
                        );
                    }
                }
            }
            "reset-hard" if yes => {
                self.run("reset", &format!("Reset --hard {}", &arg[..arg.len().min(7)]), json!({ "rev": arg, "mode": "hard" }), ctx)
            }
            "drop" if yes => self.run("stash", &format!("Stash drop {arg}"), json!({ "action": "drop", "rev": arg }), ctx),
            "login-kind" => match (text, &mut self.login) {
                (Some(kind), Some(l)) => {
                    l.1 = kind;
                    let host = l.0.clone();
                    ctx.prompt("login-user", format!("Username on {host}"), "");
                }
                _ => self.login = None,
            },
            "login-user" => match (text.filter(|u| !u.trim().is_empty()), &mut self.login) {
                (Some(user), Some(l)) => {
                    l.2 = user.trim().to_string();
                    let label = if l.1 == "token" { "Access token" } else { "Password" };
                    ctx.prompt_secret("login-secret", format!("{label} for {}", l.0));
                }
                _ => self.login = None,
            },
            "login-secret" => {
                if let (Some(secret), Some((host, kind, user))) = (text.filter(|s| !s.is_empty()), self.login.take()) {
                    ctx.send(
                        "login-saved",
                        Method::PUT,
                        format!("/git-credentials/{}", enc(&host)),
                        json!({ "username": user, "kind": kind, "secret": secret }),
                    );
                } else {
                    self.login = None;
                }
            }
            "login-saved" => {
                // a credential list came back: retry the operation that was refused
                ctx.info("login saved");
                if let Some((op, label, body)) = self.last_op.clone() {
                    self.run(&op, &label, body, ctx);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, index: &str, worktree: &str, untracked: bool, conflicted: bool) -> Value {
        json!({ "path": path, "index": index, "worktree": worktree, "untracked": untracked, "conflicted": conflicted })
    }

    #[test]
    fn splits_status_into_sections() {
        let files = vec![
            file("both.rs", "M", "M", false, false),
            file("staged.rs", "A", " ", false, false),
            file("new.txt", "?", "?", true, false),
            file("clash.rs", "U", "U", false, true),
            file("edited.rs", " ", "M", false, false),
        ];
        let rows: Vec<(String, bool)> = file_rows(&files).into_iter().map(|(f, s)| (f["path"].as_str().unwrap().to_string(), s)).collect();
        assert_eq!(
            rows,
            vec![
                ("both.rs".into(), false),
                ("new.txt".into(), false),
                ("clash.rs".into(), false),
                ("edited.rs".into(), false),
                ("both.rs".into(), true),
                ("staged.rs".into(), true),
            ]
        );
    }

    #[test]
    fn letters_and_auth() {
        let f = file("a", "M", "D", false, false);
        assert_eq!(status_letters(&f, true), ("M".into(), " ".into()));
        assert_eq!(status_letters(&f, false), (" ".into(), "D".into()));
        assert_eq!(status_letters(&file("b", "?", "?", true, false), false), ("?".into(), "?".into()));
        assert!(auth_message(&json!({ "host": "github.com", "protocol": "https", "reason": "missing" })).contains("L to sign in"));
        assert!(auth_message(&json!({ "host": "h", "protocol": "ssh", "reason": "hostkey" })).contains("known ssh host"));
    }
}
