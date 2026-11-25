//! Server monitoring: system metrics, Docker, services (systemd or launchd), package
//! details. Only official CLIs and /proc; nothing requires root (system units are
//! read-only with logs; user units/agents can be managed). Linux and macOS.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use hubness_core::{
    ContainerInfo, DiskUsage, DockerNetwork, DockerVolume, HostPlatform, MemoryUsage, ServiceManager, SwapUsage, SystemOverview, UnitInfo,
    UnitScope,
};
use regex::Regex;
use serde::Serialize;
use serde_json::{Map, Value, json};

use super::exec::{CmdResult, sh, sh_timeout};
use super::host::{self, PLATFORM, read, system_platform};
use crate::error::{ApiResult, HttpError};

const LONG: Duration = Duration::from_secs(60);

/// Result of a state-changing command (`{ok, output}`).
#[derive(Debug, Clone, Serialize)]
pub struct ActionResult {
    pub ok: bool,
    pub output: String,
}

impl ActionResult {
    fn from(r: &CmdResult) -> Self {
        Self { ok: r.code == 0, output: r.output() }
    }
    fn or_ok(r: &CmdResult) -> Self {
        let output = r.output();
        Self { ok: r.code == 0, output: if output.is_empty() { "OK".into() } else { output } }
    }
    fn refused(output: &str) -> Self {
        Self { ok: false, output: output.into() }
    }
}

/// The last `n` lines of the trimmed text.
pub fn tail(s: &str, n: usize) -> String {
    let lines: Vec<&str> = s.trim().split('\n').collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// `String(n)` of a JavaScript number, for `--tail`/`-n` arguments taken from the query.
pub fn js_string(n: f64) -> String {
    if n.is_nan() {
        "NaN".into()
    } else if n.is_infinite() {
        if n > 0.0 { "Infinity".into() } else { "-Infinity".into() }
    } else if n.fract() == 0.0 && n.abs() < 1e21 {
        format!("{}", n as i128)
    } else {
        format!("{n}")
    }
}

/// `Number(text ?? 200)` of a `lines` query parameter.
pub fn lines_param(q: Option<&str>) -> f64 {
    match q.map(str::trim) {
        None => 200.0,
        Some("") => 0.0,
        Some(t) => t.parse().unwrap_or(f64::NAN),
    }
}

// ---------- overview ----------

#[derive(Clone, Copy)]
struct CpuSample {
    idle: f64,
    total: f64,
}

static LAST_CPU: Mutex<Option<CpuSample>> = Mutex::new(None);

/// Per-CPU `cpuN` lines of /proc/stat: user nice system idle iowait irq … (in ticks).
fn proc_stat_cpus() -> Vec<Vec<f64>> {
    read("/proc/stat")
        .lines()
        .filter(|l| l.starts_with("cpu") && l.as_bytes().get(3).is_some_and(u8::is_ascii_digit))
        .map(|l| l.split_whitespace().skip(1).map(|v| v.parse().unwrap_or(0.0)).collect())
        .collect()
}

/// Number of CPUs (as Node's `os.cpus().length`).
fn cpu_count() -> u32 {
    let n = proc_stat_cpus().len();
    if n > 0 { n as u32 } else { std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(0) }
}

/// Aggregate CPU times: user + nice + sys + irq + idle, like Node's `os.cpus()` times.
fn cpu_sample() -> Option<CpuSample> {
    let list = proc_stat_cpus();
    if list.is_empty() {
        return None;
    }
    let (mut idle, mut total) = (0.0, 0.0);
    for c in list {
        let at = |i: usize| c.get(i).copied().unwrap_or(0.0);
        idle += at(3);
        total += at(0) + at(1) + at(2) + at(5) + at(3);
    }
    Some(CpuSample { idle, total })
}

fn cpu_percent() -> Option<i64> {
    let cur = cpu_sample()?;
    let prev = LAST_CPU.lock().ok()?.replace(cur);
    let prev = prev?;
    if cur.total == prev.total {
        return None;
    }
    Some((100.0 * (1.0 - (cur.idle - prev.idle) / (cur.total - prev.total))).round() as i64)
}

struct MemFacts {
    total_mb: i64,
    available_mb: i64,
    swap_total_mb: i64,
    swap_used_mb: i64,
}

fn mb(kb: f64) -> i64 {
    (kb / 1024.0).round() as i64
}

fn linux_memory() -> MemFacts {
    static LINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\w+):\s+(\d+)").unwrap());
    let mut m: HashMap<String, f64> = HashMap::new();
    for l in read("/proc/meminfo").lines() {
        if let Some(r) = LINE.captures(l) {
            m.insert(r[1].to_string(), r[2].parse().unwrap_or(0.0));
        }
    }
    let get = |k: &str| m.get(k).copied().unwrap_or(0.0);
    MemFacts {
        total_mb: mb(get("MemTotal")),
        available_mb: mb(get("MemAvailable")),
        swap_total_mb: mb(get("SwapTotal")),
        swap_used_mb: mb(get("SwapTotal") - get("SwapFree")),
    }
}

