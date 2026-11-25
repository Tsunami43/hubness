//! “Ask about this”: gather the object's context (logs, status, config) and start a regular
//! chat with it (its own Inbox folder, like a chat from the start page). The server-safety
//! rules travel in the first message, not in a special project.

use hubness_core::{AgentConfig, AskTarget, GlobalStreamMsg, PermissionMode, ServiceManager, Session, UnitScope};
use serde_json::{Map, Value};

use super::host::system_platform;
use super::monitor::{container_inspect_summary, container_logs, package_info, unit_logs, unit_status};
use crate::AppState;
use crate::db::sessions as repo;
use crate::error::{ApiResult, HttpError};
use crate::{agents, inbox, runtime};

/// Agent of a new session when none (or an unknown one) is asked for.
const DEFAULT_AGENT_KIND: &str = "claude-code";

fn fence(label: &str, body: &str) -> String {
    let body = body.trim();
    format!("### {label}\n```\n{}\n```", if body.is_empty() { "(empty)" } else { body })
}

struct Context {
    title: String,
    intro: String,
    blocks: Vec<String>,
}

async fn build_context(target: &AskTarget) -> ApiResult<Context> {
    Ok(match target {
        AskTarget::Container { id } => {
            let (inspect, logs) = tokio::join!(container_inspect_summary(id), container_logs(id, 120.0));
            Context {
                title: format!("Container {id}"),
                intro: format!("Look at the Docker container `{id}` on this server."),
                blocks: vec![fence("docker inspect (summary)", &inspect?), fence("docker logs --tail 120", &logs)],
            }
        }
        AskTarget::Unit { id, scope } => {
            let (status, logs) = tokio::join!(unit_status(id, *scope), unit_logs(id, *scope, 120.0));
            let launchd = system_platform().services == Some(ServiceManager::Launchd);
            let scope = if *scope == UnitScope::User { "user" } else { "system" };
            Context {
                title: format!("Unit {id}"),
                intro: if launchd {
                    format!("Look at the launchd agent `{id}` on this macOS server.")
                } else {
                    format!("Look at the {scope} systemd unit `{id}` on this server.")
                },
                blocks: if launchd {
                    vec![fence("launchctl print", &status), fence("logs", &logs)]
                } else {
                    vec![fence("systemctl status", &status), fence("journalctl -n 120", &logs)]
                },
            }
        }
        AskTarget::Package { id } => Context {
            title: format!("Package {id}"),
            intro: format!("Look at the package `{id}` on this server (pacman)."),
            blocks: vec![fence("pacman -Qi + pacman.log", &package_info(id).await)],
        },
    })
}

/// Rules for an agent asked about the server (same spirit as the home project's AGENTS.md).
const ASK_RULES: &str = "You are helping with the server this hub runs on.
- Diagnose first: explain the cause in plain words, then propose a concrete fix.
- Never run destructive or state-changing commands (restart, stop, rm, edit configs, package installs) without asking the user and getting an explicit yes.
- System units and packages need root: show the exact command for the user instead of running it.";

/// A checked `POST /api/ask` body.
pub struct AskRequest {
    pub target: AskTarget,
    pub agent: Option<AgentConfig>,
    pub question: Option<String>,
}

fn type_name(v: Option<&Value>) -> &'static str {
    match v {
        None => "undefined",
        Some(Value::Null) => "null",
        Some(Value::Bool(_)) => "boolean",
        Some(Value::Number(_)) => "number",
        Some(Value::String(_)) => "string",
        Some(Value::Array(_)) => "array",
        Some(Value::Object(_)) => "object",
    }
}

fn bad(msg: impl Into<String>) -> HttpError {
    HttpError::bad_request(msg)
}

/// An optional string field with a length limit (UTF-16 units, as zod counts).
fn opt_string(o: &Map<String, Value>, key: &str, path: &str, max: usize) -> ApiResult<Option<String>> {
    match o.get(key) {
        None => Ok(None),
        Some(Value::String(s)) if s.encode_utf16().count() > max => {
            Err(bad(format!("{path}: Too big: expected string to have <={max} characters")))
        }
        Some(Value::String(s)) => Ok(Some(s.clone())),
        v => Err(bad(format!("{path}: Invalid input: expected string, received {}", type_name(v)))),
    }
}

