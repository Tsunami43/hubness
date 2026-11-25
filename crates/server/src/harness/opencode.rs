//! OpenCode: version and install source, provider accounts, models, default model, stats.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::agents;
use crate::error::{ApiResult, HttpError};

use super::kit::*;

pub(super) const OPENCODE: &str = "opencode";

pub(super) fn opencode_bin() -> String {
    agents::agent_bin("HUBNESS_OPENCODE_BIN", "opencode")
}

pub(super) fn oc_config_dir() -> PathBuf {
    home().join(".config").join("opencode")
}

pub(super) async fn opencode_version() -> Option<String> {
    run_cli(&opencode_bin(), &["--version"], Duration::from_secs(10), &[]).await.map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

pub(super) struct OcProvider {
    id: String,
    name: String,
    connections: Vec<Value>,
}

pub(super) async fn opencode_providers() -> Vec<OcProvider> {
    let out = run_cli(&opencode_bin(), &["auth", "list", "--format", "json"], Duration::from_secs(15), &[]).await;
    let Ok(Value::Array(list)) = serde_json::from_str::<Value>(out.as_deref().unwrap_or("[]")) else { return vec![] };
    let str_of = |v: &Value| v.as_str().map(String::from).unwrap_or_else(|| v.to_string());
    list.iter()
        .map(|p| OcProvider {
            id: str_of(&p["id"]),
            name: if p["name"].is_null() { str_of(&p["id"]) } else { str_of(&p["name"]) },
            connections: p["connections"]
                .as_array()
                .map(|cs| {
                    cs.iter()
                        .map(|c| {
                            let label = [&c["label"], &c["method"], &c["id"]].into_iter().find(|v| !v.is_null()).map(str_of).unwrap_or_default();
                            let mut o = json!({ "id": str_of(&c["id"]), "label": label, "method": if c["method"].is_null() { String::new() } else { str_of(&c["method"]) } });
                            if let Some(a) = c["active"].as_bool() {
                                o["active"] = json!(a);
                            }
                            o
                        })
                        .collect()
                })
                .unwrap_or_default(),
        })
        .collect()
}

pub(super) type ModelsCache = Mutex<Option<(Instant, Vec<String>)>>;

/// Models available in opencode (`opencode models`), cached for 10 minutes; with parallel
/// calls opencode sometimes returns an empty list: retry, and don't cache empty results.
pub(super) async fn opencode_models() -> Vec<String> {
    static CACHE: OnceLock<ModelsCache> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some((at, m)) = cache.lock().expect("models cache").as_ref()
        && at.elapsed() < Duration::from_secs(600)
    {
        return m.clone();
    }
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(r"^[\w.~-]+/\S+$").expect("valid"));
    for _ in 0..3 {
        let Some(out) = run_cli(&opencode_bin(), &["models"], Duration::from_secs(30), &[]).await else { break };
        let models: Vec<String> = out.lines().map(str::trim).filter(|l| re.is_match(l)).map(String::from).collect();
        if !models.is_empty() {
            *cache.lock().expect("models cache") = Some((Instant::now(), models.clone()));
            return models;
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
    cache.lock().expect("models cache").as_ref().map(|(_, m)| m.clone()).unwrap_or_default()
}

/// Models of connected providers only: the others won't run anyway.
pub(super) async fn connected_models(providers: Option<&[OcProvider]>) -> Vec<String> {
    let owned;
    let list = match providers {
        Some(p) => p,
        None => {
            owned = opencode_providers().await;
            &owned
        }
    };
    let all = opencode_models().await;
    if list.is_empty() {
        return all;
    }
    let mut ids: Vec<String> = list.iter().filter(|p| !p.connections.is_empty()).map(|p| p.id.clone()).collect();
    if let Some(cfg) = read_json(&oc_config_dir().join("opencode.json"))
        && let Some(prov) = cfg["provider"].as_object()
    {
        ids.extend(prov.keys().cloned());
    }
    all.into_iter().filter(|m| ids.iter().any(|i| m.split('/').next() == Some(i))).collect()
}

pub(super) fn opencode_default_model() -> Option<String> {
    for f in ["opencode.json", "opencode.jsonc"] {
        if let Some(cfg) = read_json(&oc_config_dir().join(f))
            && let Some(m) = cfg.get("model").filter(|m| !m.is_null() && *m != "")
        {
            return Some(m.as_str().map(String::from).unwrap_or_else(|| m.to_string()));
        }
    }
    let state = read_json(&home().join(".local/state/opencode/model.json"))?;
    let r = &state["recent"][0];
    (!r.is_null()).then(|| format!("{}/{}", r["providerID"].as_str().unwrap_or_default(), r["modelID"].as_str().unwrap_or_default()))
}

pub(super) fn set_opencode_default_model(model: Option<&str>) -> ApiResult<()> {
    if let Some(m) = model {
        let ok = m.split_once('/').is_some_and(|(p, rest)| {
            !p.is_empty()
                && p.chars().all(|c| c.is_alphanumeric() || "_.~-".contains(c))
                && !rest.is_empty()
                && !rest.chars().any(char::is_whitespace)
        });
        if !ok {
            return Err(HttpError::bad_request("Model must look like provider/model"));
        }
    }
    let init = json!({ "$schema": "https://opencode.ai/config.json" });
    write_json_key(&oc_config_dir().join("opencode.json"), "model", model, Some(init), "opencode.json")
}

pub(super) fn parse_stats(out: Option<&str>) -> Value {
    let Some(out) = out else { return Value::Null };
    let clean = strip_ansi(out);
    let num = |re: &str| regex::Regex::new(re).expect("valid").captures(&clean).and_then(|c| c[1].replace(',', "").parse::<f64>().ok());
    let tokens = regex::Regex::new(r"([\d.,]+[kKmMbB]?)\s+tokens").expect("valid").captures(&clean).map(|c| c[1].to_string());
    let sessions = num(r"([\d,]+)\s+sessions?");
    let prompts = num(r"([\d,]+)\s+prompts?");
    if sessions.is_none() && prompts.is_none() && tokens.is_none() {
        return Value::Null;
    }
    let n = |v: Option<f64>| v.map(|f| json!(f as i64)).unwrap_or(Value::Null);
    json!({ "sessions": n(sessions), "prompts": n(prompts), "tokens": tokens })
}

pub(super) async fn opencode_source(real: Option<&Path>) -> (Option<String>, &'static str, bool) {
    // installed by a package manager — check against its repository
    if let Some(r) = real {
        let owner = run_cli("pacman", &["-Qqo", &crate::paths::to_string(r)], Duration::from_secs(5), &[]).await;
        if let Some(pkg) = owner.as_deref().map(str::trim).filter(|o| !o.is_empty()).and_then(|o| o.lines().next()).map(String::from) {
            let latest = cached_latest(OPENCODE, &format!("pacman:{pkg}"), || async {
                let si = run_cli("pacman", &["-Si", &pkg], Duration::from_secs(10), &[]).await?;
                let v = regex::Regex::new(r"(?m)^Version\s*:\s*(\S+)").expect("valid").captures(&si).map(|c| c[1].to_string());
                parse_version(v.as_deref())
            })
            .await;
            return (latest, "pacman", true);
        }
    }
    let latest = cached_latest(OPENCODE, "github", || async {
        let res = http()
            .get("https://api.github.com/repos/anomalyco/opencode/releases?per_page=10")
            .header("Accept", "application/vnd.github+json")
            .send()
            .await
            .ok()?;
        if !res.status().is_success() {
            return None;
        }
        let list: Vec<Value> = res.json().await.ok()?;
        let mut versions: Vec<String> = list.iter().filter_map(|r| parse_version(r["tag_name"].as_str())).collect();
        versions.sort_by(|a, b| cmp_version(a, b));
        versions.pop()
    })
    .await;
    (latest, "GitHub releases", false)
}

/// Update opencode by install method (`opencode upgrade` knows only its own installers; a
/// manually placed binary is replaced with a fresh archive from GitHub releases).
pub(super) fn opencode_upgrade_command(real: Option<&Path>, latest: Option<&str>) -> String {
    let bin = shq(&opencode_bin());
    let v = latest.map(|l| format!(" {l}")).unwrap_or_default();
    let Some(real) = real.map(crate::paths::to_string) else { return format!("{bin} upgrade{v}") };
    for (needle, method) in
        [("/.opencode/bin/", "curl"), ("/.bun/", "bun"), ("/pnpm/", "pnpm"), ("/node_modules/", "npm"), ("brew", "brew")]
    {
        if real.contains(needle) {
            return format!("{bin} upgrade{v} --method {method}");
        }
    }
    let tag = latest.map(|l| format!("download/v{l}")).unwrap_or_else(|| "latest/download".into());
    let arch = if cfg!(target_arch = "aarch64") { "arm64" } else { "x64" };
    let dir = real.rfind('/').map(|i| &real[..i]).filter(|d| !d.is_empty()).unwrap_or("/");
    format!(
        "curl -fsSL https://github.com/anomalyco/opencode/releases/{tag}/opencode-linux-{arch}.tar.gz | tar -xz -C {} opencode",
        shq(dir)
    )
}

pub(super) async fn opencode_harness() -> Value {
    let bin = opencode_bin();
    let real = resolve_bin(&bin);
    let installed = parse_version(run_cli(&bin, &["--version"], Duration::from_secs(20), &[]).await.as_deref());
    let (latest, source, pacman) = opencode_source(real.as_deref()).await;
    let update =
        if pacman { "sudo pacman -Syu opencode".to_string() } else { opencode_upgrade_command(real.as_deref(), latest.as_deref()) };
    let version = version_info(installed.as_deref(), latest.as_deref(), if installed.is_some() { source } else { "—" }, Some(&update));
    let mut info = json!({
        "kind": OPENCODE, "label": "Opencode", "protocol": "acp", "pinned": false, "version": version,
        "auth": null, "accountMode": "providers", "limits": null, "limitsAsOf": null, "defaultModelEdit": "search",
    });
    let o = info.as_object_mut().expect("object");
    if installed.is_none() {
        o.insert(
            "actions".into(),
            json!([
                act("install", "curl -fsSL https://opencode.ai/install | bash"),
                action("install", "sudo pacman -S opencode", false, true)
            ]),
        );
        o.insert("accounts".into(), json!([]));
        o.insert("defaultModel".into(), Value::Null);
        o.insert("models".into(), json!([]));
        o.insert("usage".into(), Value::Null);
        return info;
    }
    // opencode commands run one at a time: in parallel the CLI sometimes returns empty output
    let providers = opencode_providers().await;
    let stats = if providers.is_empty() { None } else { run_cli(&bin, &["stats"], Duration::from_secs(15), &[]).await };
    let models = connected_models(Some(&providers)).await;
    let accounts: Vec<Value> = providers
        .iter()
        .flat_map(|p| {
            let single = p.connections.len() == 1;
            p.connections.iter().map(move |c| {
                json!({
                    "id": c["id"], "label": c["label"],
                    "detail": c["method"].as_str().filter(|m| !m.is_empty()),
                    "loggedIn": true, "active": c["active"].as_bool().unwrap_or(single),
                    "provider": p.id, "providerName": p.name,
                })
            })
        })
        .collect();
    o.insert("actions".into(), json!([action("update", &update, !pacman, true), act("login", &format!("{} auth login", shq(&bin)))]));
    o.insert("accounts".into(), json!(accounts));
    o.insert("defaultModel".into(), json!(opencode_default_model()));
    o.insert("models".into(), json!(models.iter().map(|id| json!({ "id": id, "label": id })).collect::<Vec<_>>()));
    o.insert("usage".into(), parse_stats(stats.as_deref()));
    info
}
