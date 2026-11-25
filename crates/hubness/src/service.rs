//! `hubness server …`: the hub as a systemd user service.
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

use crate::hubenv::{EnvFile, Paths, UNIT, local_url};

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
        after_help = "Examples:\n  hubness server config\n  hubness server config port 9000\n  hubness server config data-dir ~/hubness-data\n  hubness server config host 0.0.0.0   # needs an account first"
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
                println!("no service installed (`hubness server start` installs it)");
                return Ok(());
            }
            sys.run(&["disable", "--now", UNIT])?;
            println!("hubness stopped (it won't start at login; `hubness server start` brings it back)");
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

/// An absolute path from user input: `~` expanded, relative to the current folder.
fn absolute(paths: &Paths, p: &str) -> PathBuf {
    let p = if p == "~" {
        paths.home.clone()
    } else if let Some(rest) = p.strip_prefix("~/") {
        paths.home.join(rest)
    } else {
        PathBuf::from(p)
    };
    hubness_server::paths::resolve(&p.to_string_lossy())
}

// ---------- the unit ----------

fn unit_text(paths: &Paths) -> anyhow::Result<String> {
    let exe = std::env::current_exe().context("can't find the hubness binary")?;
    let exe = exe.canonicalize().unwrap_or(exe);
    // `%` starts a specifier everywhere; only ExecStart takes quotes (and expands `$`)
    let plain = |p: &Path| p.to_string_lossy().replace('%', "%%");
    let exec = format!("\"{}\"", plain(&exe).replace('\\', "\\\\").replace('"', "\\\"").replace('$', "$$"));
    Ok(format!(
        "# Written by `hubness server start`; it rewrites this file when the binary moves.\n\
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
                "no systemd user manager here, so hubness can't run the hub as a service.\n\
                 Run it in the foreground instead, under your own process manager (launchd, a container, tmux…):\n  \
                 hubness serve\n\
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

/// The service is installed and running (`hubness update` restarts it on the new binary).
/// `false` without a systemd user manager.
pub fn is_active() -> bool {
    let Ok(paths) = Paths::from_env() else { return false };
    paths.unit.exists() && Systemd::detect().is_ok_and(|sys| sys.query(&["is-active", UNIT]) == "active")
}

// ---------- commands ----------

fn start(paths: &Paths, data_dir: Option<&str>) -> anyhow::Result<()> {
    let sys = Systemd::detect()?;
    let existed = paths.env.exists();
    let mut env = EnvFile::load_or_default(paths)?;
    // the first start asks where the data goes (in a terminal; scripts pass --data-dir)
    let asked = if data_dir.is_none() && !existed { ask_data_dir(paths)? } else { None };
    if let Some(d) = data_dir.or(asked.as_deref()) {
        env.set("HUBNESS_DATA_DIR", &absolute(paths, d).to_string_lossy());
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
            bail!("the service started, but {url}/api/health doesn't answer: see `hubness server logs`");
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    println!("hubness is running: {url}");
    println!("  data: {}", env.data_dir(paths).display());
    if accounts(&env.data_dir(paths)) == 0 {
        println!("  no accounts yet: open to this machine only. Add one (needed for remote use):");
        println!("    hubness user add <name>");
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
        "not installed (`hubness server start`)".to_string()
    } else {
        format!("{} ({})", if active.is_empty() { "unknown" } else { &active }, if enabled.is_empty() { "?" } else { &enabled })
    };
    println!("service   {state}");
    println!("health    {}", if health { "ok" } else { "not answering" });
    println!("url       {url}");
    println!("data      {}", env.data_dir(paths).display());
    println!("accounts  {}", if n == 0 { "none: open to this machine only".to_string() } else { n.to_string() });
    println!("settings  {}", paths.env.display());
    if let Some(latest) = crate::update::cached_newer(true) {
        println!("update    {latest} is available: hubness update");
    }
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
            let d = absolute(paths, value.trim());
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
        println!("apply it: hubness server restart");
    }
    Ok(())
}

/// Never open the hub to other machines without accounts (the server refuses it too).
fn check_public(host: &str, data_dir: &Path) -> anyhow::Result<()> {
    if !hubness_server::auth::is_loopback(host) && accounts(data_dir) == 0 {
        bail!(
            "host {host} makes the hub reachable from other machines, but {} has no accounts.\n\
             Create one first: hubness user add <name>",
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

    #[test]
    fn unit_of_an_old_binary_is_rewritten() {
        let dir = std::env::temp_dir().join(format!("hubness-unit-{}", std::process::id()));
        let p = Paths {
            env: dir.join("config/hubness/hub.env"),
            unit: dir.join("config/systemd/user/hubness.service"),
            default_data: dir.join("data/hubness"),
            home: dir.clone(),
        };
        let env = EnvFile::defaults(&p);
        // what the 0.1 `hubcli server start` wrote
        let old = unit_text(&p)
            .unwrap()
            .replace(&std::env::current_exe().unwrap().canonicalize().unwrap().to_string_lossy().to_string(), "/home/u/.local/bin/hubcli");
        std::fs::create_dir_all(p.unit.parent().unwrap()).unwrap();
        std::fs::write(&p.unit, &old).unwrap();
        assert!(write_unit(&p, &env).unwrap(), "an ExecStart on another binary is rewritten");
        let text = std::fs::read_to_string(&p.unit).unwrap();
        assert!(!text.contains("/home/u/.local/bin/hubcli"));
        assert!(!write_unit(&p, &env).unwrap(), "an up-to-date unit stays");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn public_host_needs_an_account() {
        let dir = std::env::temp_dir().join(format!("hubness-public-{}", std::process::id()));
        assert!(check_public("127.0.0.1", &dir).is_ok());
        assert!(check_public("0.0.0.0", &dir).is_err());
    }
}