/// Check the body like the TS zod schema (same first-issue messages).
pub fn parse_request(body: &Value) -> ApiResult<AskRequest> {
    let Some(o) = body.as_object() else { return Err(bad("Invalid input: expected object")) };
    let union = "target: container | unit | package";
    let Some(t) = o.get("target").and_then(Value::as_object) else { return Err(bad(format!("target: {union}"))) };
    let kind = t.get("kind").and_then(Value::as_str).unwrap_or_default();
    if !matches!(kind, "container" | "unit" | "package") {
        return Err(bad(format!("target.kind: {union}")));
    }
    let id = match t.get("id") {
        Some(Value::String(s)) if s.is_empty() => return Err(bad("target.id: Too small: expected string to have >=1 characters")),
        Some(Value::String(s)) => s.clone(),
        v => return Err(bad(format!("target.id: Invalid input: expected string, received {}", type_name(v)))),
    };
    let target = match kind {
        "container" => AskTarget::Container { id },
        "package" => AskTarget::Package { id },
        _ => {
            let scope = match t.get("scope").and_then(Value::as_str) {
                Some("system") => UnitScope::System,
                Some("user") => UnitScope::User,
                _ => return Err(bad("target.scope: Invalid option: expected one of \"system\"|\"user\"")),
            };
            AskTarget::Unit { id, scope }
        }
    };
    let agent = match o.get("agent") {
        None => None,
        Some(Value::Object(a)) => {
            let kind = match a.get("kind") {
                Some(Value::String(s)) if s.is_empty() => return Err(bad("agent.kind: Too small: expected string to have >=1 characters")),
                Some(Value::String(s)) if s.encode_utf16().count() > 64 => {
                    return Err(bad("agent.kind: Too big: expected string to have <=64 characters"));
                }
                Some(Value::String(s)) => s.clone(),
                v => return Err(bad(format!("agent.kind: Invalid input: expected string, received {}", type_name(v)))),
            };
            let model = opt_string(a, "model", "agent.model", 200)?;
            let permission_mode = match a.get("permissionMode") {
                None => None,
                Some(v) => Some(serde_json::from_value::<PermissionMode>(v.clone()).map_err(|_| {
                    bad("agent.permissionMode: Invalid option: expected one of \"ask\"|\"acceptEdits\"|\"bypass\"|\"plan\"")
                })?),
            };
            let account = opt_string(a, "account", "agent.account", 120)?;
            Some(AgentConfig { kind, model, permission_mode, account })
        }
        v => return Err(bad(format!("agent: Invalid input: expected object, received {}", type_name(v)))),
    };
    let question = opt_string(o, "question", "question", 100_000)?;
    Ok(AskRequest { target, agent, question })
}

/// Start the chat: a regular one (no project or group, its own Inbox folder).
pub async fn ask_about(state: &AppState, req: AskRequest) -> ApiResult<Session> {
    let ctx = build_context(&req.target).await?;
    let a = req.agent.as_ref();
    let agent = AgentConfig {
        kind: a.map(|a| a.kind.as_str()).filter(|k| agents::is_known(k)).unwrap_or(DEFAULT_AGENT_KIND).to_string(),
        model: a.and_then(|a| a.model.clone()),
        permission_mode: Some(a.and_then(|a| a.permission_mode).unwrap_or(PermissionMode::Ask)),
        account: None,
    };
    let question = req.question.as_deref().map(str::trim).filter(|q| !q.is_empty()).map(|q| format!("User question: {q}"));
    let mut parts = vec![ctx.intro];
    parts.extend(question);
    parts.push(ASK_RULES.to_string());
    parts.extend(ctx.blocks);
    let prompt = parts.join("\n\n");
    let session = {
        let db = state.db()?;
        let id = crate::db::new_id();
        let cwd = inbox::inbox_dir_for(state.config(), &id)?;
        repo::create(
            &db,
            repo::New {
                id: Some(id),
                project_id: None,
                group_id: None,
                workspace_id: None,
                title: &ctx.title,
                agent: &agent,
                cwd: Some(&cwd),
            },
        )?
    };
    state.bus.emit_global(GlobalStreamMsg::Session { session: session.clone() });
    runtime::send_message(state, &session.id, &prompt, vec![])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn err(v: Value) -> String {
        parse_request(&v).err().expect("an error").message
    }

    #[test]
    fn body_checks_match_the_ts_messages() {
        assert_eq!(err(json!({})), "target: target: container | unit | package");
        assert_eq!(err(json!({"target": {"kind": "x"}})), "target.kind: target: container | unit | package");
        assert_eq!(err(json!({"target": {"kind": "package", "id": ""}})), "target.id: Too small: expected string to have >=1 characters");
        assert_eq!(
            err(json!({"target": {"kind": "unit", "id": "a"}})),
            "target.scope: Invalid option: expected one of \"system\"|\"user\""
        );
        assert_eq!(
            err(json!({"target": {"kind": "package", "id": "a"}, "agent": {}})),
            "agent.kind: Invalid input: expected string, received undefined"
        );
        assert_eq!(
            err(json!({"target": {"kind": "package", "id": "a"}, "question": 5})),
            "question: Invalid input: expected string, received number"
        );
        let ok = parse_request(&json!({"target": {"kind": "unit", "id": "a.service", "scope": "user"}, "question": "why?"})).unwrap();
        assert_eq!(ok.target, AskTarget::Unit { id: "a.service".into(), scope: UnitScope::User });
        assert_eq!(fence("x", "  "), "### x\n```\n(empty)\n```");
    }
}
