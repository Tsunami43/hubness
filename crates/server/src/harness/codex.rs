//! Codex (via codex-acp): installed adapter and CLI, sign-in method, update commands.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::AppState;

use super::kit::*;

pub(super) const CODEX: &str = "codex";
pub(super) const CODEX_ACP_VERSION: &str = "0.16.0";
pub(super) const CODEX_CLI_VERSION: &str = "0.166.0";
pub(super) const CODEX_ACP_PKG: &str = "@zed-industries/codex-acp";
pub(super) const CODEX_CLI_PKG: &str = "@openai/codex";

pub(super) fn codex_override() -> Option<Vec<String>> {
    std::env::var("HUBNESS_CODEX_ACP").ok().map(|v| v.split_whitespace().map(String::from).collect::<Vec<_>>()).filter(|v| !v.is_empty())
}

/// Version of the npm package a global bin links to (…/node_modules/<pkg>/bin/x.js).
pub(super) fn bin_version(bin: &str, pkg: &str) -> Option<String> {
    let real = resolve_bin(bin)?;
    let mut dir = real.parent()?.to_path_buf();
    for _ in 0..4 {
        if let Some(j) = read_json(&dir.join("package.json"))
            && j["name"] == pkg
        {
            return j["version"].as_str().map(String::from);
        }
        dir = dir.parent()?.to_path_buf();
    }
    None
}

/// Installed (adapter, cli) versions; a custom adapter command counts as installed.
pub(super) fn codex_installed() -> (Option<String>, Option<String>) {
    let acp = if codex_override().is_some() { Some("custom".to_string()) } else { bin_version("codex-acp", CODEX_ACP_PKG) };
    (acp, bin_version("codex", CODEX_CLI_PKG))
}

/// npm -g writes to <node prefix>/lib/node_modules; without write access it needs root.
pub(super) fn npm_global_writable() -> bool {
    let Some(node) = resolve_bin("node") else { return false };
    let Some(prefix) = node.parent().and_then(Path::parent) else { return false };
    let dir = prefix.join("lib").join("node_modules");
    let probe = dir.join(format!(".hubness-write-check-{}", std::process::id()));
    let ok = std::fs::File::create(&probe).is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

pub(super) fn codex_install_command(acp: &str) -> String {
    format!("npm i -g {CODEX_ACP_PKG}@{acp} {CODEX_CLI_PKG}@{CODEX_CLI_VERSION}")
}

pub(super) fn codex_dir() -> PathBuf {
    std::env::var("CODEX_HOME").map(PathBuf::from).unwrap_or_else(|_| home().join(".codex"))
}

/// How Codex is authenticated: ChatGPT subscription (auth.json) or an API key.
pub(super) fn codex_auth() -> Option<&'static str> {
    if codex_dir().join("auth.json").exists() {
        Some("ChatGPT / auth.json")
    } else if env_set("CODEX_API_KEY") {
        Some("CODEX_API_KEY")
    } else if env_set("OPENAI_API_KEY") {
        Some("OPENAI_API_KEY")
    } else {
        None
    }
}
/// The adapter runs sessions, the CLI signs in: both are needed.
pub(super) fn codex_ready() -> (Option<String>, bool, Option<String>) {
    let (acp, cli) = codex_installed();
    let custom = acp.as_deref() == Some("custom");
    let installed = if custom || cli.is_some() { acp.clone() } else { None };
    (acp, custom, installed)
}

pub(super) async fn codex_harness(s: &AppState) -> Value {
    let (_, custom, installed) = codex_ready();
    let writable = npm_global_writable();
    let latest = if installed.is_some() && !custom { cached_latest(CODEX, "npm", || npm_latest(CODEX_ACP_PKG)).await } else { None };
    let update = latest.as_deref().map(codex_install_command);
    let method = installed.as_ref().and_then(|_| codex_auth());
    let version = if custom {
        let cmd = codex_override().unwrap_or_default().join(" ");
        json!({ "installed": null, "latest": null, "status": "unknown", "source": format!("HUBNESS_CODEX_ACP ({cmd})"), "updateCommand": null })
    } else {
        version_info(
            installed.as_deref(),
            latest.as_deref(),
            if installed.is_some() { "npm -g (codex-acp)" } else { "—" },
            update.as_deref(),
        )
    };
    let actions = if installed.is_some() {
        let mut a = vec![];
        if let Some(u) = update.as_deref().filter(|_| !custom) {
            a.push(action("update", u, writable, true));
        }
        a.push(act("login", "codex login --device-auth"));
        a.push(act("logout", "codex logout"));
        a
    } else {
        vec![action("install", &codex_install_command(CODEX_ACP_VERSION), writable, true)]
    };
    let accounts = if installed.is_some() {
        vec![json!({ "id": "codex", "label": "OpenAI", "detail": method, "loggedIn": method.is_some(), "active": true })]
    } else {
        vec![]
    };
    let models: Vec<Value> = kv_models(s, CODEX).into_iter().map(|id| json!({ "id": id, "label": id })).collect();
    json!({
        "kind": CODEX, "label": "Codex", "protocol": "acp", "pinned": true,
        "version": version, "actions": actions, "accounts": accounts,
        "auth": { "loggedIn": method.is_some(), "method": method },
        "accountMode": "single", "limits": null, "limitsAsOf": null,
        "models": models, "defaultModel": null, "defaultModelEdit": null, "usage": null,
    })
}
