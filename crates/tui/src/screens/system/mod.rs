//! System (the web app's System page): server monitoring in sections — Overview, Docker
//! containers / networks / volumes, systemd (or launchd) services and packages — with
//! "Ask about this" for containers, units and packages. Each section loads its own data
//! only when it is opened.

mod docker;
mod overview;
mod packages;
mod services;

use hubness_client::Method;
use serde_json::{Value, json};

use super::kit::Sections;
use super::tabs::Tab;
use crate::core::{Ctx, Route, Screen};

pub fn screen(section: Option<&str>) -> Box<dyn Screen> {
    let tabs = vec![
        Tab::new("overview", "Overview", Box::new(overview::Overview::default())),
        Tab::new("containers", "Containers", Box::new(docker::Containers::default())),
        Tab::new("networks", "Networks", Box::new(docker::Networks::default())),
        Tab::new("volumes", "Volumes", Box::new(docker::Volumes::default())),
        Tab::new("services", "Services", Box::new(services::Services::default())),
        Tab::new("packages", "Packages", Box::new(packages::Packages::default())),
    ];
    // the web app's `docker` section is the containers tab here
    let section = match section {
        Some("docker") => Some("containers"),
        other => other,
    };
    Box::new(Sections::new("System", tabs, section))
}

/// "Ask about this": a question (optional) for the agent, then `POST /api/ask` starts a chat
/// with the object's context (status, logs) and the TUI opens it.
#[derive(Default)]
pub struct Ask {
    target: Option<Value>,
}

impl Ask {
    /// Ask about `target` (`{kind, id, scope?}`), described as `what` in the prompt.
    pub fn start(&mut self, ctx: &mut Ctx, target: Value, what: &str) {
        let agent = ctx.shared.agents["defaultKind"].as_str().unwrap_or("claude-code").to_string();
        self.target = Some(target);
        ctx.prompt("ask:question", format!("Question for {agent} (optional) · {what}"), "");
    }

    /// Handle the prompt answer and the created chat; `true` when the key was the ask's.
    pub fn on_task(&mut self, key: &str, result: &anyhow::Result<Value>, ctx: &mut Ctx) -> bool {
        match key {
            "ask:question" => {
                let target = self.target.take();
                if let (Ok(Value::String(q)), Some(target)) = (result, target) {
                    let agent = ctx.shared.agents["defaultKind"].as_str().unwrap_or("claude-code").to_string();
                    let mut body = json!({ "target": target, "agent": { "kind": agent, "permissionMode": "ask" } });
                    if !q.trim().is_empty() {
                        body["question"] = json!(q.trim());
                    }
                    ctx.info("starting a chat about it…");
                    ctx.send("ask:created", Method::POST, "/ask", body);
                }
                true
            }
            "ask:created" => {
                match result {
                    Ok(s) => {
                        let id = s["id"].as_str().unwrap_or_default().to_string();
                        ctx.shared.sessions.retain(|x| x["id"] != s["id"]);
                        ctx.shared.sessions.insert(0, s.clone());
                        ctx.go(Route::Chat(id));
                    }
                    Err(e) => ctx.error(format!("ask: {e}")),
                }
                true
            }
            _ => false,
        }
    }
}

/// `encodeURIComponent` for a path segment.
pub fn enc(s: &str) -> String {
    super::session::enc(s)
}

/// Whether an error is the hub saying the feature is missing on this server (501).
pub fn unavailable(e: &anyhow::Error) -> bool {
    e.to_string().starts_with("501")
}