/// `vm_stat` pages: free + inactive + speculative + purgeable ≈ what Activity Monitor calls available.
pub fn parse_vm_stat(out: &str) -> f64 {
    static PAGE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"page size of (\d+) bytes").unwrap());
    let page: f64 = PAGE.captures(out).and_then(|m| m[1].parse().ok()).unwrap_or(4096.0);
    let pages = |name: &str| {
        Regex::new(&format!(r"Pages {name}:\s+(\d+)"))
            .ok()
            .and_then(|r| r.captures(out))
            .and_then(|m| m[1].parse::<f64>().ok())
            .unwrap_or(0.0)
    };
    (pages("free") + pages("inactive") + pages("speculative") + pages("purgeable")) * page / 1_048_576.0
}

/// `vm.swapusage: total = 2048.00M  used = 1024.50M  free = …` (sysctl -n drops the key).
pub fn parse_swap_usage(out: &str) -> (i64, i64) {
    let val = |k: &str| {
        let Some(m) = Regex::new(&format!(r"{k} = ([\d.]+)([KMG])")).ok().and_then(|r| r.captures(out)) else { return 0.0 };
        let n: f64 = m[1].parse().unwrap_or(0.0);
        n * match &m[2] {
            "K" => 1.0 / 1024.0,
            "G" => 1024.0,
            _ => 1.0,
        }
    };
    (val("total").round() as i64, val("used").round() as i64)
}

async fn darwin_memory() -> MemFacts {
    let (vm, swap, total) = tokio::join!(sh("vm_stat", &[]), sh("sysctl", &["-n", "vm.swapusage"]), sh("sysctl", &["-n", "hw.memsize"]));
    let (swap_total_mb, swap_used_mb) = parse_swap_usage(&swap.out);
    let total: f64 = total.out.trim().parse().unwrap_or(0.0);
    MemFacts {
        total_mb: (total / 1_048_576.0).round() as i64,
        available_mb: if vm.code == 0 { parse_vm_stat(&vm.out).round() as i64 } else { 0 },
        swap_total_mb,
        swap_used_mb,
    }
}

async fn memory() -> MemFacts {
    match PLATFORM {
        HostPlatform::Linux => linux_memory(),
        HostPlatform::Darwin => darwin_memory().await,
        HostPlatform::Other => MemFacts { total_mb: 0, available_mb: 0, swap_total_mb: 0, swap_used_mb: 0 },
    }
}

/// macOS: the sealed system snapshot and its helper volumes are noise; `/` and real volumes stay.
fn darwin_hidden(mount: &str) -> bool {
    (mount.starts_with("/System/Volumes/") && mount != "/System/Volumes/Data") || mount.starts_with("/private/var/vm")
}

pub async fn disks() -> Vec<DiskUsage> {
    let r = if PLATFORM == HostPlatform::Darwin {
        sh("df", &["-P", "-k", "-l"]).await
    } else {
        sh("df", &["-P", "-k", "-x", "tmpfs", "-x", "devtmpfs", "-x", "overlay", "-x", "squashfs", "-x", "efivarfs"]).await
    };
    parse_df(&r.out).into_iter().filter(|d| !(PLATFORM == HostPlatform::Darwin && darwin_hidden(&d.mount))).collect()
}

