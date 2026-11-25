//! The new-session composer shared by Chats and the Sessions tabs of projects and groups:
//! the first message plus the agent, model and permission mode to start with.

use ratatui::text::Span;
use serde_json::{Value, json};

use super::session::MODES;
use crate::core::{Action, Ctx};
use crate::ui::Theme;
use crate::ui::input::TextInput;

pub struct NewSession {
    pub input: TextInput,
    pub agent: Option<String>,
    pub model: Option<String>,
    pub mode: Option<String>,
}

impl Default for NewSession {
    fn default() -> Self {
        Self { input: TextInput::multiline(), agent: None, model: None, mode: None }
    }
}

impl NewSession {
    /// The chosen agent or the hub default.
    pub fn agent(&self, ctx: &Ctx) -> String {
        self.agent.clone().unwrap_or_else(|| ctx.shared.agents["defaultKind"].as_str().unwrap_or("claude-code").to_string())
    }

    /// The chosen permission mode or the hub default.
    pub fn mode(&self, ctx: &Ctx) -> String {
        self.mode.clone().unwrap_or_else(|| ctx.shared.settings["defaultPermission"].as_str().unwrap_or("ask").to_string())
    }

    /// Keys for agent, model and permission mode.
    pub fn actions() -> Vec<Action> {
        vec![Action::new("A", "Agent"), Action::new("m", "Model"), Action::new("o", "Permission mode")]
    }

    /// Handle one of `actions()`; false when the key is not the composer's.
    pub fn on_action(&mut self, key: &str, ctx: &mut Ctx) -> bool {
        match key {
            "A" => {
                let items = ctx.shared.agents["agents"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|a| {
                        let ok = if a["available"] == true { "" } else { " (unavailable)" };
                        (
                            a["kind"].as_str().unwrap_or_default().to_string(),
                            format!("{} — {}{ok}", a["label"].as_str().unwrap_or_default(), a["detail"].as_str().unwrap_or_default()),
                        )
                    })
                    .collect();
                ctx.pick("agent", "Agent", items);
            }
            "m" => ctx.get("models", format!("/agents/{}/models", self.agent(ctx))),
            "o" => ctx.pick("mode", "Permission mode", MODES.iter().map(|(id, l)| (id.to_string(), l.to_string())).collect()),
            _ => return false,
        }
        true
    }

    /// Answers of the composer's pickers; false when the key is not the composer's.
    pub fn on_task(&mut self, key: &str, v: &Value, ctx: &mut Ctx) -> bool {
        match key {
            "agent" => {
                if let Some(a) = v.as_str() {
                    self.agent = Some(a.to_string());
                    self.model = None;
                }
            }
            "models" => {
                let mut items: Vec<(String, String)> = vec![(String::new(), "Agent default".into())];
                match v["options"].as_array() {
                    Some(opts) => {
                        items.extend(opts.iter().map(|o| {
                            (o["id"].as_str().unwrap_or_default().to_string(), o["label"].as_str().unwrap_or_default().to_string())
                        }))
                    }
                    None => items.extend(
                        v["models"].as_array().into_iter().flatten().filter_map(|m| m.as_str()).map(|m| (m.to_string(), m.to_string())),
                    ),
                }
                ctx.pick("model", "Model", items);
            }
            "model" => {
                if let Some(m) = v.as_str() {
                    self.model = (!m.is_empty()).then(|| m.to_string());
                }
            }
            "mode" => {
                if let Some(m) = v.as_str() {
                    self.mode = Some(m.to_string());
                }
            }
            _ => return false,
        }
        true
    }

    /// The `POST /sessions` body (without the scope); `prompt` may be empty for a session
    /// that waits for its first message.
    pub fn body(&self, ctx: &Ctx, prompt: &str) -> Value {
        let mut agent = json!({ "kind": self.agent(ctx), "permissionMode": self.mode(ctx) });
        if let Some(m) = &self.model {
            agent["model"] = json!(m);
        }
        let mut body = json!({ "agent": agent });
        if !prompt.trim().is_empty() {
            body["prompt"] = json!(prompt);
        }
        body
    }

    /// "A agent  m model  o mode" for the settings line above the composer.
    pub fn settings_spans(&self, ctx: &Ctx) -> Vec<Span<'static>> {
        let t = Theme::get();
        let model = self.model.clone().unwrap_or_else(|| "default model".into());
        vec![
            Span::styled(" A ", t.key_style()),
            Span::raw(format!(" {}  ", self.agent(ctx))),
            Span::styled(" m ", t.key_style()),
            Span::raw(format!(" {model}  ")),
            Span::styled(" o ", t.key_style()),
            Span::raw(format!(" {}  ", self.mode(ctx))),
        ]
    }
}
