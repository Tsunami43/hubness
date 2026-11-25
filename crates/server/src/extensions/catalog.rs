//! The MCP catalog (as `mcp-catalog.ts`): a search in the official MCP Registry
//! (registry.modelcontextprotocol.io, or `HUBNESS_MCP_REGISTRY`), proxied by the hub and
//! normalized into install options the add form fills in. An empty search shows a small
//! built-in list of popular servers (packages/shared/src/mcp-popular.json, registry format).

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::error::{ApiResult, HttpError};

use super::items::cut;
use super::mcp::{env_name_ok, header_name_ok};

const DEFAULT_REGISTRY: &str = "https://registry.modelcontextprotocol.io";
const PAGE: usize = 30;
const TTL: Duration = Duration::from_secs(5 * 60);
const MAX_CACHE: usize = 50;
const POPULAR: &str = include_str!("../../../../packages/shared/src/mcp-popular.json");

fn registry() -> String {
    let base = std::env::var("HUBNESS_MCP_REGISTRY").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| DEFAULT_REGISTRY.into());
    base.trim_end_matches('/').to_string()
}

fn s(v: &Value) -> Option<String> {
    v.as_str().filter(|s| !s.is_empty()).map(str::to_string)
}

fn list(v: &Value) -> Vec<&Value> {
    v.as_array().map(|a| a.iter().filter(|x| x.is_object()).collect()).unwrap_or_default()
}

fn vars(v: &Value, ok: fn(&str) -> bool) -> Vec<Value> {
    list(v)
        .into_iter()
        .filter(|x| x["name"].as_str().is_some_and(ok))
        .take(50)
        .map(|x| {
            json!({
                "name": x["name"],
                "description": cut(&s(&x["description"]).unwrap_or_default(), 500),
                "required": x["isRequired"] == true,
                "secret": x["isSecret"] == true,
                "value": s(&x["value"]).or_else(|| s(&x["default"])),
            })
        })
        .collect()
}

/// Registry arguments as argv; a required argument without a value becomes a `<hint>` placeholder.
fn argv(v: &Value) -> Vec<String> {
    let mut out = vec![];
    for a in list(v) {
        let value = s(&a["value"]).or_else(|| s(&a["default"]));
        let required = a["isRequired"] == true;
        if a["type"] == "named" {
            let Some(name) = s(&a["name"]) else { continue };
            if let Some(v) = value {
                out.push(name);
                out.push(v);
            } else if required {
                let hint = s(&a["valueHint"]).unwrap_or_else(|| name.trim_start_matches('-').to_string());
                out.push(name);
                if a["format"] != "boolean" {
                    out.push(format!("<{hint}>"));
                }
            }
        } else if let Some(v) = value {
            out.push(v);
        } else if required {
            out.push(format!("<{}>", s(&a["valueHint"]).or_else(|| s(&a["name"])).unwrap_or_else(|| "value".into())));
        }
    }
    out.truncate(100);
    out
}

fn option(
    kind: &str,
    transport: &str,
    command: Option<String>,
    args: Vec<String>,
    url: Option<String>,
    env: Vec<Value>,
    headers: Vec<Value>,
) -> Value {
    json!({ "kind": kind, "transport": transport, "command": command, "args": args, "url": url, "env": env, "headers": headers })
}