fn parse_df(out: &str) -> Vec<DiskUsage> {
    let gb = |kb: &str| (kb.parse::<f64>().unwrap_or(f64::NAN) / 1024.0 / 1024.0).round() as i64;
    out.split('\n')
        .skip(1)
        .filter(|l| !l.is_empty())
        .map(|l| l.split_whitespace().collect::<Vec<_>>())
        .filter(|c| c.len() >= 6 && c[0].starts_with('/'))
        .map(|c| DiskUsage {
            device: c[0].to_string(),
            mount: c[5..].join(" "),
            size_gb: gb(c[1]),
            used_gb: gb(c[2]),
            used_percent: c[4].replace('%', "").parse().ok(),
        })
        .collect()
}

fn uptime_sec() -> i64 {
    if let Some(v) = read("/proc/uptime").split_whitespace().next().and_then(|v| v.parse::<f64>().ok()) {
        return v.round() as i64;
    }
    // macOS and BSDs: `{ sec = 1700000000, usec = 0 } …`
    let out = std::process::Command::new("sysctl").args(["-n", "kern.boottime"]).output().map(|o| o.stdout).unwrap_or_default();
    let boot = String::from_utf8_lossy(&out);
    let sec: Option<i64> = boot.split("sec = ").nth(1).and_then(|s| s.split(',').next()).and_then(|s| s.trim().parse().ok());
    sec.map(|s| jiff::Timestamp::now().as_second() - s).unwrap_or(0)
}

fn loadavg() -> [f64; 3] {
    // /proc/loadavg has two decimals, as Node reports them
    let proc: Vec<f64> = read("/proc/loadavg").split_whitespace().take(3).filter_map(|v| v.parse().ok()).collect();
    if let [a, b, c] = proc[..] {
        return [a, b, c];
    }
    let mut l = [0f64; 3];
    // SAFETY: the buffer holds three doubles.
    let n = unsafe { libc::getloadavg(l.as_mut_ptr(), 3) };
    if n < 3 { [0.0; 3] } else { l }
}

/// Host metrics and static facts: Docker and services are asked only on their own pages.
pub async fn overview() -> SystemOverview {
    let (info, mem, d, (gpus, temps)) = tokio::join!(host::host_info(), memory(), disks(), host::sensors());
    // the first CPU sample is the baseline; with frequent requests (UI every 10 s) the percentage is real
    let cpu = match cpu_percent() {
        Some(c) => Some(c),
        None => {
            tokio::time::sleep(Duration::from_millis(300)).await;
            cpu_percent()
        }
    };
    let used_percent = if mem.total_mb != 0 { (100.0 * (1.0 - mem.available_mb as f64 / mem.total_mb as f64)).round() as i64 } else { 0 };
    SystemOverview {
        info,
        hostname: host::hostname(),
        uptime_sec: uptime_sec(),
        load: loadavg(),
        cpus: cpu_count(),
        cpu_percent: cpu,
        gpus,
        temps,
        memory: MemoryUsage { total_mb: mem.total_mb, available_mb: mem.available_mb, used_percent },
        swap: SwapUsage { total_mb: mem.swap_total_mb, used_mb: mem.swap_used_mb },
        disks: d,
    }
}

// ---------- Docker ----------

fn parse_json<T: serde::de::DeserializeOwned>(s: &str) -> ApiResult<T> {
    serde_json::from_str(s).map_err(|e| HttpError::new(500, e.to_string()))
}

fn json_lines(out: &str) -> ApiResult<Vec<Map<String, Value>>> {
    out.split('\n').filter(|l| !l.is_empty()).map(parse_json).collect()
}

fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        v => v.to_string(),
    }
}

fn opt_text(v: &Value) -> Option<String> {
    v.as_str().map(String::from)
}

/// JavaScript truthiness of a JSON value.
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

