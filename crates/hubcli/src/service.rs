//! `hubcli server …`: the hub as a systemd user service.
//!
//! Files (every base folder follows the XDG variables, so tests can point them elsewhere):
//! - `$XDG_CONFIG_HOME/hubness/hub.env` — the service settings (`HUBNESS_HOST`,
//!   `HUBNESS_PORT`, `HUBNESS_DATA_DIR`, a `PATH` snapshot; any other `HUBNESS_*` line the
//!   user adds is kept);
//! - `$XDG_CONFIG_HOME/systemd/user/hubness.service` — the unit, rewritten by `start` and
//!   `restart` when it no longer matches this binary;
//! - `$XDG_DATA_HOME/hubness` — the default data folder of the service.
//!
//! `HUBNESS_SYSTEMCTL`, `HUBNESS_JOURNALCTL` and `HUBNESS_LOGINCTL` replace the system tools
//! (tests use fake scripts that record their calls).

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use clap::Subcommand;

const UNIT: &str = "hubness.service";
const DEFAULT_HOST: &str = "127.0.0.1";
const DEFAULT_PORT: u16 = 8787;

#[derive(Subcommand)]
pub enum ServerCommand {
    /// Install (if needed) and start the service; it also starts at login
    Start {
        /// Data folder of the hub (database, logs, keys); kept in hub.env
        #[arg(long, value_name = "PATH")]
        data_dir: Option<String>,
    },
    /// Stop the service and don't start it at login
    Stop,
    /// Restart the service (after an upgrade or a config change)
    Restart,
    /// Is the hub running and answering? URL, data folder, accounts
    Status,
    /// Show the hub's log (journalctl)
    Logs {
        /// Keep printing new lines
        #[arg(short, long)]
        follow: bool,
        /// Number of lines to show
        #[arg(short = 'n', long, default_value_t = 100)]
        lines: u32,
    },
    /// Show or change the service settings: host, port, data-dir
    #[command(
        after_help = "Examples:\n  hubcli server config\n  hubcli server config port 9000\n  hubcli server config data-dir ~/hubness-data\n  hubcli server config host 0.0.0.0   # needs an account first"
    )]
    Config {
        /// host | port | data-dir
        key: Option<String>,
        /// The new value (prints the current one without it)
        value: Option<String>,
    },
    /// Stop the service and remove its unit (settings and data stay)
    Uninstall,
}

pub fn run(cmd: ServerCommand) -> anyhow::Result<()> {
    let paths = Paths::from_env()?;
    match cmd {
        ServerCommand::Config { key, value } => config(&paths, key.as_deref(), value.as_deref()),
        ServerCommand::Start { data_dir } => start(&paths, data_dir.as_deref()),
        ServerCommand::Stop => {
            let sys = Systemd::detect()?;
            if !paths.unit.exists() {
                println!("no service installed (`hubcli server start` installs it)");
                return Ok(());
            }
            sys.run(&["disable", "--now", UNIT])?;
            println!("hubness stopped (it won't start at login; `hubcli server start` brings it back)");
            Ok(())
        }
        ServerCommand::Restart => {
            let sys = Systemd::detect()?;
            let env = EnvFile::load_or_default(&paths)?;
            if write_unit(&paths, &env)? {
                sys.run(&["daemon-reload"])?;
            }
            sys.run(&["restart", UNIT])?;
            report_started(&paths, &env, &sys)
        }
        ServerCommand::Status => status(&paths),
        ServerCommand::Logs { follow, lines } => {
            let sys = Systemd::detect()?;
            let mut args = vec!["--user".to_string(), "-u".into(), UNIT.into(), "-n".into(), lines.to_string(), "--no-pager".into()];
            if follow {
                args.push("-f".into());
            }
            let st = Command::new(&sys.journalctl).args(&args).status().with_context(|| format!("can't run {}", sys.journalctl))?;
            if !st.success() {
                std::process::exit(st.code().unwrap_or(1));
            }
            Ok(())
        }
        ServerCommand::Uninstall => {
            let sys = Systemd::detect()?;
            if paths.unit.exists() {
                let _ = sys.run(&["disable", "--now", UNIT]);
                std::fs::remove_file(&paths.unit).with_context(|| format!("can't remove {}", paths.unit.display()))?;
                sys.run(&["daemon-reload"])?;
                println!("removed the service ({})", paths.unit.display());
            } else {
                println!("no service installed ({} doesn't exist)", paths.unit.display());
            }
            let env = EnvFile::load_or_default(&paths)?;
            println!("kept the settings: {}", paths.env.display());
            println!("kept the data:     {}", env.data_dir(&paths).display());
            Ok(())
        }
    }
}

