//! An agent's extensions inside its Harness tab (the web app's Extensions card): skills,
//! rules files, subagents, commands and plugins read from the agent's own folders (and a
//! picked project's). `e` on a rules file opens it in the editor (Vim keys, `:w` / Ctrl+S
//! save, `:q` / Esc close); only files the hub listed can be opened or saved.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use edtui::clipboard::InternalClipboard;
use edtui::{EditorEventHandler, EditorMode, EditorState, EditorTheme, EditorView, LineNumbers, Lines, SyntaxHighlighter};
use hubness_client::Method;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{List, ListItem, Paragraph, Wrap};
use serde_json::{Value, json};

use super::kit::{Nav, click_items, click_select, clicked_row, s};
use super::session::enc;
use crate::core::{Action, Ctx, Handled};
use crate::ui::{Theme, block};

/// The sections after the overview, with their keys in the extensions response.
pub const SECTIONS: [(&str, &str); 5] =
    [("Skills", "skills"), ("Rules", "rules"), ("Agents", "agents"), ("Commands", "commands"), ("Plugins", "plugins")];

struct RulesEditor {
    path: String,
    state: EditorState,
    handler: EditorEventHandler,
    original: String,
}

impl RulesEditor {
    fn new(path: &str, content: &str) -> Self {
        let mut state = EditorState::new(Lines::from(content));
        state.set_clipboard(InternalClipboard::default());
        state.mode = EditorMode::Normal;
        Self { path: path.to_string(), state, handler: EditorEventHandler::vim_mode(), original: content.to_string() }
    }

    fn text(&self) -> String {
        self.state.lines.flatten(&Some('\n')).into_iter().collect()
    }

    fn dirty(&self) -> bool {
        self.text() != self.original
    }
}

/// A path with the home folder shown as `~`.
pub fn tilde(path: &str) -> String {
    match std::env::var("HOME") {
        Ok(h) if !h.is_empty() && path.starts_with(&format!("{h}/")) => format!("~{}", &path[h.len()..]),
        _ => path.to_string(),
    }
}

pub struct ExtView {
    kind: String,
    data: Value,
    error: Option<String>,
    nav: Nav,
    project: Option<(String, String)>,
    editor: Option<RulesEditor>,
    pub loaded: bool,
}

impl ExtView {
    pub fn new(kind: &str) -> Self {
        Self { kind: kind.to_string(), data: Value::Null, error: None, nav: Nav::default(), project: None, editor: None, loaded: false }
    }

    fn project_query(&self, first: bool) -> String {
        self.project.as_ref().map(|(id, _)| format!("{}project={}", if first { '?' } else { '&' }, enc(id))).unwrap_or_default()
    }

    pub fn load(&mut self, ctx: &Ctx) {
        self.loaded = true;
        ctx.get("ext", format!("/harness/{}/extensions{}", self.kind, self.project_query(true)));
    }

    pub fn editing(&self) -> bool {
        self.editor.is_some()
    }

    fn items(&self, section: usize) -> Vec<Value> {
        let key = SECTIONS.get(section).map(|(_, k)| *k).unwrap_or("skills");
        self.data[key].as_array().cloned().unwrap_or_default()
    }

    /// Counts for the section pills.
    pub fn count(&self, section: usize) -> Option<usize> {
        if self.data.is_null() {
            return None;
        }
        let items = self.items(section);
        Some(if SECTIONS[section].1 == "rules" { items.iter().filter(|r| r["exists"] == true).count() } else { items.len() })
    }

    fn open_rules(&self, ctx: &mut Ctx) {
        let Some(r) = self.nav.selected().and_then(|i| self.items(1).get(i).cloned()) else { return };
        let path = s(&r["path"], "");
        ctx.get("rules-open", format!("/harness/{}/rules?path={}{}", self.kind, enc(path), self.project_query(false)));
    }

    fn save(&self, ctx: &mut Ctx) {
        let Some(ed) = &self.editor else { return };
        ctx.send(
            "rules-saved",
            Method::PUT,
            format!("/harness/{}/rules?path={}{}", self.kind, enc(&ed.path), self.project_query(false)),
            json!({ "content": ed.text() }),
        );
    }

    fn details(&self, section: usize, ctx: &mut Ctx) {
        let Some(it) = self.nav.selected().and_then(|i| self.items(section).get(i).cloned()) else { return };
        if SECTIONS[section].1 == "rules" {
            return self.open_rules(ctx);
        }
        let mut text = String::new();
        if let Some(d) = it["description"].as_str().filter(|d| !d.is_empty()) {
            text.push_str(d);
            text.push_str("\n\n");
        }
        if let Some(m) = it["model"].as_str() {
            text.push_str(&format!("model: {m}\n"));
        }
        if let Some(v) = it["version"].as_str() {
            text.push_str(&format!("version: {v}\n"));
        }
        text.push_str(&format!("scope: {}\n{}", s(&it["scope"], ""), tilde(s(&it["path"], ""))));
        ctx.message(s(&it["name"], "").to_string(), text);
    }

