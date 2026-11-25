//! The Secrets tab of a project (the web app's SecretsPanel): secret names (values are never
//! shown), add / replace with a hidden input, delete, unreadable ones and the key source.

use crossterm::event::{KeyCode, KeyEvent};
use hubness_client::Method;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph, Wrap};
use serde_json::{Value, json};

use crate::core::{Action, Ctx, Handled, Screen};
use crate::screens::session::enc;
use crate::ui::{Theme, ago, block};

/// A secret name as the web app normalizes it: letters, digits and `_`.
pub fn secret_name(s: &str) -> String {
    s.trim().chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).collect()
}

pub struct SecretsTab {
    project: String,
    data: Value,
    list: ListState,
    /// The name whose value is being asked for.
    naming: Option<String>,
}

impl SecretsTab {
    pub fn new(project: &str) -> Self {
        Self { project: project.to_string(), data: Value::Null, list: ListState::default().with_selected(Some(0)), naming: None }
    }

    fn secrets(&self) -> Vec<Value> {
        self.data["secrets"].as_array().cloned().unwrap_or_default()
    }

    fn selected(&self) -> Option<Value> {
        self.list.selected().and_then(|i| self.secrets().get(i).cloned())
    }

    fn ask_value(&mut self, name: String, ctx: &mut Ctx) {
        let exists = self.secrets().iter().any(|s| s["name"] == name.as_str());
        ctx.prompt_secret("value", format!("{} {name} — value (hidden)", if exists { "Update" } else { "Add" }));
        self.naming = Some(name);
    }

    fn reload(&self, ctx: &mut Ctx) {
        ctx.get("secrets", format!("/projects/{}/secrets", self.project));
    }
}

impl Screen for SecretsTab {
    fn title(&self) -> String {
        "Secrets".into()
    }

    fn on_show(&mut self, ctx: &mut Ctx) {
        self.reload(ctx);
    }

    fn actions(&self) -> Vec<Action> {
        vec![
            Action::new("n", "Add a secret"),
            Action::new("e", "Replace the selected secret's value"),
            Action::new("x", "Delete the selected secret"),
            Action::new("r", "Reload"),
        ]
    }

    fn on_action(&mut self, key: &str, ctx: &mut Ctx) {
        match key {
            "n" => ctx.prompt("name", "Secret name (e.g. API_KEY)", ""),
            "e" => {
                if let Some(s) = self.selected() {
                    self.ask_value(s["name"].as_str().unwrap_or_default().to_string(), ctx);
                }
            }
            "x" => {
                if let Some(s) = self.selected() {
                    let name = s["name"].as_str().unwrap_or_default();
                    ctx.confirm(&format!("delete:{name}"), "Delete the secret", format!("Delete {name}?"), true);
                }
            }
            "r" => self.reload(ctx),
            _ => {}
        }
    }

    fn on_key(&mut self, k: KeyEvent, ctx: &mut Ctx) -> Handled {
        let n = self.secrets().len();
        match k.code {
            KeyCode::Down | KeyCode::Char('j') if n > 0 => self.list.select(Some((self.list.selected().unwrap_or(0) + 1).min(n - 1))),
            KeyCode::Up | KeyCode::Char('k') => self.list.select(Some(self.list.selected().unwrap_or(0).saturating_sub(1))),
            KeyCode::Enter => self.on_action(if n > 0 { "e" } else { "n" }, ctx),
            _ => return Handled::No,
        }
        Handled::Yes
    }

    fn on_task(&mut self, key: &str, result: anyhow::Result<Value>, ctx: &mut Ctx) {
        let Some(v) = ctx.ok(result) else { return };
        match key {
            "secrets" => {
                self.data = v;
                let n = self.secrets().len();
                if n > 0 && self.list.selected().is_none_or(|i| i >= n) {
                    self.list.select(Some(n - 1));
                }
            }
            "name" => {
                let Some(name) = v.as_str().map(secret_name).filter(|s| !s.is_empty()) else { return };
                self.ask_value(name, ctx);
            }
            "value" => {
                let name = self.naming.take();
                let (Some(name), Some(value)) = (name, v.as_str().filter(|s| !s.is_empty())) else { return };
                ctx.send("saved", Method::PUT, format!("/projects/{}/secrets/{}", self.project, enc(&name)), json!({ "value": value }));
            }
            "saved" | "deleted" => {
                ctx.info("secrets saved");
                self.reload(ctx);
            }
            k if k.starts_with("delete:") && v == Value::Bool(true) => {
                ctx.send("deleted", Method::DELETE, format!("/projects/{}/secrets/{}", self.project, enc(&k[7..])), Value::Null);
            }
            _ => {}
        }
    }

    fn draw(&mut self, f: &mut Frame, area: Rect, _ctx: &Ctx) {
        let t = Theme::get();
        let [hint, list, foot] = Layout::vertical([Constraint::Length(2), Constraint::Min(3), Constraint::Length(2)]).areas(area);
        f.render_widget(
            Paragraph::new(Span::styled(
                " Environment variables for Run and Shell (and agents, if allowed in Settings → Environment). Stored encrypted; values are never shown again.",
                t.muted_style(),
            ))
            .wrap(Wrap { trim: true }),
            hint,
        );
        let unreadable: Vec<&str> = self.data["unreadable"].as_array().into_iter().flatten().filter_map(|v| v.as_str()).collect();
        let secrets = self.secrets();
        let title = format!("secrets ({}) — n add · e / Enter replace · x delete", secrets.len());
        if self.data.is_null() || secrets.is_empty() {
            let msg = if self.data.is_null() { "loading…" } else { "No secrets yet — n adds one." };
            f.render_widget(Paragraph::new(Span::styled(msg, t.muted_style())).block(block(title, true)), list);
        } else {
            let items: Vec<ListItem> = secrets
                .iter()
                .map(|s| {
                    let name = s["name"].as_str().unwrap_or_default();
                    let state = if unreadable.contains(&name) {
                        Span::styled("  can’t decrypt — replace it", Style::default().fg(t.danger))
                    } else {
                        Span::styled("  ••••••", t.muted_style())
                    };
                    ListItem::new(Line::from(vec![
                        Span::styled(" ⚿ ", t.accent_style()),
                        Span::raw(name.to_string()),
                        state,
                        Span::styled(format!("   updated {}", ago(s["updatedAt"].as_str().unwrap_or_default())), t.muted_style()),
                    ]))
                })
                .collect();
            f.render_stateful_widget(List::new(items).block(block(title, true)).highlight_style(t.selected()), list, &mut self.list);
        }
        let key = match self.data["keySource"].as_str() {
            Some("env") => " Encryption key: HUBNESS_SECRET_KEY.",
            Some(_) => {
                " Encryption key: generated in the hubness data folder (secret.key). Set HUBNESS_SECRET_KEY to manage it yourself; keep backups of the key apart from the database."
            }
            None => "",
        };
        f.render_widget(Paragraph::new(Span::styled(key, t.muted_style())).wrap(Wrap { trim: true }), foot);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(secret_name(" api-key.v2 "), "api_key_v2");
        assert_eq!(secret_name("TOKEN_1"), "TOKEN_1");
    }
}