pub async fn docker_containers() -> ApiResult<Option<Vec<ContainerInfo>>> {
    let ps = sh("docker", &["ps", "-a", "--format", "{{json .}}"]).await;
    if ps.code != 0 {
        return Ok(None);
    }
    let rows = json_lines(&ps.out)?;
    if rows.is_empty() {
        return Ok(Some(vec![]));
    }
    let id = |r: &Map<String, Value>| text(r.get("ID").unwrap_or(&Value::Null));
    let ids: Vec<String> = rows.iter().map(id).collect();
    let mut args = vec!["inspect"];
    args.extend(ids.iter().map(String::as_str));
    let ins = sh("docker", &args).await;
    let mut details: HashMap<String, Value> = HashMap::new();
    if ins.code == 0 {
        for d in parse_json::<Vec<Value>>(&ins.out)? {
            details.insert(text(&d["Id"]).chars().take(12).collect(), d);
        }
    }
    let null = Value::Null;
    Ok(Some(
        rows.iter()
            .map(|r| {
                let rid = id(r);
                let d = details.get(&rid.chars().take(12).collect::<String>()).unwrap_or(&null);
                let field = |k: &str| r.get(k).unwrap_or(&null);
                let mem = d["HostConfig"]["Memory"].as_f64().unwrap_or(0.0);
                let finished = &d["State"]["FinishedAt"];
                ContainerInfo {
                    id: rid.clone(),
                    name: text(field("Names")),
                    image: text(field("Image")),
                    state: d["State"]["Status"].as_str().map(String::from).unwrap_or_else(|| text(field("State"))),
                    status: text(field("Status")),
                    health: opt_text(&d["State"]["Health"]["Status"]),
                    restart_count: d["RestartCount"].as_i64().unwrap_or(0),
                    oom_killed: truthy(&d["State"]["OOMKilled"]),
                    exit_code: d["State"]["ExitCode"].as_i64().unwrap_or(0),
                    ports: text(field("Ports")),
                    mem_limit_mb: if mem != 0.0 { Some((mem / 1024.0 / 1024.0).round() as i64) } else { None },
                    finished_at: if truthy(finished) && !text(finished).starts_with("0001") { Some(text(finished)) } else { None },
                }
            })
            .collect(),
    ))
}

/// Container names and ids as Docker allows them.
static DOCKER_NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_][A-Za-z0-9_.-]*$").unwrap());

fn docker_name(n: &str) -> ApiResult<&str> {
    if DOCKER_NAME.is_match(n) { Ok(n) } else { Err(HttpError::bad_request("Invalid name")) }
}

/// start / stop / restart a container. `--` keeps a name from being read as an option.
pub async fn container_action(name: &str, action: &str) -> ActionResult {
    ActionResult::from(&sh_timeout("docker", &[action, "--", name], LONG).await)
}

/// Remove a container — only a stopped one (never `-f`, volumes stay).
pub async fn container_remove(name: &str) -> ApiResult<ActionResult> {
    let name = docker_name(name)?;
    let ins = sh("docker", &["inspect", "--format", "{{.State.Status}}", name]).await;
    if ins.code != 0 {
        return Err(HttpError::not_found("Container not found"));
    }
    let state = ins.out.trim();
    if matches!(state, "running" | "restarting" | "paused") {
        return Err(HttpError::new(409, "Stop the container first — only stopped containers can be removed"));
    }
    Ok(ActionResult::from(&sh_timeout("docker", &["rm", name], LONG).await))
}

const BUILTIN_NETWORKS: &[&str] = &["bridge", "host", "none"];

/// Roughly `localeCompare`: case-insensitive first, then by code points.
fn locale_cmp(a: &str, b: &str) -> Ordering {
    a.to_lowercase().cmp(&b.to_lowercase()).then_with(|| b.cmp(a))
}

pub async fn docker_networks() -> ApiResult<Option<Vec<DockerNetwork>>> {
    let ls = sh("docker", &["network", "ls", "-q"]).await;
    if ls.code != 0 {
        return Ok(None);
    }
    let ids: Vec<&str> = ls.out.split('\n').filter(|l| !l.is_empty()).collect();
    if ids.is_empty() {
        return Ok(Some(vec![]));
    }
    let mut args = vec!["network", "inspect"];
    args.extend(ids);
    let ins = sh("docker", &args).await;
    if ins.code != 0 {
        return Ok(None);
    }
    let mut list: Vec<DockerNetwork> = parse_json::<Vec<Value>>(&ins.out)?
        .iter()
        .map(|n| {
            let name = text(&n["Name"]);
            DockerNetwork {
                id: text(&n["Id"]).chars().take(12).collect(),
                builtin: BUILTIN_NETWORKS.contains(&name.as_str()),
                name,
                driver: text(&n["Driver"]),
                scope: text(&n["Scope"]),
                subnet: opt_text(&n["IPAM"]["Config"][0]["Subnet"]),
                containers: n["Containers"].as_object().map(|c| c.values().map(|c| text(&c["Name"])).collect()).unwrap_or_default(),
                project: opt_text(&n["Labels"]["com.docker.compose.project"]),
                internal: truthy(&n["Internal"]),
                created_at: text(&n["Created"]),
            }
        })
        .collect();
    list.sort_by(|a, b| b.builtin.cmp(&a.builtin).then_with(|| locale_cmp(&a.name, &b.name)));
    Ok(Some(list))
}

