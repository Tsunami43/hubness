//! System → Packages: a tab per package manager on the server (pacman, AUR, cargo, npm…):
//! updates, the installed list with a filter, package info and "ask" for system packages.

use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, Paragraph, Wrap};
use serde_json::{Value, json};

use super::{Ask, enc};
use crate::core::{Action, Ctx, Handled, Screen};
use crate::screens::kit::{Nav, TextView, matches, s, since};
use crate::ui::{Theme, block};

#[derive(Default)]
pub struct Packages {
    managers: Option<Vec<Value>>,
    current: Option<String>,
    data: HashMap<String, Value>,
    loading: Option<String>,
    error: Option<String>,
    filter: String,
    nav: Nav,
    view: Option<TextView>,
    ask: Ask,
}

impl Packages {
    /// "Ask the agent" and package info know system packages (pacman -Qi).
    fn askable(&self) -> bool {
        matches!(self.current.as_deref(), Some("pacman" | "aur"))
    }

    fn cur(&self) -> Option<&Value> {
        self.current.as_ref().and_then(|m| self.data.get(m))
    }

    fn list(&self) -> Vec<&Value> {
        self.cur()
            .and_then(|d| d["packages"].as_array())
            .into_iter()
            .flatten()
            .filter(|p| matches(&format!("{} {}", s(&p["name"], ""), s(&p["note"], "")), &self.filter))
            .collect()
    }

    fn selected(&self) -> Option<String> {
        self.nav.selected().and_then(|i| self.list().get(i).map(|p| s(&p["name"], "").to_string()))
    }

    fn load(&mut self, ctx: &Ctx, refresh: bool) {
        if let Some(m) = self.current.clone() {
            self.loading = Some(m.clone());
            self.error = None;
            ctx.get(format!("pkgs:{m}"), format!("/system/packages/{m}{}", if refresh { "?refresh=1" } else { "" }));
        }
    }

    fn select(&mut self, id: String, ctx: &Ctx) {
        self.current = Some(id.clone());
        self.nav.reset();
        self.filter.clear();
        if !self.data.contains_key(&id) {
            self.load(ctx, false);
        }
    }

    fn step(&mut self, by: isize, ctx: &Ctx) {
        let Some(ms) = &self.managers else { return };
        if ms.is_empty() {
            return;
        }
        let i = ms.iter().position(|m| m["id"].as_str() == self.current.as_deref()).unwrap_or(0) as isize;
        let n = ms.len() as isize;
        let id = s(&ms[((i + by).rem_euclid(n)) as usize]["id"], "").to_string();
        self.select(id, ctx);
    }
}

impl Screen for Packages {
    fn title(&self) -> String {
        "Packages".into()
    }

    fn on_show(&mut self, ctx: &mut Ctx) {
        if self.managers.is_none() {
            ctx.get("managers", "/system/packages");
        }
    }

    fn actions(&self) -> Vec<Action> {
        if self.view.is_some() {
            return vec![];
        }
        let mut a = vec![
            Action::new("m", "Package manager…"),
            Action::new("]", "Next manager"),
            Action::new("[", "Previous manager"),
            Action::new("/", "Filter packages"),
            Action::new("r", "Refresh (check again)"),
        ];
        if self.cur().is_some_and(|d| d["updates"].as_array().is_some_and(|u| !u.is_empty())) {
            a.push(Action::new("u", "All updates"));
        }
        if self.askable() && self.selected().is_some() {
            a.push(Action::new("i", "Package info"));
            a.push(Action::new("a", "Ask about this package"));
        }
        a
    }