fn package_option(p: &Value) -> Option<Value> {
    if p["transport"]["type"] != "stdio" {
        return None;
    }
    let id = s(&p["identifier"])?;
    let version = s(&p["version"]);
    let env = vars(&p["environmentVariables"], env_name_ok);
    let runtime = argv(&p["runtimeArguments"]);
    let rest = argv(&p["packageArguments"]);
    match p["registryType"].as_str() {
        Some("npm") => {
            let command = s(&p["runtimeHint"]).unwrap_or_else(|| "npx".into());
            let mut args = if !runtime.is_empty() {
                runtime
            } else if command == "npx" {
                vec!["-y".into()]
            } else {
                vec![]
            };
            args.push(match &version {
                Some(v) => format!("{id}@{v}"),
                None => id,
            });
            args.extend(rest);
            Some(option("npm", "stdio", Some(command), args, None, env, vec![]))
        }
        Some("pypi") => {
            let command = s(&p["runtimeHint"]).unwrap_or_else(|| "uvx".into());
            let mut args = runtime;
            args.push(match &version {
                Some(v) => format!("{id}@{v}"),
                None => id,
            });
            args.extend(rest);
            Some(option("pypi", "stdio", Some(command), args, None, env, vec![]))
        }
        Some("oci") => {
            let command = s(&p["runtimeHint"]).unwrap_or_else(|| "docker".into());
            let last = id.rsplit('/').next().unwrap_or_default();
            let image = match &version {
                Some(v) if !last.contains(':') && !id.contains('@') => format!("{id}:{v}"),
                _ => id.clone(),
            };
            let mut args = vec!["run".to_string()];
            args.extend(if runtime.is_empty() { vec!["-i".into(), "--rm".into()] } else { runtime });
            for e in &env {
                args.push("-e".into());
                args.push(e["name"].as_str().unwrap_or_default().to_string());
            }
            args.push(image);
            args.extend(rest);
            Some(option("docker", "stdio", Some(command), args, None, env, vec![]))
        }
        _ => None,
    }
}

fn remote_option(r: &Value) -> Option<Value> {
    let transport = match r["type"].as_str() {
        Some("streamable-http") => "http",
        Some("sse") => "sse",
        _ => return None,
    };
    let url = s(&r["url"]).filter(|u| u.starts_with("http://") || u.starts_with("https://"))?;
    Some(option("remote", transport, None, vec![], Some(url), vec![], vars(&r["headers"], header_name_ok)))
}

const PREFIXES: [&str; 3] = ["mcp-server-", "server-", "mcp-"];
const SUFFIXES: [&str; 3] = ["-mcp-server", "-server", "-mcp"];

/// A short config name from a registry id: io.github.upstash/context7 → context7.
pub(super) fn suggested_name(id: &str) -> String {
    let last = id.rsplit('/').next().unwrap_or_default().to_lowercase();
    let mut out = String::new();
    let mut dash = false;
    for c in last.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-' {
            out.push(c);
            dash = false;
        } else if !dash {
            out.push('-');
            dash = true;
        }
    }
    let mut s = out.trim_matches('-').to_string();
    loop {
        let mut changed = false;
        if let Some(p) = PREFIXES.iter().find(|p| s.starts_with(**p) && s.len() > p.len()) {
            s = s[p.len()..].to_string();
            changed = true;
        }
        if let Some(x) = SUFFIXES.iter().find(|x| s.ends_with(**x) && s.len() > x.len()) {
            s.truncate(s.len() - x.len());
            changed = true;
        }
        if !changed {
            break;
        }
    }
    let s: String = s.chars().take(64).collect();
    if s.is_empty() { "server".into() } else { s }
}

/// One registry record (`{server, _meta}` or a bare server.json) as a catalog entry.
fn entry(rec: &Value) -> Option<Value> {
    let srv = if rec["server"].is_object() { &rec["server"] } else { rec };
    if !srv.is_object() {
        return None;
    }
    let id = s(&srv["name"])?;
    let mut options: Vec<Value> = list(&srv["packages"]).into_iter().filter_map(package_option).collect();
    options.extend(list(&srv["remotes"]).into_iter().filter_map(remote_option));
    options.truncate(10);
    let short = id.rsplit('/').next().unwrap_or_default().to_string();
    Some(json!({
        "id": cut(&id, 200),
        "title": cut(&s(&srv["title"]).unwrap_or(short), 200),
        "description": cut(&s(&srv["description"]).unwrap_or_default(), 1000),
        "version": s(&srv["version"]),
        "repository": s(&srv["repository"]["url"]),
        "website": s(&srv["websiteUrl"]),
        "suggestedName": suggested_name(&id),
        "options": options,
    }))
}