    pub fn actions(&self, section: usize) -> Vec<Action> {
        if self.editor.is_some() {
            return vec![];
        }
        let mut a = vec![Action::new("P", "Show a project's extensions too")];
        if SECTIONS.get(section).is_some_and(|(_, k)| *k == "rules") {
            a.insert(0, Action::new("e", "Edit the rules file"));
        }
        a
    }

    /// A screen action of the section; `true` when it was one.
    pub fn on_action(&mut self, key: &str, section: usize, ctx: &mut Ctx) -> bool {
        if self.editor.is_some() {
            match key {
                ":w" | "Ctrl+S" => self.save(ctx),
                ":q" => self.editor_command("q", ctx),
                _ => return false,
            }
            return true;
        }
        match key {
            "e" if SECTIONS.get(section).is_some_and(|(_, k)| *k == "rules") => self.open_rules(ctx),
            "P" => {
                let mut items = vec![("".to_string(), "No project — the user's own folders".to_string())];
                items.extend(
                    ctx.shared
                        .projects
                        .iter()
                        .filter(|p| p["kind"] == "normal")
                        .map(|p| (format!("{}\u{1f}{}", s(&p["id"], ""), s(&p["name"], "")), s(&p["name"], "").to_string())),
                );
                ctx.pick("ext-project", "Show a project's extensions too", items);
            }
            _ => return false,
        }
        true
    }

    pub fn on_key(&mut self, k: KeyEvent, section: usize, ctx: &mut Ctx) -> Handled {
        if self.editor.is_some() {
            return self.editor_key(k, ctx);
        }
        let n = self.items(section).len();
        if self.nav.key(&k, n) {
            return Handled::Yes;
        }
        match k.code {
            KeyCode::Enter => self.details(section, ctx),
            _ => return Handled::No,
        }
        Handled::Yes
    }

    pub fn reset_selection(&mut self) {
        self.nav.reset();
    }

    fn editor_command(&mut self, cmd: &str, ctx: &mut Ctx) {
        let dirty = self.editor.as_ref().is_some_and(RulesEditor::dirty);
        match cmd.trim() {
            "w" => self.save(ctx),
            "wq" | "x" => {
                if dirty {
                    self.save(ctx);
                }
                self.editor = None;
            }
            "q" if dirty => ctx.error("unsaved changes — :w to save, :q! to discard"),
            "q" | "q!" => self.editor = None,
            "" => {}
            other => ctx.error(format!("not an editor command: :{other}")),
        }
    }

    fn editor_key(&mut self, k: KeyEvent, ctx: &mut Ctx) -> Handled {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let normal = self.editor.as_ref().is_some_and(|e| e.state.mode == EditorMode::Normal);
        match k.code {
            KeyCode::Char('s') if ctrl => self.save(ctx),
            KeyCode::Char(':') if normal => ctx.prompt("ext-editor-cmd", ":", ""),
            KeyCode::Esc if normal => {
                if self.editor.as_ref().is_some_and(RulesEditor::dirty) {
                    ctx.confirm("ext-discard", "Unsaved changes", "Discard the unsaved changes of this file?", true);
                } else {
                    self.editor = None;
                }
            }
            _ => {
                if let Some(ed) = self.editor.as_mut() {
                    ed.handler.on_key_event(k, &mut ed.state);
                }
            }
        }
        Handled::Yes
    }

    pub fn on_paste(&mut self, text: &str) {
        if let Some(ed) = self.editor.as_mut() {
            ed.handler.on_paste_event(text.to_string(), &mut ed.state);
        }
    }