/// Remove a network — only a user-created one with nothing attached.
pub async fn network_remove(name: &str) -> ApiResult<ActionResult> {
    if BUILTIN_NETWORKS.contains(&docker_name(name)?) {
        return Err(HttpError::new(409, "Docker’s own networks can’t be removed"));
    }
    let ins = sh("docker", &["network", "inspect", name]).await;
    if ins.code != 0 {
        return Err(HttpError::not_found("Network not found"));
    }
    let list: Vec<Value> = parse_json(&ins.out)?;
    let attached = list.first().and_then(|n| n["Containers"].as_object()).is_some_and(|c| !c.is_empty());
    if attached {
        return Err(HttpError::new(409, "Containers are attached to this network"));
    }
    Ok(ActionResult::from(&sh("docker", &["network", "rm", name]).await))
}

static ANONYMOUS_VOLUME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[0-9a-f]{64}$").unwrap());

/// Volumes with usage and size (docker system df -v — sizes cost a disk walk, so only on demand).
pub async fn docker_volumes() -> ApiResult<Option<Vec<DockerVolume>>> {
    let ls = sh("docker", &["volume", "ls", "--format", "{{json .}}"]).await;
    if ls.code != 0 {
        return Ok(None);
    }
    let df = sh_timeout("docker", &["system", "df", "-v", "--format", "{{json .}}"], LONG).await;
    let mut usage: HashMap<String, (i64, Option<String>)> = HashMap::new();
    if let Ok(v) = serde_json::from_str::<Value>(&df.out)
        && let Some(vols) = v["Volumes"].as_array()
    {
        for v in vols {
            let links = match &v["Links"] {
                Value::Number(n) => n.as_f64().unwrap_or(0.0),
                Value::String(s) => s.trim().parse().unwrap_or(0.0),
                _ => 0.0,
            };
            let links = if links.is_finite() { links as i64 } else { 0 };
            usage.insert(text(&v["Name"]), (links, opt_text(&v["Size"])));
        }
    }
    let mut list: Vec<DockerVolume> = json_lines(&ls.out)?
        .iter()
        .map(|v| {
            let get = |k: &str| text(v.get(k).unwrap_or(&Value::Null));
            let labels: HashMap<String, Option<String>> = get("Labels")
                .split(',')
                .filter(|kv| !kv.is_empty())
                .map(|kv| {
                    let mut parts = kv.split('=');
                    (parts.next().unwrap_or("").to_string(), parts.next().map(String::from))
                })
                .collect();
            let name = get("Name");
            let u = usage.get(&name);
            DockerVolume {
                driver: get("Driver"),
                links: u.map_or(0, |u| u.0),
                size: u.and_then(|u| u.1.clone()),
                project: labels.get("com.docker.compose.project").cloned().flatten(),
                anonymous: labels.contains_key("com.docker.volume.anonymous") || ANONYMOUS_VOLUME.is_match(&name),
                mountpoint: get("Mountpoint"),
                name,
            }
        })
        .collect();
    list.sort_by(|a, b| a.anonymous.cmp(&b.anonymous).then_with(|| locale_cmp(&a.name, &b.name)));
    Ok(Some(list))
}

/// Remove a volume — only one no container (running or stopped) uses.
pub async fn volume_remove(name: &str) -> ApiResult<ActionResult> {
    let filter = format!("volume={}", docker_name(name)?);
    let users = sh("docker", &["ps", "-a", "-q", "--filter", &filter]).await;
    if users.code != 0 {
        let err = users.err.trim();
        return Err(HttpError::new(500, if err.is_empty() { "docker failed" } else { err }));
    }
    if !users.out.trim().is_empty() {
        return Err(HttpError::new(409, "A container uses this volume — remove the container first"));
    }
    Ok(ActionResult::from(&sh("docker", &["volume", "rm", name]).await))
}

pub async fn container_logs(name: &str, lines: f64) -> String {
    let n = js_string(lines);
    sh("docker", &["logs", "--tail", &n, "--timestamps", "--", name]).await.output()
}