// ---------- paths ----------

pub struct Paths {
    pub env: PathBuf,
    pub unit: PathBuf,
    pub default_data: PathBuf,
    home: PathBuf,
}

impl Paths {
    pub fn from_env() -> anyhow::Result<Self> {
        let home = std::env::var_os("HOME").filter(|h| !h.is_empty()).map(PathBuf::from).context("HOME is not set")?;
        let xdg = |var: &str, fallback: &str| {
            std::env::var_os(var)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .unwrap_or_else(|| home.join(fallback))
        };
        let config = xdg("XDG_CONFIG_HOME", ".config");
        let data = xdg("XDG_DATA_HOME", ".local/share");
        Ok(Self {
            env: config.join("hubness").join("hub.env"),
            unit: config.join("systemd").join("user").join(UNIT),
            default_data: data.join("hubness"),
            home,
        })
    }

    /// An absolute path from user input: `~` expanded, relative to the current folder.
    fn absolute(&self, p: &str) -> PathBuf {
        let p = if p == "~" {
            self.home.clone()
        } else if let Some(rest) = p.strip_prefix("~/") {
            self.home.join(rest)
        } else {
            PathBuf::from(p)
        };
        hubness_server::paths::resolve(&p.to_string_lossy())
    }
}

/// The data folder `hubcli user …` works on when HUBNESS_DATA_DIR is not set: the
/// service's one, once `hubcli server start` (or `config`) wrote hub.env.
pub fn service_data_dir() -> Option<PathBuf> {
    let paths = Paths::from_env().ok()?;
    if !paths.env.exists() {
        return None;
    }
    EnvFile::load(&paths.env).ok().and_then(|e| e.get("HUBNESS_DATA_DIR").map(PathBuf::from))
}

// ---------- hub.env ----------

/// A systemd `EnvironmentFile`, edited line by line so comments and extra settings survive.
pub struct EnvFile {
    lines: Vec<String>,
}

impl EnvFile {
    fn load(path: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| format!("can't read {}", path.display()))?;
        Ok(Self { lines: text.lines().map(String::from).collect() })
    }

    fn defaults(paths: &Paths) -> Self {
        let mut env = Self {
            lines: [
                "# hubness service settings (`hubcli server config` edits them; restart to apply).",
                "# HUBNESS_HOST other than 127.0.0.1 makes the hub reachable from other machines:",
                "# only with accounts, and behind HTTPS or a VPN.",
            ]
            .map(String::from)
            .to_vec(),
        };
        env.set("HUBNESS_HOST", DEFAULT_HOST);
        env.set("HUBNESS_PORT", &DEFAULT_PORT.to_string());
        env.set("HUBNESS_DATA_DIR", &paths.default_data.to_string_lossy());
        env.lines.push("# Agents and run commands see this PATH (`hubcli server start` adds new folders).".into());
        env.set("PATH", &std::env::var("PATH").unwrap_or_default());
        env
    }

    fn load_or_default(paths: &Paths) -> anyhow::Result<Self> {
        if paths.env.exists() { Self::load(&paths.env) } else { Ok(Self::defaults(paths)) }
    }

    fn get(&self, key: &str) -> Option<String> {
        self.lines.iter().rev().find_map(|l| parse_line(l).filter(|(k, _)| k == key).map(|(_, v)| v)).filter(|v| !v.is_empty())
    }

    fn set(&mut self, key: &str, value: &str) {
        let line = format!("{key}={}", quote(value));
        match self.lines.iter().rposition(|l| parse_line(l).is_some_and(|(k, _)| k == key)) {
            Some(i) => self.lines[i] = line,
            None => self.lines.push(line),
        }
    }

    fn host(&self) -> String {
        self.get("HUBNESS_HOST").unwrap_or_else(|| DEFAULT_HOST.into())
    }

    fn port(&self) -> u16 {
        self.get("HUBNESS_PORT").and_then(|p| p.parse().ok()).unwrap_or(DEFAULT_PORT)
    }

    fn data_dir(&self, paths: &Paths) -> PathBuf {
        self.get("HUBNESS_DATA_DIR").map(PathBuf::from).unwrap_or_else(|| paths.default_data.clone())
    }

    /// Add the folders of the current PATH the snapshot doesn't have yet (newly installed
    /// agent CLIs); folders the user put there stay first.
    fn merge_path(&mut self) {
        let current = std::env::var("PATH").unwrap_or_default();
        let mut dirs: Vec<String> = self.get("PATH").unwrap_or_default().split(':').filter(|d| !d.is_empty()).map(String::from).collect();
        for d in current.split(':').filter(|d| !d.is_empty()) {
            if !dirs.iter().any(|x| x == d) {
                dirs.push(d.to_string());
            }
        }
        self.set("PATH", &dirs.join(":"));
    }

    /// Write for the owner only (it may hold more settings later): 0600, folder 0700.
    fn save(&self, path: &Path) -> anyhow::Result<()> {
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
        if let Some(dir) = path.parent() {
            std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
        }
        let tmp = path.with_extension("env.tmp");
        let _ = std::fs::remove_file(&tmp);
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
        f.write_all((self.lines.join("\n") + "\n").as_bytes())?;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
        std::fs::rename(&tmp, path).with_context(|| format!("can't write {}", path.display()))
    }
}