    /// The status line mode while the editor is open.
    pub fn mode(&self) -> Option<&'static str> {
        Some(match self.editor.as_ref()?.state.mode {
            EditorMode::Insert => "INSERT",
            EditorMode::Visual => "VISUAL",
            _ => "EDIT",
        })
    }

    pub fn hints(&self) -> Option<Vec<(String, String)>> {
        self.editor.as_ref()?;
        Some(vec![
            (":w".into(), "save".into()),
            (":q".into(), "close".into()),
            ("i".into(), "type".into()),
            ("Esc".into(), "normal".into()),
        ])
    }

    /// A task of this view; `true` when it was one.
    pub fn on_task(&mut self, key: &str, result: anyhow::Result<Value>, ctx: &mut Ctx) -> bool {
        match key {
            "ext" => match result {
                Ok(v) => {
                    self.data = v;
                    self.error = None;
                }
                Err(e) => self.error = Some(e.to_string()),
            },
            "ext-project" => {
                if let Ok(Value::String(id)) = result {
                    self.project = id.split_once('\u{1f}').map(|(i, n)| (i.to_string(), n.to_string()));
                    self.nav.reset();
                    self.load(ctx);
                }
            }
            "rules-open" => {
                if let Some(v) = ctx.ok(result) {
                    self.editor = Some(RulesEditor::new(s(&v["path"], ""), s(&v["content"], "")));
                    ctx.info("i / a / o type · Esc stops typing · :w or Ctrl+S saves · :q closes");
                }
            }
            "rules-saved" => {
                if ctx.ok(result).is_some() {
                    if let Some(ed) = &mut self.editor {
                        ed.original = ed.text();
                    }
                    ctx.info("saved");
                    self.load(ctx);
                }
            }
            "ext-editor-cmd" => {
                if let Ok(Value::String(cmd)) = result {
                    self.editor_command(&cmd, ctx);
                }
            }
            "ext-discard" => {
                if matches!(result, Ok(Value::Bool(true))) {
                    self.editor = None;
                }
            }
            _ => return false,
        }
        true
    }

    /// A click on a row of the list; `true` when it was one.
    pub fn on_click(&mut self, id: &str, section: usize, ctx: &mut Ctx) -> bool {
        let Some(i) = clicked_row(id, "ext") else { return false };
        if click_select(&mut self.nav.state, i) {
            self.details(section, ctx);
        }
        true
    }

    pub fn draw(&mut self, f: &mut Frame, area: Rect, section: usize, ctx: &Ctx) {
        let t = Theme::get();
        if let Some(ed) = &mut self.editor {
            let dirty = if ed.dirty() { " ● unsaved" } else { "" };
            let mode = match ed.state.mode {
                EditorMode::Insert => "INSERT",
                EditorMode::Visual => "VISUAL",
                _ => "NORMAL",
            };
            let theme = EditorTheme::default()
                .base(Style::default().bg(t.bg).fg(t.fg))
                .cursor_style(Style::default().bg(t.accent).fg(t.bg))
                .selection_style(Style::default().bg(t.selection))
                .line_numbers_style(t.muted_style())
                .hide_status_line()
                .block(block(format!("{}{dirty}  [{mode}]", tilde(&ed.path)), true));
            let view = EditorView::new(&mut ed.state)
                .theme(theme)
                .line_numbers(LineNumbers::Absolute)
                .wrap(true)
                .syntax_highlighter(SyntaxHighlighter::new("base16-ocean.dark", "md").ok());
            f.render_widget(view, area);
            return;
        }
        let (label, key) = SECTIONS[section];
        let project = self.project.as_ref().map(|(_, n)| format!(" · project {n}")).unwrap_or_default();
        let hint = if key == "rules" { "Enter / e edit · P project" } else { "Enter details · P project" };
        let title = format!("{label}{project} — {hint}");
        if self.data.is_null() {
            let msg = self.error.clone().unwrap_or_else(|| "Loading…".into());
            f.render_widget(Paragraph::new(msg).wrap(Wrap { trim: false }).block(block(title, true)), area);
            return;
        }
        let items = self.items(section);
        if items.is_empty() {
            let empty = match key {
                "skills" => "No skills found.",
                "agents" => "No subagents found.",
                "commands" => "No custom commands found.",
                "plugins" => "No plugins installed.",
                _ => "No rules files.",
            };
            f.render_widget(Paragraph::new(Line::styled(format!(" {empty}"), t.muted_style())).block(block(title, true)), area);
            return;
        }
        let width = area.width.saturating_sub(6) as usize;
        let short = |x: &str| -> String {
            let one = x.replace('\n', " ");
            if one.chars().count() > width { format!("{}…", one.chars().take(width.saturating_sub(1)).collect::<String>()) } else { one }
        };
        let list: Vec<ListItem> = items
            .iter()
            .map(|it| {
                let scope = s(&it["scope"], "");
                let scope_style = match scope {
                    "project" => t.accent_style(),
                    "plugin" => Style::default().fg(t.warn),
                    _ => t.muted_style(),
                };
                let (name, sub) = if key == "rules" {
                    let state = if it["exists"] == true { format!("{} B", it["size"]) } else { "not created yet".into() };
                    (s(&it["label"], "").to_string(), format!("{state} · {}", tilde(s(&it["path"], ""))))
                } else if key == "plugins" {
                    let on = if it["enabled"] == true { "enabled" } else { "disabled" };
                    let ver = it["version"].as_str().map(|v| format!(" {v}")).unwrap_or_default();
                    (format!("{}{ver}", s(&it["name"], "")), format!("{on} · {}", tilde(s(&it["path"], ""))))
                } else {
                    let prefix = if key == "commands" { "/" } else { "" };
                    let model = it["model"].as_str().map(|m| format!("  [{m}]")).unwrap_or_default();
                    let desc = s(&it["description"], "");
                    (
                        format!("{prefix}{}{model}", s(&it["name"], "")),
                        if desc.is_empty() { tilde(s(&it["path"], "")) } else { desc.to_string() },
                    )
                };
                ListItem::new(Text::from(vec![
                    Line::from(vec![Span::styled(format!(" {name}"), t.bold()), Span::styled(format!("  {scope}"), scope_style)]),
                    Line::styled(format!("   {}", short(&sub)), t.muted_style()),
                ]))
            })
            .collect();
        self.nav.clamp(list.len());
        f.render_stateful_widget(List::new(list).block(block(title, true)).highlight_style(t.selected()), area, &mut self.nav.state);
        click_items(ctx, "ext", area, true, &self.nav.state, &vec![2; items.len()]);
    }
}