pub async fn container_inspect_summary(name: &str) -> ApiResult<String> {
    let r = sh("docker", &["inspect", "--", name]).await;
    if r.code != 0 {
        return Ok(r.err.trim().to_string());
    }
    let list: Vec<Value> = parse_json(&r.out)?;
    Ok(inspect_summary(list.first().unwrap_or(&json!({}))))
}

/// The parts of `docker inspect` that explain a container's trouble (pretty JSON).
fn inspect_summary(d: &Value) -> String {
    let mut state = d["State"].as_object().cloned().unwrap_or_default();
    match d["State"]["Health"].as_object() {
        Some(h) => {
            let mut health = Map::new();
            for k in ["Status", "FailingStreak"] {
                if let Some(v) = h.get(k) {
                    health.insert(k.into(), v.clone());
                }
            }
            if let Some(log) = h.get("Log").and_then(Value::as_array) {
                health.insert("Last".into(), Value::Array(log[log.len().saturating_sub(3)..].to_vec()));
            }
            state.insert("Health".into(), Value::Object(health));
        }
        None => {
            state.remove("Health");
        }
    }
    let mut out = Map::new();
    // missing (undefined) fields are dropped, as JSON.stringify does; nulls stay
    let mut put = |k: &str, v: Option<Value>| {
        if let Some(v) = v {
            out.insert(k.into(), v);
        }
    };
    let at = |p: &str| d.pointer(p).cloned();
    put("Name", at("/Name"));
    put("Image", at("/Config/Image"));
    put("State", Some(Value::Object(state)));
    put("RestartCount", at("/RestartCount"));
    put("RestartPolicy", at("/HostConfig/RestartPolicy"));
    put("MemoryLimit", at("/HostConfig/Memory"));
    put("CpuLimit", at("/HostConfig/NanoCpus"));
    let mounts = d["Mounts"].as_array().map(|m| {
        Value::Array(
            m.iter()
                .map(|m| {
                    let s = |k: &str| if m[k].is_null() { "undefined".to_string() } else { text(&m[k]) };
                    Value::String(format!("{} -> {}", s("Source"), s("Destination")))
                })
                .collect(),
        )
    });
    put("Mounts", mounts);
    put("Ports", at("/NetworkSettings/Ports"));
    let labels: Map<String, Value> = d["Config"]["Labels"]
        .as_object()
        .map(|l| l.iter().filter(|(k, _)| k.starts_with("com.docker.compose")).map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default();
    put("Labels", Some(Value::Object(labels)));
    serde_json::to_string_pretty(&Value::Object(out)).unwrap_or_default()
}

// ---------- systemd ----------

fn scope_args(scope: UnitScope) -> &'static [&'static str] {
    if scope == UnitScope::User { &["--user"] } else { &[] }
}

async fn list_units(scope: UnitScope) -> Vec<UnitInfo> {
    let mut args: Vec<&str> = scope_args(scope).to_vec();
    args.extend(["list-units", "--type=service", "--all", "--no-pager", "--output=json"]);
    let r = sh("systemctl", &args).await;
    if r.code != 0 || r.out.trim().is_empty() {
        return vec![];
    }
    let Ok(list) = serde_json::from_str::<Vec<Value>>(&r.out) else { return vec![] };
    list.iter()
        .filter(|u| u["load"].as_str() != Some("not-found"))
        .map(|u| UnitInfo {
            name: text(&u["unit"]),
            scope,
            load: text(&u["load"]),
            active: text(&u["active"]),
            sub: text(&u["sub"]),
            description: text(&u["description"]),
        })
        .collect()
}

pub async fn units() -> ApiResult<Vec<UnitInfo>> {
    match system_platform().services {
        Some(ServiceManager::Launchd) => Ok(launchd_agents().await),
        Some(ServiceManager::Systemd) => {
            let (mut sys, user) = tokio::join!(list_units(UnitScope::System), list_units(UnitScope::User));
            sys.extend(user);
            Ok(sys)
        }
        None => Err(HttpError::new(501, "No supported service manager on this server (systemd or launchd)")),
    }
}

fn unit_name(name: &str) -> bool {
    static UNIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_@.:-]+$").unwrap());
    UNIT.is_match(name)
}

fn launchd() -> bool {
    system_platform().services == Some(ServiceManager::Launchd)
}

