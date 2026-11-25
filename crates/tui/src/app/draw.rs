//! Drawing the app shell: the frame layout, status line, sidebar, which-key panel and
//! modals.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};
use serde_json::Value;

use crate::core::{Hit, Modal, Route, Section};
use crate::ui::input::TextInput;
use crate::ui::theme::icon;
use crate::ui::{Theme, block, centered};

use super::*;

impl App {
    pub fn draw(&mut self, f: &mut Frame) {
        let t = Theme::get();
        self.ctx.hits.borrow_mut().clear();
        self.ctx.draw_prefix.borrow_mut().clear();
        if let Some(form) = &self.login {
            form.draw(f, self.ctx.client.base());
            return;
        }
        // the theme's background and text everywhere
        f.render_widget(Block::default().style(t.base()), f.area());
        let [main, status] = Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).areas(f.area());
        let content = if self.sidebar && main.width > 90 {
            let [side, content] = Layout::horizontal([Constraint::Length(30), Constraint::Min(40)]).areas(main);
            self.draw_sidebar(f, side);
            content
        } else {
            main
        };
        let route = self.route.clone();
        if let Some(slot) = self.screens.get_mut(&route) {
            self.ctx.screen = slot.id;
            slot.screen.draw(f, content, &self.ctx);
        }
        self.draw_status(f, status);
        match self.overlay {
            Overlay::Leader(menu) => {
                let title = match menu {
                    Menu::Root => format!("{}  leader", icon("\u{f030c}", "")),
                    Menu::Actions => {
                        let screen = self.screens.get(&self.route).map(|s| &s.screen);
                        let help = screen.and_then(|s| s.help()).map(|h| format!(": {h}")).unwrap_or_default();
                        format!("{}  {}{help}", icon("\u{f02d6}", "?"), screen.map(|s| s.title()).unwrap_or_default())
                    }
                    Menu::Windows => format!("{}  windows", icon("\u{f05b6}", "")),
                    Menu::Tabs => format!("{}  tabs", icon("\u{f04e9}", "")),
                };
                let entries = self.leader_entries(menu);
                self.draw_which_key(f, main, title, entries);
            }
            Overlay::None => {}
        }
        if let Some((_, modal, input)) = &self.modal {
            draw_modal(f, main, modal, input);
        }
    }

    /// Mode, where you are, the message, agents at work, the hub — like Neovim's lualine.
    fn draw_status(&self, f: &mut Frame, area: Rect) {
        let t = Theme::get();
        let (mode, color) = if self.modal.is_some() {
            if matches!(self.modal, Some((_, Modal::Pick { ref key, .. }, _)) if key == "palette") {
                ("COMMAND", t.warn)
            } else {
                ("PROMPT", t.magenta)
            }
        } else if matches!(self.overlay, Overlay::Leader(_)) {
            ("LEADER", t.magenta)
        } else if self.typing() {
            (self.screens.get(&self.route).and_then(|s| s.screen.mode()).unwrap_or("INSERT"), t.ok)
        } else {
            ("NORMAL", t.accent)
        };
        let pill = Style::default().fg(t.bg).bg(color).add_modifier(Modifier::BOLD);
        let mut left =
            vec![Span::styled(format!(" {mode} "), pill), Span::styled(icon("\u{e0b4}", ""), Style::default().fg(color).bg(t.bg_dark))];
        let crumb = self.breadcrumb();
        left.push(Span::styled(format!(" {crumb} "), Style::default().fg(t.fg).bg(t.bg_dark)));
        left.push(Span::styled(icon("\u{e0b4}", ""), Style::default().fg(t.bg_dark).bg(t.bg)));
        if !self.pending.is_empty() || self.count.is_some() {
            left.push(Span::styled(
                format!("  {}{}", self.count.map(|c| c.to_string()).unwrap_or_default(), self.pending.replace('^', "Ctrl+")),
                t.accent_style(),
            ));
        }
        let msg = match &self.ctx.status {
            Some((m, true)) => Span::styled(format!("  {m}"), Style::default().fg(t.danger)),
            Some((m, false)) => Span::styled(format!("  {m}"), t.dim_style()),
            None => Span::raw(""),
        };
        let has_msg = self.ctx.status.is_some();
        left.push(msg);
        if !has_msg {
            // the screen's main keys, as many as fit next to the right part (about 34 columns)
            let used: usize = left.iter().map(|s| s.content.chars().count()).sum();
            let mut room = (area.width as usize).saturating_sub(used + 34 + 16);
            let hints = self.screens.get(&self.route).map(|s| s.screen.hints()).unwrap_or_default();
            for (key, label) in hints {
                let label = label.split(" (").next().unwrap_or(&label);
                let label: String =
                    if label.chars().count() > 18 { label.chars().take(17).collect::<String>() + "…" } else { label.to_string() };
                let w = key.chars().count() + label.chars().count() + 3;
                if w > room {
                    break;
                }
                room -= w;
                let x = area.x + left.iter().map(|s| s.content.chars().count() as u16).sum::<u16>();
                self.hit(Rect { x: x + 2, y: area.y, width: w as u16 - 2, height: 1 }, Hit::Action(key.clone()));
                left.push(Span::styled(format!("  {key}"), Style::default().fg(t.accent)));
                left.push(Span::styled(format!(" {label}"), t.dim_style()));
            }
            let x = area.x + left.iter().map(|s| s.content.chars().count() as u16).sum::<u16>();
            self.hit(Rect { x: x + 3, y: area.y, width: 6, height: 1 }, Hit::Key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE)));
            self.hit(Rect { x: x + 12, y: area.y, width: 8, height: 1 }, Hit::Key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE)));
            left.push(Span::styled("   ? more · ␣ leader", t.muted_style()));
        }
        let sessions = &self.ctx.shared.sessions;
        let running = sessions.iter().filter(|s| s["status"] == "running").count();
        let waiting = sessions.iter().filter(|s| s["status"] == "waiting").count();
        let host = self.ctx.shared.base_url.trim_start_matches("http://").trim_start_matches("https://").to_string();
        let mut right = vec![];
        if running > 0 {
            right.push(Span::styled(format!(" ● {running} "), Style::default().fg(t.ok)));
        }
        if waiting > 0 {
            right.push(Span::styled(format!(" ? {waiting} "), Style::default().fg(t.warn).add_modifier(Modifier::BOLD)));
        }
        right.push(Span::styled(icon("\u{e0b6}", ""), Style::default().fg(t.bg_dark).bg(t.bg)));
        let host = match &self.user {
            Some(u) => format!("{u}@{host}"),
            None => host,
        };
        right.push(Span::styled(format!(" {} {host} ", icon("󰒋", "hub")), Style::default().fg(t.dim).bg(t.bg_dark)));
        right
            .push(Span::styled(format!(" {} ", icon("󰣇", "hubness")), Style::default().fg(t.bg).bg(t.accent).add_modifier(Modifier::BOLD)));
        let rw: u16 = right.iter().map(|s| s.content.chars().count() as u16).sum();
        let [l, r] = Layout::horizontal([Constraint::Min(10), Constraint::Length(rw)]).areas(area);
        f.render_widget(Paragraph::new(Line::from(left)).style(Style::default().bg(t.bg)), l);
        f.render_widget(Paragraph::new(Line::from(right)).style(Style::default().bg(t.bg)), r);
    }

    fn breadcrumb(&self) -> String {
        let shared = &self.ctx.shared;
        let name =
            |list: &Vec<Value>, id: &str| list.iter().find(|x| x["id"] == id).and_then(|x| x["name"].as_str()).unwrap_or("…").to_string();
        let title = self.screens.get(&self.route).map(|s| s.screen.title()).unwrap_or_default();
        match &self.route {
            Route::Home => format!("{} Chats", icon("󰭹", "")),
            Route::Chat(_) => format!("{} Chats › {title}", icon("󰭹", "")),
            Route::Projects => format!("{} Projects", icon("\u{f401}", "")),
            Route::Project(id) => format!("{} Projects › {}", icon("\u{f401}", ""), name(&shared.projects, id)),
            Route::ProjectSession { project, .. } => format!("{} {} › {title}", icon("\u{f401}", ""), name(&shared.projects, project)),
            Route::Group(id) => format!("{} Groups › {}", icon("󰉋", ""), name(&shared.groups, id)),
            Route::GroupSession { group, .. } => format!("{} {} › {title}", icon("󰉋", ""), name(&shared.groups, group)),
            Route::System(_) => format!("{} System", icon("󰍹", "")),
            Route::Harness(_) => format!("{} Harness", icon("󰚩", "")),
            Route::Settings(_) => format!("{} Settings", icon("\u{f013}", "")),
        }
        .trim()
        .to_string()
    }

    fn draw_sidebar(&self, f: &mut Frame, area: Rect) {
        let t = Theme::get();
        let section = self.route.section();
        let mut lines: Vec<Line> = vec![
            Line::raw(""),
            Line::from(vec![
                Span::styled(format!("  {} ", icon("\u{f056e}", "◆")), Style::default().fg(t.accent)),
                Span::styled("hubness", Style::default().fg(t.fg).add_modifier(Modifier::BOLD)),
            ]),
            Line::raw(""),
        ];
        for (s, label, ic) in [
            (Section::Chats, "Chats", icon("\u{f0b79}", "›")),
            (Section::Projects, "Projects", icon("\u{f024b}", "›")),
            (Section::System, "System", icon("\u{f0379}", "›")),
            (Section::Harness, "Agents", icon("\u{f06a9}", "›")),
            (Section::Settings, "Settings", icon("\u{f0493}", "›")),
        ] {
            let target = match s {
                Section::Chats => Route::Home,
                Section::Projects => Route::Projects,
                Section::System => Route::System(None),
                Section::Harness => Route::Harness(None),
                Section::Settings => Route::Settings(None),
            };
            self.hit(Rect { x: area.x, y: area.y + lines.len() as u16, width: area.width, height: 1 }, Hit::Go(target));
            let on = s == section;
            let line = if on {
                Line::from(vec![
                    Span::styled("▌", Style::default().fg(t.accent)),
                    Span::styled(format!(" {ic}  {label}"), Style::default().fg(t.accent).add_modifier(Modifier::BOLD)),
                ])
            } else {
                Line::from(vec![Span::raw(" "), Span::styled(format!(" {ic}  {label}"), Style::default().fg(t.dim))])
            };
            lines.push(line);
        }
        let sessions = &self.ctx.shared.sessions;
        let running = sessions.iter().filter(|s| s["status"] == "running" || s["status"] == "waiting").count();
        lines.push(Line::raw(""));
        lines.push(Line::from(vec![
            Span::styled("  SESSIONS", t.muted_style().add_modifier(Modifier::BOLD)),
            Span::styled(if running > 0 { format!("  {running} active") } else { String::new() }, Style::default().fg(t.ok)),
        ]));
        let current = match &self.route {
            Route::Chat(id) | Route::ProjectSession { session: id, .. } | Route::GroupSession { session: id, .. } => Some(id.as_str()),
            _ => None,
        };
        let room = area.height.saturating_sub(lines.len() as u16 + 1) as usize;
        let width = area.width.saturating_sub(6) as usize;
        for s in sessions.iter().take(room) {
            let (mark, color) = t.status(s["status"].as_str().unwrap_or("idle"));
            let on = current == s["id"].as_str();
            let mut title: String = s["title"].as_str().unwrap_or_default().chars().take(width).collect();
            if s["title"].as_str().unwrap_or_default().chars().count() > width {
                title.pop();
                title.push('…');
            }
            let style = if on { t.selected() } else { Style::default().fg(t.fg) };
            if let Some(id) = s["id"].as_str() {
                self.hit(Rect { x: area.x, y: area.y + lines.len() as u16, width: area.width, height: 1 }, Hit::Go(self.session_route(id)));
            }
            lines.push(Line::from(vec![Span::styled(format!("  {mark} "), Style::default().fg(color)), Span::styled(title, style)]));
        }
        let b = Block::default().borders(Borders::RIGHT).border_style(t.line_style()).style(Style::default().bg(t.bg_dark));
        f.render_widget(Paragraph::new(lines).block(b), area);
    }

    /// which-key, LazyVim style: a compact box at the bottom right, groups marked `+`.
    fn draw_which_key(&self, f: &mut Frame, area: Rect, title: String, entries: Vec<Entry>) {
        let t = Theme::get();
        if entries.is_empty() {
            return;
        }
        let key_w = entries.iter().map(|e| e.key.chars().count()).max().unwrap_or(1).max(1);
        let label_w = entries.iter().map(|e| e.label.chars().count() + usize::from(e.group)).max().unwrap_or(10).min(34);
        let col_w = (key_w + 3 + label_w + 2) as u16;
        let max_rows = (area.height.saturating_sub(4)).clamp(4, 14) as usize;
        let cols = entries.len().div_ceil(max_rows).max(1);
        let rows = entries.len().div_ceil(cols);
        // wide enough for the title too (it may carry the screen's explanation)
        let title_w = title.chars().count() as u16 + 4;
        let w = (col_w * cols as u16 + 4).max(title_w).min(area.width.saturating_sub(2));
        let h = (rows as u16 + 2).min(area.height);
        let rect = Rect { x: area.x + area.width.saturating_sub(w + 1), y: area.y + area.height.saturating_sub(h), width: w, height: h };
        f.render_widget(Clear, rect);
        let b = block(title, true).style(Style::default().bg(t.bg_dark));
        let inner = b.inner(rect);
        f.render_widget(b, rect);
        // a click on the box between entries keeps it open (entries are registered on top)
        self.hit(rect, Hit::Key(KeyEvent::new(KeyCode::Null, KeyModifiers::NONE)));
        for (i, e) in entries.iter().enumerate() {
            let (c, r) = (i / rows, i % rows);
            let cell = Rect { x: inner.x + c as u16 * col_w, y: inner.y + r as u16, width: col_w.min(inner.width), height: 1 };
            let label: String = if e.group { format!("+{}", e.label) } else { e.label.clone() };
            let label: String =
                if label.chars().count() > label_w { label.chars().take(label_w - 1).collect::<String>() + "…" } else { label };
            let line = Line::from(vec![
                Span::styled(format!("{:>key_w$}", e.key), Style::default().fg(t.accent).add_modifier(Modifier::BOLD)),
                Span::styled(" ➜ ", t.muted_style()),
                Span::styled(
                    label,
                    if e.group { Style::default().fg(t.magenta).add_modifier(Modifier::BOLD) } else { Style::default().fg(t.fg) },
                ),
            ]);
            f.render_widget(Paragraph::new(line), cell);
            let c = if e.key == "␣" { ' ' } else { e.key.chars().next().unwrap_or(' ') };
            self.hit(cell, Hit::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)));
        }
    }
}

