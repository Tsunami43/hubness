//! Grok CLI: version, sign-in, models.

use std::time::Duration;

use serde_json::{Value, json};

use crate::{AppState, agents};

use super::kit::*;

pub(super) const GROK: &str = "grok";

pub(super) fn grok_bin() -> String {
    agents::agent_bin("HUBNESS_GROK_BIN", "grok")
}

pub(super) async fn grok_version() -> Option<String> {
    parse_version(run_cli(&grok_bin(), &["--version"], Duration::from_secs(10), &[]).await.as_deref())
}

pub(super) fn grok_logged_in() -> bool {
    home().join(".grok").join("auth.json").exists()
}

/// Models from the CLI's own cache (~/.grok/models_cache.json), otherwise those declared in
/// the last session.
pub(super) fn grok_models(s: &AppState) -> Vec<String> {
    let cached: Vec<String> = read_json(&home().join(".grok/models_cache.json"))
        .and_then(|d| d["models"].as_object().map(|m| m.keys().cloned().collect()))
        .unwrap_or_default();
    if cached.is_empty() { kv_models(s, GROK) } else { cached }
}

pub(super) async fn grok_harness(s: &AppState) -> Value {
    let installed = grok_version().await;
    let logged_in = installed.is_some() && grok_logged_in();
    let latest = if installed.is_some() {
        cached_latest(GROK, "self", || async {
            let out = run_cli(&grok_bin(), &["update", "--check", "--json"], Duration::from_secs(30), &[]).await;
            let v: Value = serde_json::from_str(out.as_deref().unwrap_or("{}")).unwrap_or_else(|_| json!({}));
            parse_version(v["latestVersion"].as_str())
        })
        .await
    } else {
        None
    };
    let b = shq(&grok_bin());
    let update = format!("{b} update");
    let actions = if installed.is_some() {
        vec![act("update", &update), act("login", &format!("{b} login --device-auth")), act("logout", &format!("{b} logout"))]
    } else {
        vec![act("install", "curl -fsSL https://x.ai/cli/install.sh | bash")]
    };
    let accounts = if installed.is_some() {
        vec![
            json!({ "id": "grok", "label": "grok.com", "detail": if logged_in { Some("session") } else { None }, "loggedIn": logged_in, "active": true }),
        ]
    } else {
        vec![]
    };
    json!({
        "kind": GROK, "label": "Grok CLI", "protocol": "acp", "pinned": false,
        "version": version_info(installed.as_deref(), latest.as_deref(), if installed.is_some() { "grok installer" } else { "—" }, installed.as_ref().map(|_| update.as_str())),
        "actions": actions, "accounts": accounts,
        "auth": { "loggedIn": logged_in, "method": if logged_in { Some("grok.com account") } else { None } },
        "accountMode": "single", "limits": null, "limitsAsOf": null,
        "models": grok_models(s).into_iter().map(|id| json!({ "id": id, "label": id })).collect::<Vec<_>>(),
        "defaultModel": null, "defaultModelEdit": null, "usage": null,
    })
}