pub async fn unit_action(name: &str, scope: UnitScope, action: &str) -> ActionResult {
    if !unit_name(name) {
        return ActionResult::refused("Invalid unit name");
    }
    if scope == UnitScope::System {
        return ActionResult::refused("System units need root. Ask the agent, or run with sudo in a terminal.");
    }
    if launchd() {
        return launchd_action(name, action).await;
    }
    ActionResult::or_ok(&sh_timeout("systemctl", &["--user", action, "--", name], LONG).await)
}

pub async fn unit_logs(name: &str, scope: UnitScope, lines: f64) -> String {
    if !unit_name(name) {
        return "Invalid unit name".into();
    }
    if launchd() {
        return launchd_logs(name, lines).await;
    }
    let n = js_string(lines);
    let mut args: Vec<&str> = scope_args(scope).to_vec();
    args.extend(["-u", name, "-n", &n, "--no-pager", "-o", "short-iso"]);
    sh("journalctl", &args).await.output()
}

pub async fn unit_status(name: &str, scope: UnitScope) -> String {
    if !unit_name(name) {
        return "Invalid unit name".into();
    }
    if launchd() {
        return launchd_print(name).await.0;
    }
    let mut args: Vec<&str> = scope_args(scope).to_vec();
    args.extend(["status", "--no-pager", "--lines=0", "--", name]);
    sh("systemctl", &args).await.output()
}

// ---------- launchd (macOS) ----------
// Only the user's GUI domain: listing system daemons needs root, like system units on Linux.

fn gui_domain() -> String {
    // SAFETY: getuid has no preconditions.
    format!("gui/{}", unsafe { libc::getuid() })
}

/// `launchctl list`: "PID\tStatus\tLabel" rows; PID "-" — not running, Status — last exit code.
pub fn parse_launchctl_list(out: &str) -> Vec<UnitInfo> {
    out.split('\n')
        .skip(1)
        .map(|l| l.split('\t').collect::<Vec<_>>())
        .filter(|c| c.len() >= 3 && !c[2].is_empty())
        .map(|c| {
            let (pid, status, label) = (c[0], c[1], c[2]);
            let running = pid != "-";
            let failed = !running && status != "0" && status != "-";
            UnitInfo {
                name: label.into(),
                scope: UnitScope::User,
                load: "loaded".into(),
                active: (if running {
                    "active"
                } else if failed {
                    "failed"
                } else {
                    "inactive"
                })
                .into(),
                sub: if running {
                    format!("running (pid {pid})")
                } else if failed {
                    format!("exited {status}")
                } else {
                    "idle".into()
                },
                description: (if label.starts_with("com.apple.") { "Apple" } else { "" }).into(),
            }
        })
        .collect()
}

async fn launchd_agents() -> Vec<UnitInfo> {
    let r = sh("launchctl", &["list"]).await;
    if r.code == 0 { parse_launchctl_list(&r.out) } else { vec![] }
}

async fn launchd_action(label: &str, action: &str) -> ActionResult {
    let target = format!("{}/{label}", gui_domain());
    let args: Vec<&str> = match action {
        "stop" => vec!["kill", "SIGTERM", &target],
        "restart" => vec!["kickstart", "-k", &target],
        _ => vec!["kickstart", &target],
    };
    ActionResult::or_ok(&sh_timeout("launchctl", &args, LONG).await)
}

/// `launchctl print`: (text, stdout path, stderr path).
async fn launchd_print(label: &str) -> (String, Option<String>, Option<String>) {
    let r = sh("launchctl", &["print", &format!("{}/{label}", gui_domain())]).await;
    let text = r.output();
    let path = |k: &str| Regex::new(&format!("{k} = (.+)")).ok().and_then(|re| re.captures(&text)).map(|m| m[1].trim().to_string());
    let (out, err) = (path("stdout path"), path("stderr path"));
    (text, out, err)
}