fn parse_line(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    if line.starts_with('#') || line.starts_with(';') {
        return None;
    }
    let (k, v) = line.split_once('=')?;
    let v = v.trim();
    let v = if v.len() >= 2 && ((v.starts_with('"') && v.ends_with('"')) || (v.starts_with('\'') && v.ends_with('\''))) {
        let inner = &v[1..v.len() - 1];
        if v.starts_with('"') { inner.replace("\\\"", "\"").replace("\\\\", "\\") } else { inner.to_string() }
    } else {
        v.to_string()
    };
    Some((k.trim().to_string(), v))
}

fn quote(v: &str) -> String {
    if !v.is_empty() && v.chars().all(|c| c.is_ascii_alphanumeric() || "/._-:+@%,".contains(c)) {
        v.to_string()
    } else {
        format!("\"{}\"", v.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

// ---------- the unit ----------

fn unit_text(paths: &Paths) -> anyhow::Result<String> {
    let exe = std::env::current_exe().context("can't find the hubcli binary")?;
    let exe = exe.canonicalize().unwrap_or(exe);
    // `%` starts a specifier everywhere; only ExecStart takes quotes (and expands `$`)
    let plain = |p: &Path| p.to_string_lossy().replace('%', "%%");
    let exec = format!("\"{}\"", plain(&exe).replace('\\', "\\\\").replace('"', "\\\"").replace('$', "$$"));
    Ok(format!(
        "# Written by `hubcli server start`; it rewrites this file when hubcli moves.\n\
         [Unit]\n\
         Description=hubness: self-hosted hub for coding agents\n\
         Documentation=https://github.com/Tsunami43/hubness\n\
         After=network.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         EnvironmentFile={env}\n\
         WorkingDirectory={home}\n\
         ExecStart={exe} serve\n\
         Restart=on-failure\n\
         RestartSec=3\n\
         TimeoutStopSec=20\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n",
        env = plain(&paths.env),
        home = plain(&paths.home),
        exe = exec,
    ))
}

/// Write the unit when it is missing or differs; `true` when it changed.
fn write_unit(paths: &Paths, env: &EnvFile) -> anyhow::Result<bool> {
    if !paths.env.exists() {
        env.save(&paths.env)?;
    }
    let text = unit_text(paths)?;
    if std::fs::read_to_string(&paths.unit).is_ok_and(|t| t == text) {
        return Ok(false);
    }
    if let Some(dir) = paths.unit.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&paths.unit, text).with_context(|| format!("can't write {}", paths.unit.display()))?;
    Ok(true)
}

// ---------- systemd ----------

struct Systemd {
    systemctl: String,
    journalctl: String,
    loginctl: String,
}

impl Systemd {
    /// The user manager, or a clear message where there is none (macOS, containers).
    fn detect() -> anyhow::Result<Self> {
        let tool = |var: &str, name: &str| std::env::var(var).ok().filter(|v| !v.is_empty()).unwrap_or_else(|| name.into());
        let sys = Self {
            systemctl: tool("HUBNESS_SYSTEMCTL", "systemctl"),
            journalctl: tool("HUBNESS_JOURNALCTL", "journalctl"),
            loginctl: tool("HUBNESS_LOGINCTL", "loginctl"),
        };
        let ok = cfg!(target_os = "linux")
            && Command::new(&sys.systemctl)
                .args(["--user", "show-environment"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|s| s.success());
        if !ok {
            bail!(
                "no systemd user manager here, so hubcli can't run the hub as a service.\n\
                 Run it in the foreground instead, under your own process manager (launchd, a container, tmux…):\n  \
                 hubcli serve\n\
                 It reads HUBNESS_HOST, HUBNESS_PORT and HUBNESS_DATA_DIR from the environment."
            );
        }
        Ok(sys)
    }

    fn run(&self, args: &[&str]) -> anyhow::Result<()> {
        let st =
            Command::new(&self.systemctl).arg("--user").args(args).status().with_context(|| format!("can't run {}", self.systemctl))?;
        if !st.success() {
            bail!("`systemctl --user {}` failed", args.join(" "));
        }
        Ok(())
    }

    fn query(&self, args: &[&str]) -> String {
        Command::new(&self.systemctl)
            .arg("--user")
            .args(args)
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default()
    }

    /// Linger off: the user manager (and the hub) stops at logout and doesn't start at boot.
    fn linger_off(&self) -> bool {
        let user = std::env::var("USER").unwrap_or_default();
        Command::new(&self.loginctl)
            .args(["show-user", &user, "-p", "Linger", "--value"])
            .output()
            .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "no")
    }
}

// ---------- commands ----------

fn start(paths: &Paths, data_dir: Option<&str>) -> anyhow::Result<()> {
    let sys = Systemd::detect()?;
    let existed = paths.env.exists();
    let mut env = EnvFile::load_or_default(paths)?;
    // the first start asks where the data goes (in a terminal; scripts pass --data-dir)
    let asked = if data_dir.is_none() && !existed { ask_data_dir(paths)? } else { None };
    if let Some(d) = data_dir.or(asked.as_deref()) {
        env.set("HUBNESS_DATA_DIR", &paths.absolute(d).to_string_lossy());
    }
    env.merge_path();
    check_public(&env.host(), &env.data_dir(paths))?;
    env.save(&paths.env)?;
    if !existed {
        println!("wrote the settings: {}", paths.env.display());
    }
    if write_unit(paths, &env)? {
        println!("wrote the service:  {}", paths.unit.display());
        sys.run(&["daemon-reload"])?;
    }
    sys.run(&["enable", UNIT])?;
    // restart, not start: picks up a new binary or new settings when it already runs
    sys.run(&["restart", UNIT])?;
    report_started(paths, &env, &sys)
}

/// Ask for the data folder on the first start; `None` without a terminal (the default stays).
/// An existing hub's `./.data` (a dev checkout) is offered as the default.
fn ask_data_dir(paths: &Paths) -> anyhow::Result<Option<String>> {
    use std::io::{BufRead, IsTerminal, Write};
    if !std::io::stdin().is_terminal() {
        return Ok(None);
    }
    let local = std::env::current_dir().ok().map(|d| d.join(".data")).filter(|d| d.join("hubness.sqlite").exists());
    let default = local.unwrap_or_else(|| paths.default_data.clone());
    let shown = match default.strip_prefix(&paths.home) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => default.display().to_string(),
    };
    print!("Where should the hub keep its data (database, logs, keys)? [{shown}] ");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    let answer = line.trim();
    Ok(Some(if answer.is_empty() { default.to_string_lossy().into_owned() } else { answer.to_string() }))
}

fn report_started(paths: &Paths, env: &EnvFile, sys: &Systemd) -> anyhow::Result<()> {
    let url = local_url(env);
    let deadline = Instant::now() + Duration::from_secs(15);
    while !healthy(&url) {
        if Instant::now() > deadline {
            bail!("the service started, but {url}/api/health doesn't answer: see `hubcli server logs`");
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    println!("hubness is running: {url}");
    println!("  data: {}", env.data_dir(paths).display());
    if accounts(&env.data_dir(paths)) == 0 {
        println!("  no accounts yet: open to this machine only. Add one (needed for remote use):");
        println!("    hubcli user add <name>");
    }
    if sys.linger_off() {
        println!("  it stops when you log out; to keep it running across logouts and reboots:");
        println!("    sudo loginctl enable-linger {}", std::env::var("USER").unwrap_or_else(|_| "$USER".into()));
    }
    Ok(())
}

fn status(paths: &Paths) -> anyhow::Result<()> {
    let sys = Systemd::detect()?;
    let env = EnvFile::load_or_default(paths)?;
    let installed = paths.unit.exists();
    let active = sys.query(&["is-active", UNIT]);
    let enabled = sys.query(&["is-enabled", UNIT]);
    let url = local_url(&env);
    let health = healthy(&url);
    let n = accounts(&env.data_dir(paths));
    let state = if !installed {
        "not installed (`hubcli server start`)".to_string()
    } else {
        format!("{} ({})", if active.is_empty() { "unknown" } else { &active }, if enabled.is_empty() { "?" } else { &enabled })
    };
    println!("service   {state}");
    println!("health    {}", if health { "ok" } else { "not answering" });
    println!("url       {url}");
    println!("data      {}", env.data_dir(paths).display());
    println!("accounts  {}", if n == 0 { "none: open to this machine only".to_string() } else { n.to_string() });
    println!("settings  {}", paths.env.display());
    if !health {
        std::process::exit(3);
    }
    Ok(())
}

fn config(paths: &Paths, key: Option<&str>, value: Option<&str>) -> anyhow::Result<()> {
    let mut env = EnvFile::load_or_default(paths)?;
    let Some(key) = key else {
        println!("host      {}", env.host());
        println!("port      {}", env.port());
        println!("data-dir  {}", env.data_dir(paths).display());
        println!();
        println!("file      {}{}", paths.env.display(), if paths.env.exists() { "" } else { " (not written yet)" });
        return Ok(());
    };
    let var = match key {
        "host" => "HUBNESS_HOST",
        "port" => "HUBNESS_PORT",
        "data-dir" | "data_dir" => "HUBNESS_DATA_DIR",
        other => {
            bail!("unknown setting {other}: host, port or data-dir (other HUBNESS_* settings go into {} by hand)", paths.env.display())
        }
    };
    let Some(value) = value else {
        match var {
            "HUBNESS_HOST" => println!("{}", env.host()),
            "HUBNESS_PORT" => println!("{}", env.port()),
            _ => println!("{}", env.data_dir(paths).display()),
        }
        return Ok(());
    };
    let value = match var {
        "HUBNESS_HOST" => {
            let v = value.trim();
            if v.is_empty() || v.chars().any(|c| c.is_whitespace() || c == '/') {
                bail!("not a host name or address: {value}");
            }
            check_public(v, &env.data_dir(paths))?;
            v.to_string()
        }
        "HUBNESS_PORT" => match value.trim().parse::<u16>() {
            Ok(p) if p > 0 => p.to_string(),
            _ => bail!("not a port number: {value}"),
        },
        _ => {
            let d = paths.absolute(value.trim());
            if !d.exists() {
                println!("note: {} doesn't exist yet, the hub creates it", d.display());
            } else if !d.is_dir() {
                bail!("{} is not a folder", d.display());
            }
            d.to_string_lossy().into_owned()
        }
    };
    env.set(var, &value);
    env.save(&paths.env)?;
    println!("{key} = {value}");
    if paths.unit.exists() {
        println!("apply it: hubcli server restart");
    }
    Ok(())
}

/// Never open the hub to other machines without accounts (the server refuses it too).
fn check_public(host: &str, data_dir: &Path) -> anyhow::Result<()> {
    if !hubness_server::auth::is_loopback(host) && accounts(data_dir) == 0 {
        bail!(
            "host {host} makes the hub reachable from other machines, but {} has no accounts.\n\
             Create one first: hubcli user add <name>",
            data_dir.display()
        );
    }
    Ok(())
}

/// Accounts in the hub database of a data folder (0 when there is none yet).
fn accounts(data_dir: &Path) -> i64 {
    let db = data_dir.join("hubness.sqlite");
    if !db.exists() {
        return 0;
    }
    hubness_server::db::open(&db).ok().and_then(|c| hubness_server::users::count(&c).ok()).unwrap_or(0)
}

/// Where this machine reaches the hub (`0.0.0.0` and `::` mean every interface).
fn local_url(env: &EnvFile) -> String {
    let host = env.host();
    let host = match host.as_str() {
        "0.0.0.0" | "" => "127.0.0.1".to_string(),
        "::" | "[::]" => "[::1]".to_string(),
        h if h.contains(':') && !h.starts_with('[') => format!("[{h}]"),
        h => h.to_string(),
    };
    format!("http://{host}:{}", env.port())
}

/// `GET /api/health` answered 200 (plain HTTP/1.0, no client library needed).
fn healthy(url: &str) -> bool {
    let Some(authority) = url.strip_prefix("http://") else { return false };
    let Ok(addrs) = std::net::ToSocketAddrs::to_socket_addrs(authority) else { return false };
    for addr in addrs {
        let Ok(mut s) = std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(1)) else { continue };
        let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
        if s.write_all(format!("GET /api/health HTTP/1.0\r\nHost: {authority}\r\n\r\n").as_bytes()).is_err() {
            continue;
        }
        let mut buf = String::new();
        let _ = s.take(4096).read_to_string(&mut buf);
        if buf.starts_with("HTTP/1.1 200") || buf.starts_with("HTTP/1.0 200") {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(dir: &Path) -> Paths {
        Paths {
            env: dir.join("config/hubness/hub.env"),
            unit: dir.join("config/systemd/user/hubness.service"),
            default_data: dir.join("data/hubness"),
            home: dir.to_path_buf(),
        }
    }

    #[test]
    fn env_file_keeps_comments_and_quotes_values() {
        let dir = std::env::temp_dir().join(format!("hubcli-env-{}", std::process::id()));
        let p = paths(&dir);
        let mut env = EnvFile::defaults(&p);
        assert_eq!(env.host(), "127.0.0.1");
        assert_eq!(env.port(), 8787);
        assert_eq!(env.data_dir(&p), dir.join("data/hubness"));
        env.lines.push("HUBNESS_PREVIEW_PORTS=5173".into());
        env.set("HUBNESS_DATA_DIR", "/srv/hub data/\"x\"");
        env.set("HUBNESS_PORT", "9000");
        env.save(&p.env).unwrap();
        let back = EnvFile::load(&p.env).unwrap();
        assert_eq!(back.get("HUBNESS_DATA_DIR").as_deref(), Some("/srv/hub data/\"x\""));
        assert_eq!(back.port(), 9000);
        assert_eq!(back.get("HUBNESS_PREVIEW_PORTS").as_deref(), Some("5173"));
        assert!(back.lines[0].starts_with('#'));
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&p.env).unwrap().permissions().mode() & 0o777, 0o600);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn parse_systemd_lines() {
        assert_eq!(parse_line("PATH=\"/a b:/c\""), Some(("PATH".into(), "/a b:/c".into())));
        assert_eq!(parse_line(" X = 'y' "), Some(("X".into(), "y".into())));
        assert_eq!(parse_line("# HUBNESS_HOME=x"), None);
        assert_eq!(quote("/usr/bin:/bin"), "/usr/bin:/bin");
        assert_eq!(quote("a b"), "\"a b\"");
    }

    #[test]
    fn urls_for_this_machine() {
        let mut env = EnvFile { lines: vec![] };
        assert_eq!(local_url(&env), "http://127.0.0.1:8787");
        env.set("HUBNESS_HOST", "0.0.0.0");
        env.set("HUBNESS_PORT", "9001");
        assert_eq!(local_url(&env), "http://127.0.0.1:9001");
        env.set("HUBNESS_HOST", "::1");
        assert_eq!(local_url(&env), "http://[::1]:9001");
    }

    #[test]
    fn public_host_needs_an_account() {
        let dir = std::env::temp_dir().join(format!("hubcli-public-{}", std::process::id()));
        assert!(check_public("127.0.0.1", &dir).is_ok());
        assert!(check_public("0.0.0.0", &dir).is_err());
    }
}
