//! The Files tab (the web app's explorer and file viewer): the project tree with folders
//! expanded on demand and git-ignored entries dimmed, a viewer with syntax highlighting
//! (or the file's git diff), an editor (edtui) saving with a conflict check, new / rename /
//! delete, find a file by name, and live refresh from the `/watch` stream.

use std::collections::{HashMap, HashSet};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use edtui::clipboard::InternalClipboard;
use edtui::{EditorEventHandler, EditorMode, EditorState, EditorTheme, EditorView, Index2, LineNumbers, Lines, SyntaxHighlighter};
use hubness_client::Method;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph};
use serde_json::{Value, json};

use super::{Opener, enc};
use crate::core::{Action, Ctx, Handled, Screen};
use crate::ui::{Theme, block, code};

/// One entry of `GET /projects/:id/tree`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    pub name: String,
    pub dir: bool,
    pub ignored: bool,
}

impl Entry {
    fn from_value(v: &Value) -> Self {
        Self {
            path: v["path"].as_str().unwrap_or_default().to_string(),
            name: v["name"].as_str().unwrap_or_default().to_string(),
            dir: v["dir"] == true,
            ignored: v["ignored"] == true,
        }
    }
}

/// A visible row of the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub entry: Entry,
    pub depth: usize,
    pub open: bool,
}

/// The visible rows: children of every expanded (and loaded) folder, depth first.
pub fn flatten(children: &HashMap<String, Vec<Entry>>, expanded: &HashSet<String>) -> Vec<Row> {
    fn walk(dir: &str, depth: usize, children: &HashMap<String, Vec<Entry>>, expanded: &HashSet<String>, out: &mut Vec<Row>) {
        for e in children.get(dir).into_iter().flatten() {
            let open = e.dir && expanded.contains(&e.path);
            out.push(Row { entry: e.clone(), depth, open });
            if open {
                walk(&e.path, depth + 1, children, expanded, out);
            }
        }
    }
    let mut out = vec![];
    walk("", 0, children, expanded, &mut out);
    out
}

/// The folder a path lives in ("" for the root).
pub fn parent_of(path: &str) -> &str {
    path.rsplit_once('/').map(|(p, _)| p).unwrap_or("")
}

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() { name.to_string() } else { format!("{dir}/{name}") }
}