/// Agents log to files from their plist (StandardOutPath/ErrorPath); otherwise to the unified log.
async fn launchd_logs(label: &str, lines: f64) -> String {
    let (_, out, err) = launchd_print(label).await;
    let mut files: Vec<String> = vec![];
    for f in [out, err].into_iter().flatten() {
        if !f.is_empty() && f != "/dev/null" && !files.contains(&f) {
            files.push(f);
        }
    }
    let n = js_string(lines);
    if !files.is_empty() {
        let mut parts = vec![];
        for f in &files {
            parts.push(format!("==> {f} <==\n{}", sh("tail", &["-n", &n, f]).await.out.trim()));
        }
        return parts.join("\n\n");
    }
    let proc = label.rsplit('.').next().unwrap_or(label);
    let predicate = format!("process == \"{proc}\"");
    let r = sh_timeout("log", &["show", "--last", "1h", "--style", "compact", "--predicate", &predicate], Duration::from_secs(30)).await;
    let t = tail(&format!("{}{}", r.out, r.err), if lines.is_finite() && lines > 0.0 { lines as usize } else { 0 });
    if t.is_empty() { "No log files configured for this agent and nothing in the unified log for the last hour.".into() } else { t }
}

// ---------- packages: details of a pacman package (lists are in packages.rs) ----------

fn pkg_name(name: &str) -> bool {
    static PKG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_@.+-]+$").unwrap());
    PKG.is_match(name)
}

pub async fn package_info(name: &str) -> String {
    if !pkg_name(name) {
        return "Invalid package name".into();
    }
    let r = sh("pacman", &["-Qi", "--", name]).await;
    let needle = format!(" {name} (");
    let log = tokio::fs::read("/var/log/pacman.log")
        .await
        .map(|b| {
            let text = String::from_utf8_lossy(&b);
            let hits: Vec<&str> = text.split('\n').filter(|l| l.contains(&needle)).collect();
            hits[hits.len().saturating_sub(10)..].join("\n")
        })
        .unwrap_or_default();
    format!("{}\n\nRecent pacman.log entries:\n{}", r.output(), if log.is_empty() { "—" } else { &log })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsers() {
        let df = parse_df(
            "Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/nvme0n1p2 976762584 488381292 488381292 50% /\ntmpfs 100 1 99 1% /tmp\n/dev/sdb1 2097152 1048576 1048576 50% /mnt/My Disk\n",
        );
        assert_eq!(df.len(), 2);
        assert_eq!(df[0].size_gb, 932);
        assert_eq!(df[0].used_percent, Some(50.0));
        assert_eq!(df[1].mount, "/mnt/My Disk");
        assert_eq!(parse_swap_usage("total = 2048.00M  used = 1024.50M  free = 1023.50M"), (2048, 1025));
        assert_eq!(parse_vm_stat("page size of 4096 bytes)\nPages free: 256.\nPages inactive: 0.\n"), 1.0);
        let l = parse_launchctl_list("PID\tStatus\tLabel\n123\t0\tcom.apple.x\n-\t78\tdev.y\n-\t0\tdev.z\n");
        assert_eq!(l.iter().map(|u| u.active.as_str()).collect::<Vec<_>>(), ["active", "failed", "inactive"]);
        assert_eq!(l[0].description, "Apple");
        assert_eq!(l[1].sub, "exited 78");
    }

    #[test]
    fn names_and_numbers() {
        assert!(DOCKER_NAME.is_match("web-1.app_x"));
        assert!(!DOCKER_NAME.is_match("-rf"));
        assert!(!DOCKER_NAME.is_match("bad name"));
        assert!(!DOCKER_NAME.is_match("ünï"));
        assert!(unit_name("getty@tty1.service"));
        assert!(!unit_name("a;b"));
        assert!(pkg_name("gtk+3"));
        assert!(!pkg_name("x y"));
        assert_eq!(js_string(lines_param(None)), "200");
        assert_eq!(js_string(lines_param(Some("abc"))), "NaN");
        assert_eq!(js_string(lines_param(Some("1.5"))), "1.5");
        assert_eq!(tail("a\nb\nc\n", 2), "b\nc");
    }

    #[test]
    fn inspect_summary_drops_missing_fields() {
        let d = json!({"Name": "/web", "State": {"Status": "running", "Health": {"Status": "healthy", "FailingStreak": 0, "Log": [1, 2, 3, 4]}}, "Mounts": [{"Source": "/a", "Destination": "/b"}]});
        let s: Value = serde_json::from_str(&inspect_summary(&d)).unwrap();
        assert_eq!(
            s,
            json!({"Name": "/web", "State": {"Status": "running", "Health": {"Status": "healthy", "FailingStreak": 0, "Last": [2, 3, 4]}}, "Mounts": ["/a -> /b"], "Labels": {}})
        );
    }
}