    fn on_action(&mut self, key: &str, ctx: &mut Ctx) {
        match key {
            "m" => {
                let items = self
                    .managers
                    .iter()
                    .flatten()
                    .map(|m| (s(&m["id"], "").to_string(), format!("{} ({})", s(&m["label"], ""), s(&m["kind"], ""))))
                    .collect();
                ctx.pick("manager", "Package manager", items);
            }
            "]" => self.step(1, ctx),
            "[" => self.step(-1, ctx),
            "/" => ctx.prompt("filter", "Filter packages", self.filter.clone()),
            "r" => self.load(ctx, true),
            "u" => {
                if let Some(d) = self.cur() {
                    let text = d["updates"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|p| format!("{}  {} → {}", s(&p["name"], ""), s(&p["version"], ""), s(&p["newVersion"], "")))
                        .collect::<Vec<_>>()
                        .join("\n");
                    let mut v = TextView::new(format!("Updates · {}", s(&d["label"], "")), false);
                    v.set(format!("{}\n\n{text}", s(&d["updatesNote"], "")));
                    self.view = Some(v);
                }
            }
            "i" if self.askable() => {
                if let Some(name) = self.selected() {
                    self.view = Some(TextView::new(format!("Package · {name}"), false));
                    ctx.get("info", format!("/system/package-info/{}", enc(&name)));
                }
            }
            "a" if self.askable() => {
                if let Some(name) = self.selected() {
                    self.ask.start(ctx, json!({ "kind": "package", "id": name }), &format!("the package {name}"));
                }
            }
            _ => {}
        }
    }

    fn on_key(&mut self, k: KeyEvent, ctx: &mut Ctx) -> Handled {
        if let Some(v) = &mut self.view {
            return match v.key(&k) {
                Some(false) => {
                    self.view = None;
                    Handled::Yes
                }
                Some(true) => Handled::Yes,
                None => Handled::No,
            };
        }
        if k.code == KeyCode::Enter {
            if self.askable() {
                self.on_action("i", ctx);
            }
            return Handled::Yes;
        }
        let n = self.list().len();
        if self.nav.key(&k, n) { Handled::Yes } else { Handled::No }
    }