/// Loaded folders to re-read after a batch of changed paths (every one on overflow).
pub fn dirs_to_reload<'a>(loaded: impl Iterator<Item = &'a String>, paths: &[String], overflow: bool) -> Vec<String> {
    let mut out: Vec<String> =
        loaded.filter(|d| overflow || paths.iter().any(|p| parent_of(p) == d.as_str() || p == *d)).cloned().collect();
    out.sort();
    out
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum Focus {
    Tree,
    Viewer,
    Editor,
}

struct Editor {
    state: EditorState,
    handler: EditorEventHandler,
    original: String,
    ext: String,
}

impl Editor {
    fn new(content: &str, path: &str) -> Self {
        let mut state = EditorState::new(Lines::from(content));
        state.set_clipboard(InternalClipboard::default());
        // like Vim: open in NORMAL (i / a / o to insert, Esc back, :w :q :wq :q!)
        state.mode = EditorMode::Normal;
        let ext = path.rsplit('.').next().unwrap_or_default().to_string();
        Self { state, handler: EditorEventHandler::vim_mode(), original: content.to_string(), ext }
    }

    fn text(&self) -> String {
        self.state.lines.flatten(&Some('\n')).into_iter().collect()
    }

    fn dirty(&self) -> bool {
        self.text() != self.original
    }
}

/// The file shown on the right.
struct Open {
    path: String,
    data: Value,
    lines: Vec<Line<'static>>,
    scroll: usize,
    mark: Option<usize>,
    diff: Option<Vec<Line<'static>>>,
    editor: Option<Editor>,
    /// Changed on disk while there are unsaved edits.
    changed: bool,
    gone: bool,
}

impl Open {
    fn editable(&self) -> bool {
        self.data["binary"] != true && self.data["truncated"] != true && self.data["hash"].is_string()
    }
}

pub struct FilesTab {
    id: String,
    opener: Opener,
    children: HashMap<String, Vec<Entry>>,
    expanded: HashSet<String>,
    rows: Vec<Row>,
    list: ListState,
    file: Option<Open>,
    focus: Focus,
    watching: bool,
    /// A path to select once its folders are loaded.
    reveal: Option<String>,
    view_height: usize,
}

impl FilesTab {
    pub fn new(id: &str, opener: Opener) -> Self {
        Self {
            id: id.to_string(),
            opener,
            children: HashMap::new(),
            expanded: HashSet::new(),
            rows: vec![],
            list: ListState::default(),
            file: None,
            focus: Focus::Tree,
            watching: false,
            reveal: None,
            view_height: 20,
        }
    }

    fn api(&self, rest: &str) -> String {
        format!("/projects/{}{rest}", self.id)
    }

    fn load_dir(&self, dir: &str, ctx: &mut Ctx) {
        ctx.get(format!("tree:{dir}"), self.api(&format!("/tree?path={}", enc(dir))));
    }

    fn load_file(&self, path: &str, ctx: &mut Ctx) {
        ctx.get(format!("file:{path}"), self.api(&format!("/file?path={}", enc(path))));
    }

    fn selected(&self) -> Option<&Row> {
        self.list.selected().and_then(|i| self.rows.get(i))
    }

    fn rebuild(&mut self) {
        let keep = self.selected().map(|r| r.entry.path.clone());
        self.rows = flatten(&self.children, &self.expanded);
        let target = self.reveal.clone().or(keep);
        let idx = target.and_then(|p| self.rows.iter().position(|r| r.entry.path == p));
        if let Some(i) = idx {
            self.list.select(Some(i));
            if self.reveal.as_ref().is_some_and(|r| self.rows[i].entry.path == *r) {
                self.reveal = None;
            }
        } else if self.rows.is_empty() {
            self.list.select(None);
        } else {
            let i = self.list.selected().unwrap_or(0).min(self.rows.len() - 1);
            self.list.select(Some(i));
        }
    }

    /// Expand every folder above `path` (loading them) and select it when it appears.
    fn reveal(&mut self, path: &str, ctx: &mut Ctx) {
        let mut dir = parent_of(path).to_string();
        while !dir.is_empty() {
            if self.expanded.insert(dir.clone()) && !self.children.contains_key(&dir) {
                self.load_dir(&dir, ctx);
            }
            dir = parent_of(&dir).to_string();
        }
        self.reveal = Some(path.to_string());
        self.rebuild();
    }

    fn open(&mut self, path: &str, line: Option<usize>, ctx: &mut Ctx) {
        if self.file.as_ref().is_some_and(|f| f.editor.as_ref().is_some_and(Editor::dirty) && f.path != path) {
            ctx.error("Save (Ctrl+S) or discard (Esc) the edits first");
            return;
        }
        if self.file.as_ref().is_none_or(|f| f.path != path) {
            self.file = Some(Open {
                path: path.to_string(),
                data: Value::Null,
                lines: vec![],
                scroll: 0,
                mark: None,
                diff: None,
                editor: None,
                changed: false,
                gone: false,
            });
            self.load_file(path, ctx);
        }
        if let Some(f) = &mut self.file {
            f.mark = line;
            if let Some(l) = line {
                f.scroll = l.saturating_sub(self.view_height / 3 + 1);
            }
        }
    }

    fn toggle(&mut self, ctx: &mut Ctx) {
        let Some(row) = self.selected().cloned() else { return };
        if row.entry.dir {
            if !self.expanded.remove(&row.entry.path) {
                self.expanded.insert(row.entry.path.clone());
                self.load_dir(&row.entry.path, ctx);
            }
            self.rebuild();
        } else {
            self.open(&row.entry.path, None, ctx);
            self.focus = Focus::Viewer;
        }
    }

    fn expand_or_preview(&mut self, ctx: &mut Ctx) {
        let Some(row) = self.selected().cloned() else { return };
        if row.entry.dir {
            if self.expanded.insert(row.entry.path.clone()) {
                self.load_dir(&row.entry.path, ctx);
                self.rebuild();
            } else {
                self.move_by(1);
            }
        } else {
            self.open(&row.entry.path, None, ctx);
        }
    }

    fn collapse_or_parent(&mut self) {
        let Some(row) = self.selected().cloned() else { return };
        if row.open {
            self.expanded.remove(&row.entry.path);
        } else {
            let parent = parent_of(&row.entry.path).to_string();
            if parent.is_empty() {
                return;
            }
            self.expanded.remove(&parent);
            self.reveal = Some(parent);
        }
        self.rebuild();
    }

    fn move_by(&mut self, d: isize) {
        if self.rows.is_empty() {
            return;
        }
        let i = self.list.selected().unwrap_or(0) as isize + d;
        self.list.select(Some(i.clamp(0, self.rows.len() as isize - 1) as usize));
    }

    /// The folder new entries go into: the selected folder, or the selected file's folder.
    fn target_dir(&self) -> String {
        match self.selected() {
            Some(r) if r.entry.dir => r.entry.path.clone(),
            Some(r) => parent_of(&r.entry.path).to_string(),
            None => String::new(),
        }
    }

    fn start_edit(&mut self, ctx: &mut Ctx) {
        let Some(f) = &mut self.file else {
            ctx.info("open a file first");
            return;
        };
        if !f.editable() {
            ctx.error("This file can't be edited here (binary, too large or still loading)");
            return;
        }
        if f.editor.is_none() {
            let mut ed = Editor::new(f.data["content"].as_str().unwrap_or_default(), &f.path);
            let row = f.mark.map(|l| l.saturating_sub(1)).unwrap_or(f.scroll);
            ed.state.cursor = Index2::new(row.min(ed.state.lines.len().saturating_sub(1)), 0);
            f.editor = Some(ed);
        }
        f.diff = None;
        self.focus = Focus::Editor;
    }

    fn save(&mut self, force: bool, ctx: &mut Ctx) {
        let Some(f) = &self.file else { return };
        let Some(ed) = &f.editor else { return };
        let mut body = json!({ "path": f.path, "content": ed.text() });
        if !force && let Some(h) = f.data["hash"].as_str() {
            body["baseHash"] = json!(h);
        }
        ctx.send("saved", Method::PUT, self.api("/file"), body);
        ctx.info("saving…");
    }

    fn set_data(&mut self, v: Value) {
        let Some(f) = &mut self.file else { return };
        let content = v["content"].as_str().unwrap_or_default();
        f.lines = if v["binary"] == true { vec![] } else { code::highlight(content, &f.path) };
        f.data = v;
        f.gone = false;
        f.changed = false;
    }

    fn toggle_diff(&mut self, ctx: &mut Ctx) {
        let api = self.api("/git/diff?path=");
        let Some(f) = &mut self.file else { return };
        if f.diff.take().is_none() {
            ctx.get(format!("diff:{}", f.path), format!("{api}{}", enc(&f.path)));
        }
    }

    fn on_watch(&mut self, msg: &Value, ctx: &mut Ctx) {
        if msg["type"] == "__ended" {
            self.watching = false;
            return;
        }
        let paths: Vec<String> = msg["paths"].as_array().into_iter().flatten().filter_map(|p| p.as_str()).map(String::from).collect();
        let overflow = msg["overflow"] == true;
        for d in dirs_to_reload(self.children.keys(), &paths, overflow) {
            self.load_dir(&d, ctx);
        }
        let Some(f) = &mut self.file else { return };
        if overflow || paths.contains(&f.path) {
            let p = f.path.clone();
            if f.editor.as_ref().is_some_and(Editor::dirty) {
                // unsaved edits stay; only flag the file when its content really changed
                ctx.get(format!("probe:{p}"), self.api(&format!("/file?path={}", enc(&p))));
            } else {
                self.load_file(&p, ctx);
            }
        }
    }

    /// `:w`, `:q`, `:wq` / `:x`, `:q!` typed in the editor's command line.
    fn editor_command(&mut self, cmd: &str, ctx: &mut Ctx) {
        let dirty = self.file.as_ref().and_then(|f| f.editor.as_ref()).is_some_and(Editor::dirty);
        let close = |me: &mut Self| {
            if let Some(f) = &mut me.file {
                f.editor = None;
            }
            me.focus = Focus::Viewer;
        };
        match cmd.trim() {
            "w" => self.save(false, ctx),
            "wq" | "x" => {
                if dirty {
                    self.save(false, ctx);
                }
                close(self);
            }
            "q" if dirty => ctx.error("unsaved changes — :w to save, :q! to discard"),
            "q" | "q!" => close(self),
            "" => {}
            other => ctx.error(format!("not an editor command: :{other}")),
        }
    }

    fn editor_key(&mut self, k: KeyEvent, ctx: &mut Ctx) -> Handled {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let normal = self.file.as_ref().and_then(|f| f.editor.as_ref()).is_some_and(|e| e.state.mode == EditorMode::Normal);
        match k.code {
            KeyCode::Char('s') if ctrl => self.save(false, ctx),
            // Vim's command line inside the editor: :w :q :wq :x :q!
            KeyCode::Char(':') if normal => ctx.prompt("editor-cmd", ":", ""),
            KeyCode::Esc if normal => {
                let dirty = self.file.as_ref().and_then(|f| f.editor.as_ref()).is_some_and(Editor::dirty);
                if dirty {
                    ctx.confirm("discard", "Unsaved changes", "Discard the unsaved changes of this file?", true);
                } else {
                    if let Some(f) = &mut self.file {
                        f.editor = None;
                    }
                    self.focus = Focus::Viewer;
                }
            }
            _ => {
                if let Some(ed) = self.file.as_mut().and_then(|f| f.editor.as_mut()) {
                    ed.handler.on_key_event(k, &mut ed.state);
                }
            }
        }
        Handled::Yes
    }

    fn draw_tree(&mut self, f: &mut Frame, area: Rect) {
        let t = Theme::get();
        let items: Vec<ListItem> = self
            .rows
            .iter()
            .map(|r| {
                let icon = if r.entry.dir { if r.open { "▾ " } else { "▸ " } } else { "  " };
                let style = if r.entry.ignored {
                    t.muted_style()
                } else if r.entry.dir {
                    t.accent_style()
                } else {
                    Style::default()
                };
                let open = self.file.as_ref().is_some_and(|f| f.path == r.entry.path);
                let name = if open {
                    Span::styled(r.entry.name.clone(), style.add_modifier(Modifier::BOLD))
                } else {
                    Span::styled(r.entry.name.clone(), style)
                };
                ListItem::new(Line::from(vec![Span::raw("  ".repeat(r.depth)), Span::styled(icon, t.muted_style()), name]))
            })
            .collect();
        let focused = self.focus == Focus::Tree;
        let title = if self.rows.is_empty() && self.children.is_empty() { "files — loading…".to_string() } else { "files".to_string() };
        let mut state = self.list;
        f.render_stateful_widget(
            List::new(items).block(block(title, focused)).highlight_style(if focused { t.selected() } else { t.bold() }),
            area,
            &mut state,
        );
        *self.list.offset_mut() = state.offset();
    }

    fn draw_file(&mut self, f: &mut Frame, area: Rect) {
        let t = Theme::get();
        let focus = self.focus;
        let Some(file) = &mut self.file else {
            let hint = vec![
                Line::raw(""),
                Line::styled("  Enter opens a file · e edits it · a / A new file / folder · r rename · x delete", t.muted_style()),
                Line::styled("  f finds a file by name · Tab switches between the tree and the file", t.muted_style()),
            ];
            f.render_widget(Paragraph::new(hint).block(block("viewer", false)), area);
            return;
        };
        let mut banners: Vec<Line> = vec![];
        if file.gone {
            banners.push(Line::styled(" The file is gone from disk.", Style::default().fg(t.warn)));
        }
        if file.changed {
            banners.push(Line::styled(
                " The file changed on disk — Ctrl+S offers to load it or overwrite it with your edits.",
                Style::default().fg(t.warn),
            ));
        }
        let [banner, body] = Layout::vertical([Constraint::Length(banners.len() as u16), Constraint::Min(1)]).areas(area);
        f.render_widget(Paragraph::new(banners), banner);

        if let Some(ed) = &mut file.editor {
            let dirty = ed.dirty();
            let mode = match ed.state.mode {
                EditorMode::Insert => "INSERT",
                EditorMode::Visual => "VISUAL",
                _ => "NORMAL",
            };
            let title = format!("{}{} — {mode} · :w save · :q close · Esc leave", file.path, if dirty { " ●" } else { "" });
            let theme = EditorTheme::default()
                .base(Style::default().bg(t.bg).fg(t.fg))
                .cursor_style(Style::default().bg(t.accent).fg(t.bg))
                .selection_style(Style::default().bg(t.selection))
                .line_numbers_style(t.muted_style())
                .hide_status_line()
                .block(block(title, focus == Focus::Editor));
            let view = EditorView::new(&mut ed.state)
                .theme(theme)
                .line_numbers(LineNumbers::Absolute)
                .wrap(false)
                .syntax_highlighter(SyntaxHighlighter::new("base16-ocean.dark", &ed.ext).ok());
            f.render_widget(view, body);
            return;
        }

        let focused = focus == Focus::Viewer;
        let height = body.height.saturating_sub(2) as usize;
        self.view_height = height.max(1);
        let (title, lines): (String, Vec<Line>) = if let Some(diff) = &file.diff {
            (format!("{} — git diff · d file", file.path), diff.clone())
        } else if file.data.is_null() {
            (file.path.clone(), vec![Line::styled("loading…", t.muted_style())])
        } else if file.data["binary"] == true {
            (file.path.clone(), vec![Line::styled(format!("Binary file ({})", size(&file.data)), t.muted_style())])
        } else {
            let width = file.lines.len().to_string().len();
            let mut out: Vec<Line> = file
                .lines
                .iter()
                .enumerate()
                .map(|(i, l)| {
                    let n = i + 1;
                    let marked = file.mark == Some(n);
                    let gutter = Span::styled(
                        format!("{n:>width$} "),
                        if marked { t.accent_style().add_modifier(Modifier::BOLD) } else { t.muted_style() },
                    );
                    let mut spans = vec![gutter];
                    spans.extend(l.spans.iter().cloned());
                    let line = Line::from(spans);
                    if marked { line.style(Style::default().add_modifier(Modifier::UNDERLINED)) } else { line }
                })
                .collect();
            if file.data["truncated"] == true {
                out.push(Line::styled(format!("… truncated ({}) — too large to show whole", size(&file.data)), t.muted_style()));
            }
            let hint = if file.editable() { " · e edit · d diff" } else { " · d diff" };
            (format!("{}{hint}", file.path), out)
        };
        let max = lines.len().saturating_sub(height);
        file.scroll = file.scroll.min(max);
        let shown: Vec<Line> = lines.into_iter().skip(file.scroll).take(height).collect();
        f.render_widget(Paragraph::new(shown).block(block(title, focused)), body);
    }

    fn scroll(&mut self, d: isize) {
        if let Some(f) = &mut self.file {
            f.scroll = (f.scroll as isize + d).max(0) as usize;
        }
    }
}

fn size(v: &Value) -> String {
    let n = v["size"].as_f64().unwrap_or(0.0);
    if n < 1024.0 {
        format!("{n} B")
    } else if n < 1024.0 * 1024.0 {
        format!("{:.1} KB", n / 1024.0)
    } else {
        format!("{:.1} MB", n / 1024.0 / 1024.0)
    }
}

impl Screen for FilesTab {
    fn title(&self) -> String {
        "Files".into()
    }

    fn on_show(&mut self, ctx: &mut Ctx) {
        if !self.children.contains_key("") {
            self.load_dir("", ctx);
        }
        if !self.watching {
            ctx.stream("watch", format!("/watch?projects={}", enc(&self.id)));
            self.watching = true;
        }
        if let Some((path, line)) = self.opener.take() {
            self.reveal(&path, ctx);
            self.open(&path, line, ctx);
            if let Some(f) = &mut self.file {
                f.diff = None;
            }
            self.focus = if self.file.as_ref().is_some_and(|f| f.editor.is_some()) { Focus::Editor } else { Focus::Viewer };
        }
    }

    fn captures_text(&self) -> bool {
        self.focus == Focus::Editor
    }

    fn actions(&self) -> Vec<Action> {
        if self.focus == Focus::Editor {
            return vec![];
        }
        let mut a = vec![
            Action::new("e", "Edit the file"),
            Action::new("d", "Toggle the git diff of the file"),
            Action::new("f", "Find a file by name"),
        ];
        if self.focus == Focus::Tree {
            a.extend([
                Action::new("a", "New file"),
                Action::new("A", "New folder"),
                Action::new("r", "Rename / move"),
                Action::new("x", "Delete"),
                Action::new("R", "Reload the tree"),
            ]);
        }
        a
    }

    fn on_action(&mut self, key: &str, ctx: &mut Ctx) {
        match key {
            "e" => {
                if self.file.is_none()
                    && let Some(r) = self.selected().filter(|r| !r.entry.dir).cloned()
                {
                    self.open(&r.entry.path, None, ctx);
                    ctx.info("loading… press e again to edit");
                    return;
                }
                self.start_edit(ctx);
            }
            "d" => self.toggle_diff(ctx),
            "f" => ctx.get("files", self.api("/files")),
            "a" => {
                let dir = self.target_dir();
                ctx.prompt("create-file", format!("New file in /{dir}"), "");
            }
            "A" => {
                let dir = self.target_dir();
                ctx.prompt("create-dir", format!("New folder in /{dir}"), "");
            }
            "r" => {
                if let Some(r) = self.selected().cloned() {
                    ctx.prompt(&format!("rename:{}", r.entry.path), format!("Rename / move {}", r.entry.path), r.entry.path.clone());
                }
            }
            "x" => {
                if let Some(r) = self.selected().cloned() {
                    let what = if r.entry.dir { "folder and everything in it" } else { "file" };
                    ctx.confirm(&format!("delete:{}", r.entry.path), "Delete", format!("Delete the {what} {}?", r.entry.path), true);
                }
            }
            "R" => {
                let dirs: Vec<String> = self.children.keys().cloned().collect();
                for d in dirs {
                    self.load_dir(&d, ctx);
                }
            }
            _ => {}
        }
    }

    fn on_key(&mut self, k: KeyEvent, ctx: &mut Ctx) -> Handled {
        if self.focus == Focus::Editor {
            return self.editor_key(k, ctx);
        }
        let page = self.view_height.max(2) as isize - 1;
        if self.focus == Focus::Viewer {
            match k.code {
                KeyCode::Down | KeyCode::Char('j') => self.scroll(1),
                KeyCode::Up | KeyCode::Char('k') => self.scroll(-1),
                KeyCode::PageDown | KeyCode::Char(' ') => self.scroll(page),
                KeyCode::PageUp => self.scroll(-page),
                KeyCode::Home | KeyCode::Char('g') => self.scroll(-(isize::MAX / 2)),
                KeyCode::End | KeyCode::Char('G') => self.scroll(isize::MAX / 2),
                KeyCode::Esc | KeyCode::Tab | KeyCode::Left | KeyCode::Char('h') => self.focus = Focus::Tree,
                KeyCode::Enter => self.start_edit(ctx),
                _ => return Handled::No,
            }
            return Handled::Yes;
        }
        match k.code {
            KeyCode::Down | KeyCode::Char('j') => self.move_by(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_by(-1),
            KeyCode::PageDown => self.move_by(page),
            KeyCode::PageUp => self.move_by(-page),
            KeyCode::Home | KeyCode::Char('g') => self.move_by(-(isize::MAX / 2)),
            KeyCode::End | KeyCode::Char('G') => self.move_by(isize::MAX / 2),
            KeyCode::Enter => self.toggle(ctx),
            KeyCode::Right | KeyCode::Char('l') => self.expand_or_preview(ctx),
            KeyCode::Left | KeyCode::Char('h') => self.collapse_or_parent(),
            KeyCode::Delete => self.on_action("x", ctx),
            KeyCode::Tab if self.file.is_some() => {
                self.focus = if self.file.as_ref().is_some_and(|f| f.editor.is_some()) { Focus::Editor } else { Focus::Viewer }
            }
            _ => return Handled::No,
        }
        Handled::Yes
    }

    fn on_paste(&mut self, text: &str, _ctx: &mut Ctx) {
        if self.focus == Focus::Editor
            && let Some(ed) = self.file.as_mut().and_then(|f| f.editor.as_mut())
        {
            ed.handler.on_paste_event(text.to_string(), &mut ed.state);
        }
    }

    fn on_stream(&mut self, key: &str, msg: Value, ctx: &mut Ctx) {
        if key == "watch" {
            self.on_watch(&msg, ctx);
        }
    }

    fn on_task(&mut self, key: &str, result: anyhow::Result<Value>, ctx: &mut Ctx) {
        if let Some(dir) = key.strip_prefix("tree:") {
            match result {
                Ok(v) => {
                    let list = v.as_array().into_iter().flatten().map(Entry::from_value).collect();
                    self.children.insert(dir.to_string(), list);
                }
                Err(e) if dir.is_empty() => ctx.error(e),
                Err(_) => {
                    // the folder is gone
                    self.children.remove(dir);
                    self.expanded.remove(dir);
                }
            }
            self.rebuild();
            return;
        }
        if let Some(path) = key.strip_prefix("probe:") {
            if let (Ok(v), Some(f)) = (result, self.file.as_mut())
                && f.path == path
                && v["hash"] != f.data["hash"]
            {
                f.changed = true;
            }
            return;
        }
        if let Some(path) = key.strip_prefix("file:") {
            if self.file.as_ref().is_none_or(|f| f.path != path) {
                return;
            }
            match result {
                Ok(v) => {
                    let reload_editor = self.file.as_ref().and_then(|f| f.editor.as_ref()).is_some_and(|e| !e.dirty());
                    self.set_data(v);
                    if reload_editor && let Some(f) = &mut self.file {
                        let cursor = f.editor.as_ref().map(|e| e.state.cursor).unwrap_or(Index2::new(0, 0));
                        let mut ed = Editor::new(f.data["content"].as_str().unwrap_or_default(), &f.path);
                        let rows = ed.state.lines.len().saturating_sub(1);
                        ed.state.cursor = Index2::new(cursor.row.min(rows), 0);
                        f.editor = Some(ed);
                    }
                }
                Err(e) => {
                    if let Some(f) = &mut self.file {
                        f.gone = true;
                        if f.data.is_null() {
                            ctx.error(e);
                        }
                    }
                }
            }
            return;
        }
        if let Some((untracked, path)) =
            key.strip_prefix("diff:").map(|p| (false, p)).or_else(|| key.strip_prefix("diffu:").map(|p| (true, p)))
        {
            let Some(v) = ctx.ok(result) else { return };
            let Some(f) = self.file.as_mut().filter(|f| f.path == path) else { return };
            let text = v["diff"].as_str().unwrap_or_default();
            if text.trim().is_empty() {
                // an untracked file has no diff against the index: ask for it as untracked
                if untracked {
                    ctx.info("no git changes in this file");
                } else {
                    ctx.get(format!("diffu:{path}"), self.api(&format!("/git/diff?path={}&untracked=1", enc(path))));
                }
                return;
            }
            f.diff = Some(code::diff(text));
            f.scroll = 0;
            return;
        }
        match key {
            "saved" => match result {
                Ok(v) => {
                    self.set_data(v);
                    if let Some(f) = &mut self.file
                        && let Some(ed) = &mut f.editor
                    {
                        ed.original = f.data["content"].as_str().unwrap_or_default().to_string();
                    }
                    ctx.info("saved");
                }
                Err(e) if e.to_string().starts_with("409") => ctx.pick(
                    "conflict",
                    "The file changed on disk since you started editing",
                    vec![
                        ("reload".into(), "Load the version from disk (drop my edits)".into()),
                        ("overwrite".into(), "Overwrite the file with my version".into()),
                        ("keep".into(), "Keep editing".into()),
                    ],
                ),
                Err(e) => ctx.error(e),
            },
            "conflict" => match result.ok().as_ref().and_then(|v| v.as_str()) {
                Some("reload") => {
                    if let Some(f) = &mut self.file {
                        f.editor = None;
                        f.changed = false;
                        let p = f.path.clone();
                        self.load_file(&p, ctx);
                        self.focus = Focus::Viewer;
                    }
                }
                Some("overwrite") => self.save(true, ctx),
                _ => {}
            },
            "editor-cmd" => {
                if let Ok(Value::String(cmd)) = result {
                    self.editor_command(&cmd, ctx);
                }
            }
            "discard" => {
                if result.ok() == Some(Value::Bool(true)) {
                    if let Some(f) = &mut self.file {
                        f.editor = None;
                        if f.changed {
                            let p = f.path.clone();
                            self.load_file(&p, ctx);
                        }
                    }
                    self.focus = Focus::Viewer;
                }
            }
            "files" => {
                let Some(v) = ctx.ok(result) else { return };
                let items: Vec<(String, String)> = v["files"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|p| p.as_str())
                    .map(|p| (p.to_string(), p.to_string()))
                    .collect();
                ctx.pick("find", format!("Find a file ({})", items.len()), items);
            }
            "find" => {
                if let Some(p) = result.ok().as_ref().and_then(|v| v.as_str()) {
                    let p = p.to_string();
                    self.reveal(&p, ctx);
                    self.open(&p, None, ctx);
                    self.focus = Focus::Viewer;
                }
            }
            "create-file" | "create-dir" => {
                let Some(name) = result.ok().and_then(|v| v.as_str().map(|s| s.trim().trim_matches('/').to_string())) else { return };
                if name.is_empty() {
                    return;
                }
                let dir = self.target_dir();
                let path = join(&dir, &name);
                let is_dir = key == "create-dir";
                ctx.send(
                    format!("created:{}:{path}", if is_dir { "d" } else { "f" }),
                    Method::POST,
                    self.api("/fs/create"),
                    json!({ "path": path, "dir": is_dir }),
                );
            }
            k if k.starts_with("created:") => {
                let Some(v) = ctx.ok(result) else { return };
                let path = v["path"].as_str().unwrap_or(&k[10..]).to_string();
                let parent = parent_of(&path).to_string();
                if !parent.is_empty() {
                    self.expanded.insert(parent.clone());
                }
                self.load_dir(&parent, ctx);
                self.reveal(&path, ctx);
                if k.starts_with("created:f") {
                    self.open(&path, None, ctx);
                }
                ctx.info(format!("created {path}"));
            }
            k if k.starts_with("rename:") => {
                let from = k[7..].to_string();
                let Some(to) = result.ok().and_then(|v| v.as_str().map(|s| s.trim().trim_matches('/').to_string())) else { return };
                if to.is_empty() || to == from {
                    return;
                }
                ctx.send(format!("renamed:{from}"), Method::POST, self.api("/fs/rename"), json!({ "from": from, "to": to }));
            }
            k if k.starts_with("renamed:") => {
                let Some(v) = ctx.ok(result) else { return };
                let from = &k[8..];
                let to = v["path"].as_str().unwrap_or_default().to_string();
                for d in [parent_of(from).to_string(), parent_of(&to).to_string()] {
                    self.load_dir(&d, ctx);
                }
                if self.file.as_ref().is_some_and(|f| f.path == from) {
                    self.file = None;
                    self.open(&to, None, ctx);
                }
                self.reveal(&to, ctx);
                ctx.info(format!("renamed to {to}"));
            }
            k if k.starts_with("delete:") => {
                if result.ok() == Some(Value::Bool(true)) {
                    let path = k[7..].to_string();
                    ctx.send(format!("deleted:{path}"), Method::POST, self.api("/fs/delete"), json!({ "path": path }));
                }
            }
            k if k.starts_with("deleted:") => {
                if ctx.ok(result).is_none() {
                    return;
                }
                let path = &k[8..];
                self.load_dir(parent_of(path), ctx);
                if self.file.as_ref().is_some_and(|f| f.path == path || f.path.starts_with(&format!("{path}/"))) {
                    self.file = None;
                    self.focus = Focus::Tree;
                }
                ctx.info(format!("deleted {path}"));
            }
            _ => {}
        }
    }

    fn draw(&mut self, f: &mut Frame, area: Rect, _ctx: &Ctx) {
        let tree_w = (area.width / 3).clamp(24, 48);
        let [tree, file] = Layout::horizontal([Constraint::Length(tree_w), Constraint::Min(20)]).areas(area);
        self.draw_tree(f, tree);
        self.draw_file(f, file);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(path: &str, dir: bool) -> Entry {
        Entry { path: path.into(), name: path.rsplit('/').next().unwrap().into(), dir, ignored: false }
    }

    #[test]
    fn flattens_expanded_folders() {
        let mut children = HashMap::new();
        children.insert("".to_string(), vec![e("src", true), e("docs", true), e("README.md", false)]);
        children.insert("src".to_string(), vec![e("src/ui", true), e("src/main.rs", false)]);
        children.insert("src/ui".to_string(), vec![e("src/ui/mod.rs", false)]);
        children.insert("docs".to_string(), vec![e("docs/a.md", false)]);
        let mut expanded = HashSet::new();
        expanded.insert("src".to_string());
        expanded.insert("src/ui".to_string());
        let rows = flatten(&children, &expanded);
        let paths: Vec<(&str, usize)> = rows.iter().map(|r| (r.entry.path.as_str(), r.depth)).collect();
        assert_eq!(paths, vec![("src", 0), ("src/ui", 1), ("src/ui/mod.rs", 2), ("src/main.rs", 1), ("docs", 0), ("README.md", 0)]);
        assert!(rows[0].open && !rows[4].open);
        // a folder expanded but not loaded yet shows no children
        expanded.insert("missing".into());
        assert_eq!(flatten(&children, &expanded).len(), 6);
    }

    #[test]
    fn parents_and_reloads() {
        assert_eq!(parent_of("a/b/c.rs"), "a/b");
        assert_eq!(parent_of("c.rs"), "");
        let loaded = ["".to_string(), "src".to_string(), "docs".to_string()];
        assert_eq!(dirs_to_reload(loaded.iter(), &["src/x.rs".into()], false), vec!["src".to_string()]);
        assert_eq!(dirs_to_reload(loaded.iter(), &["new.txt".into(), "docs".into()], false), vec!["".to_string(), "docs".to_string()]);
        assert_eq!(dirs_to_reload(loaded.iter(), &[], true).len(), 3);
    }

    #[test]
    fn editor_round_trips_text() {
        let ed = Editor::new("fn main() {}\n", "main.rs");
        assert_eq!(ed.text(), "fn main() {}\n");
        assert!(!ed.dirty());
    }
}