fn entries(records: &[Value]) -> Vec<Value> {
    let mut seen = std::collections::HashSet::new();
    records.iter().filter_map(entry).filter(|e| seen.insert(e["id"].as_str().unwrap_or_default().to_string())).collect()
}

fn popular() -> &'static Vec<Value> {
    static P: OnceLock<Vec<Value>> = OnceLock::new();
    P.get_or_init(|| entries(serde_json::from_str::<Vec<Value>>(POPULAR).as_deref().unwrap_or_default()))
}

fn http() -> &'static reqwest::Client {
    static C: OnceLock<reqwest::Client> = OnceLock::new();
    C.get_or_init(|| reqwest::Client::builder().timeout(Duration::from_secs(10)).user_agent("hubness").build().expect("http client"))
}

fn cache() -> &'static Mutex<HashMap<String, (Instant, Value)>> {
    static C: OnceLock<Mutex<HashMap<String, (Instant, Value)>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

pub(super) async fn mcp_catalog(q: Option<&str>, cursor: Option<&str>) -> ApiResult<Value> {
    let query = q.unwrap_or_default().trim().to_string();
    let after = cursor.unwrap_or_default().trim().to_string();
    if query.chars().count() > 200 {
        return Err(HttpError::bad_request("q: at most 200 characters"));
    }
    if after.chars().count() > 500 {
        return Err(HttpError::bad_request("cursor: at most 500 characters"));
    }
    if query.is_empty() && after.is_empty() {
        return Ok(json!({ "entries": popular(), "nextCursor": null, "source": "popular" }));
    }
    let base = registry();
    let key = format!("{base}\n{query}\n{after}");
    if let Some((at, v)) = cache().lock().expect("cache").get(&key)
        && at.elapsed() < TTL
    {
        return Ok(v.clone());
    }
    let mut params: Vec<(&str, String)> = vec![("limit", PAGE.to_string()), ("version", "latest".into())];
    if !query.is_empty() {
        params.push(("search", query));
    }
    if !after.is_empty() {
        params.push(("cursor", after));
    }
    const QUERY: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC.remove(b'-').remove(b'_').remove(b'.').remove(b'~');
    let qs: Vec<String> = params.iter().map(|(k, v)| format!("{k}={}", percent_encoding::utf8_percent_encode(v, QUERY))).collect();
    let res = http()
        .get(format!("{base}/v0/servers?{}", qs.join("&")))
        .header("accept", "application/json")
        .send()
        .await
        .map_err(|_| HttpError::new(502, "The MCP Registry is unreachable — check the internet connection or try again later"))?;
    if !res.status().is_success() {
        return Err(HttpError::new(502, format!("The MCP Registry answered {} — try again later", res.status().as_u16())));
    }
    let unreadable = || HttpError::new(502, "The MCP Registry sent an unreadable answer");
    let body: Value = res.json().await.map_err(|_| unreadable())?;
    let Some(servers) = body["servers"].as_array() else { return Err(unreadable()) };
    let value = json!({ "entries": entries(servers), "nextCursor": s(&body["metadata"]["nextCursor"]), "source": "registry" });
    let mut c = cache().lock().expect("cache");
    if c.len() >= MAX_CACHE
        && let Some(oldest) = c.iter().min_by_key(|(_, (at, _))| *at).map(|(k, _)| k.clone())
    {
        c.remove(&oldest);
    }
    c.insert(key, (Instant::now(), value.clone()));
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggested_names() {
        assert_eq!(suggested_name("io.github.upstash/context7"), "context7");
        assert_eq!(suggested_name("io.github.github/github-mcp-server"), "github");
        assert_eq!(suggested_name("x/mcp-server-py"), "py");
        assert_eq!(suggested_name("a/mcp"), "mcp");
        assert_eq!(suggested_name("a/Foo Bar.baz"), "foo-bar-baz");
        assert_eq!(suggested_name("a/..."), "server");
        assert_eq!(popular().len(), 7);
    }
}