fn draw_modal(f: &mut Frame, area: Rect, modal: &Modal, input: &TextInput) {
    let t = Theme::get();
    match modal {
        Modal::Input { title, multiline, .. } => {
            let h = if *multiline { 10 } else { 3 };
            let r = centered(area, 70, h);
            f.render_widget(Clear, r);
            input.draw(f, r, block(title.clone(), true), true, "");
        }
        Modal::Confirm { title, message, danger, .. } => {
            // wrapped lines count too (62 columns inside, with slack for word wrapping)
            let lines = message.lines().map(|l| l.chars().count().div_ceil(56).max(1)).sum::<usize>() as u16 + 4;
            let r = centered(area, 64, lines.max(5));
            f.render_widget(Clear, r);
            let b = block(title.clone(), true);
            let inner = b.inner(r);
            f.render_widget(b, r);
            let mut text: Vec<Line> = message.lines().map(|l| Line::raw(l.to_string())).collect();
            text.push(Line::raw(""));
            let yes = if *danger {
                Style::default().fg(t.danger).add_modifier(Modifier::BOLD)
            } else {
                t.accent_style().add_modifier(Modifier::BOLD)
            };
            text.push(Line::from(vec![Span::styled("[y] confirm", yes), Span::styled("   [n / Esc] cancel", t.muted_style())]));
            f.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), inner);
        }
        Modal::Message { title, text } => {
            let r = centered(area, 80, (text.lines().count() as u16 + 3).min(area.height));
            f.render_widget(Clear, r);
            f.render_widget(Paragraph::new(text.clone()).wrap(Wrap { trim: false }).block(block(title.clone(), true)), r);
        }
        Modal::Pick { title, items, filter, selected, .. } => {
            let visible = filtered(items, filter);
            let h = (visible.len() as u16 + 4).clamp(6, area.height.saturating_sub(2));
            let r = centered(area, 80, h);
            f.render_widget(Clear, r);
            let b = block(title.clone(), true);
            let inner = b.inner(r);
            f.render_widget(b, r);
            let [q, list] = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(inner);
            f.render_widget(Paragraph::new(Line::from(vec![Span::styled("› ", t.accent_style()), Span::raw(filter.clone())])), q);
            let rows = list.height as usize;
            let start = selected.saturating_sub(rows.saturating_sub(1));
            let items: Vec<ListItem> = visible
                .iter()
                .enumerate()
                .skip(start)
                .take(rows)
                .map(|(i, (_, label))| {
                    ListItem::new(Line::styled(format!(" {label}"), if i == *selected { t.selected() } else { Style::default() }))
                })
                .collect();
            f.render_widget(List::new(items), list);
        }
    }
    let _ = Direction::Vertical;
}