    fn on_task(&mut self, key: &str, result: anyhow::Result<Value>, ctx: &mut Ctx) {
        if self.ask.on_task(key, &result, ctx) {
            return;
        }
        match key {
            "managers" => match result {
                Ok(v) => {
                    let ms = v["managers"].as_array().cloned().unwrap_or_default();
                    let first = ms.first().map(|m| s(&m["id"], "").to_string());
                    self.managers = Some(ms);
                    if let Some(id) = first {
                        self.select(id, ctx);
                    }
                }
                Err(e) => self.error = Some(e.to_string()),
            },
            "manager" => {
                if let Ok(Value::String(id)) = result {
                    self.select(id, ctx);
                }
            }
            "filter" => {
                if let Ok(Value::String(f)) = result {
                    self.filter = f;
                    self.nav.reset();
                }
            }
            "info" => {
                if let Some(v) = &mut self.view {
                    match result {
                        Ok(r) => v.set(r["info"].as_str().unwrap_or_default().to_string()),
                        Err(e) => v.set(e.to_string()),
                    }
                }
            }
            k if k.starts_with("pkgs:") => {
                let m = k[5..].to_string();
                if self.loading.as_deref() == Some(m.as_str()) {
                    self.loading = None;
                }
                match result {
                    Ok(v) => {
                        self.data.insert(m, v);
                    }
                    Err(e) => {
                        if self.current.as_deref() == Some(m.as_str()) {
                            self.error = Some(e.to_string());
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn draw(&mut self, f: &mut Frame, area: Rect, _ctx: &Ctx) {
        let t = Theme::get();
        if let Some(v) = &mut self.view {
            v.draw(f, area, "Esc close · j k scroll");
            return;
        }
        match &self.managers {
            None => {
                let text = self.error.clone().unwrap_or_else(|| "Loading…".into());
                f.render_widget(Paragraph::new(text).block(block("Packages", false)), area);
                return;
            }
            Some(ms) if ms.is_empty() => {
                f.render_widget(Paragraph::new("No package managers found on this server").block(block("Packages", false)), area);
                return;
            }
            _ => {}
        }
        // the manager bar
        let mut spans = vec![Span::styled(" managers: ", t.muted_style())];
        for m in self.managers.iter().flatten() {
            let on = m["id"].as_str() == self.current.as_deref();
            spans.push(Span::styled(format!(" {} ", s(&m["label"], "")), if on { t.key_style() } else { t.muted_style() }));
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(" [ ] switch · m pick", t.muted_style()));
        let loading = self.loading.is_some() && self.loading == self.current;
        let Some(d) = self.cur().cloned() else {
            let [bar, body] = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(area);
            f.render_widget(Paragraph::new(Line::from(spans)), bar);
            let text = match &self.error {
                Some(e) => e.clone(),
                None => "Reading the package list (update checks can take a while)…".into(),
            };
            f.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }).block(block("Packages", false)), body);
            return;
        };
        let updates = d["updates"].as_array().cloned();
        let up_rows = match &updates {
            Some(u) if !u.is_empty() => (u.len() as u16).min(6) + 3,
            _ => 4,
        };
        let [bar, info, ups, body] =
            Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Length(up_rows), Constraint::Min(3)]).areas(area);
        f.render_widget(Paragraph::new(Line::from(spans)), bar);
        let mut line = vec![Span::styled(
            format!(" {} installed · checked {}", d["installed"].as_u64().unwrap_or(0), since(s(&d["checkedAt"], ""))),
            t.muted_style(),
        )];
        if loading {
            line.push(Span::styled("   checking…", t.accent_style()));
        }
        if let Some(e) = &self.error {
            line.push(Span::styled(format!("   {e}"), Style::default().fg(t.danger)));
        }
        f.render_widget(Paragraph::new(Line::from(line)), info);

        // updates
        let title = match &updates {
            Some(u) => format!("Updates ({})", u.len()),
            None => "Updates".into(),
        };
        let mut lines = vec![Line::styled(s(&d["updatesNote"], "").to_string(), t.muted_style())];
        match &updates {
            Some(u) if u.is_empty() => lines.push(Line::styled("No updates in the last sync", t.muted_style())),
            Some(u) => {
                let room = (up_rows as usize).saturating_sub(3);
                for p in u.iter().take(room) {
                    lines.push(Line::from(vec![
                        Span::raw(format!("{}  ", s(&p["name"], ""))),
                        Span::styled(format!("{} → ", s(&p["version"], "")), t.muted_style()),
                        Span::styled(s(&p["newVersion"], "").to_string(), Style::default().fg(t.ok)),
                    ]));
                }
                if u.len() > room {
                    lines.push(Line::styled(format!("… {} more — u shows all", u.len() - room), t.muted_style()));
                }
            }
            None => {}
        }
        f.render_widget(Paragraph::new(lines).block(block(title, false)), ups);

        // installed
        let list = self.list();
        let title = if d["listTitle"] == "explicit" {
            format!("Explicitly installed ({})", list.len())
        } else {
            format!("Installed ({})", list.len())
        };
        let title = if self.filter.is_empty() { title } else { format!("{title} matching “{}”", self.filter) };
        let hint = if self.askable() { " — Enter info · a ask · / filter" } else { " — / filter" };
        let w = body.width.saturating_sub(4) as usize;
        let items: Vec<ListItem> = list
            .iter()
            .map(|p| {
                let name = s(&p["name"], "");
                let note = p["note"].as_str().map(|n| format!("  {n}")).unwrap_or_default();
                let ver = s(&p["version"], "");
                let pad = w.saturating_sub(name.chars().count() + note.chars().count() + ver.chars().count() + 1);
                ListItem::new(Line::from(vec![
                    Span::raw(format!(" {name}")),
                    Span::styled(note, t.muted_style()),
                    Span::raw(" ".repeat(pad)),
                    Span::styled(ver.to_string(), t.muted_style()),
                ]))
            })
            .collect();
        if items.is_empty() {
            f.render_widget(
                Paragraph::new(Line::styled("Nothing here", t.muted_style())).block(block(format!("{title}{hint}"), true)),
                body,
            );
            return;
        }
        self.nav.clamp(items.len());
        f.render_stateful_widget(
            List::new(items).block(block(format!("{title}{hint}"), true)).highlight_style(t.selected()),
            body,
            &mut self.nav.state,
        );
    }
}
