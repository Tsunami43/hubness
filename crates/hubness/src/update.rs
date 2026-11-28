//! `hubness update`: replace this binary with a GitHub release of the same target and flavor.
//!
//! Releases live at `$HUBNESS_RELEASE_URL` (default `https://github.com/Tsunami43/hubness/releases`):
//! - `latest/download/VERSION` — the latest version (a plain file: no API, no rate limit);
//! - `download/v<version>/hubness-<version>-<target>-<flavor>.tar.gz` and its `.sha256`.
//!
//! The base may be a `file://` folder with the same layout (tests). The archive holds a
//! `hubness-<version>-<target>-<flavor>/` folder with the `hubness` binary.
//!
//! The last known latest version is cached in `$XDG_CACHE_HOME/hubness/update-check`, so
//! `hubness --version` and `hubness server status` can mention an update without a network
//! request on every command.

use std::cmp::Ordering;
use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, bail};
use hubness_core::version::{Version, compare};
use sha2::{Digest, Sha256};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const FLAVOR: &str = env!("HUBNESS_FLAVOR");
pub const TARGET: &str = env!("HUBNESS_TARGET");
const RELEASES: &str = "https://github.com/Tsunami43/hubness/releases";
pub const INSTALL_FULL: &str = "curl -fsSL https://raw.githubusercontent.com/Tsunami43/hubness/main/install.sh | sh";
/// Exit code of `hubness update --check` when a newer release exists.
const EXIT_UPDATE_AVAILABLE: i32 = 10;
const CHECK_EVERY: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(clap::Args)]
#[command(after_help = "Examples:\n  hubness update --check       exit code 0: up to date, 10: an update is available\n  \
                        hubness update               install the latest release\n  \
                        hubness update --version 0.2.0 --yes   a specific release (also an older one)")]
pub struct Args {
    /// Only report whether a newer release exists
    #[arg(long)]
    check: bool,
    /// Install this release instead of the latest one (a downgrade asks first)
    #[arg(long = "version", value_name = "X")]
    version: Option<String>,
    /// Don't ask: allow a downgrade, or replacing a binary installed by cargo
    #[arg(short, long)]
    yes: bool,
}

/// `hubness 0.2.0 (full) x86_64-unknown-linux-gnu`
pub fn long_version() -> String {
    format!("hubness {VERSION} ({FLAVOR}) {TARGET}")
}

pub fn run(args: Args) -> anyhow::Result<()> {
    let base = releases_url();
    let wanted = match args.version.as_deref() {
        Some(v) => {
            let v = v.trim().trim_start_matches('v').to_string();
            Version::parse(&v).with_context(|| format!("not a version: {v} (e.g. 0.2.0)"))?;
            v
        }
        None => {
            let latest = latest_version(&base, Duration::from_secs(30))?;
            save_cache(&latest);
            latest
        }
    };
    let order = compare(&wanted, VERSION);
    let link = format!("https://github.com/Tsunami43/hubness/releases/tag/v{wanted}");

    if args.check {
        if order == Ordering::Greater {
            println!("hubness {wanted} is available (this is {VERSION}): hubness update");
            println!("  changes: {link}");
            std::process::exit(EXIT_UPDATE_AVAILABLE);
        }
        println!("hubness {VERSION} is up to date");
        return Ok(());
    }
    match order {
        Ordering::Equal => {
            println!("hubness {VERSION} is already installed");
            return Ok(());
        }
        Ordering::Less if args.version.is_none() => {
            println!("hubness {VERSION} is newer than the latest release {wanted}: nothing to do");
            return Ok(());
        }
        Ordering::Less => {
            if !confirm(args.yes, &format!("Downgrade hubness {VERSION} to {wanted}?"))? {
                bail!("not downgraded (pass --yes to allow it)");
            }
        }
        Ordering::Greater => {}
    }

    let exe = std::env::current_exe().context("can't find the running hubness binary")?;
    let exe = exe.canonicalize().unwrap_or(exe);
    if let Some(bin) = cargo_bin(&exe) {
        println!("{} was installed by cargo ({}).", exe.display(), bin.display());
        println!("  `cargo install --locked --git https://github.com/Tsunami43/hubness hubness` keeps cargo's records right.");
        if !confirm(args.yes, "Replace it with the release binary anyway?")? {
            bail!("not updated (pass --yes to replace it anyway)");
        }
    }

    let name = archive_name(&wanted, TARGET, FLAVOR);
    let url = format!("{base}/download/v{wanted}/{name}");
    println!("downloading {name}");
    let archive = fetch(&url, None).with_context(|| format!("can't download {url} (is {wanted} released for {TARGET}?)"))?;
    let sums = fetch(&format!("{url}.sha256"), Some(Duration::from_secs(30))).context("can't download the checksum")?;
    verify(&archive, &String::from_utf8_lossy(&sums))?;
    let binary = extract(&archive)?;
    install(&exe, &binary, &wanted)?;
    save_cache(&wanted);
    println!("updated hubness {VERSION} → {wanted} ({FLAVOR}): {}", exe.display());
    println!("  changes: {link}");
    restart_service(&exe);
    Ok(())
}

fn releases_url() -> String {
    std::env::var("HUBNESS_RELEASE_URL").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| RELEASES.into()).trim_end_matches('/').to_string()
}

/// `hubness-0.2.0-x86_64-unknown-linux-gnu-full.tar.gz`
pub fn archive_name(version: &str, target: &str, flavor: &str) -> String {
    format!("hubness-{version}-{target}-{flavor}.tar.gz")
}

fn latest_version(base: &str, timeout: Duration) -> anyhow::Result<String> {
    let url = format!("{base}/latest/download/VERSION");
    let body = fetch(&url, Some(timeout)).with_context(|| format!("can't find the latest release ({url})"))?;
    let v = String::from_utf8_lossy(&body).trim().trim_start_matches('v').to_string();
    Version::parse(&v).with_context(|| format!("{url} doesn't hold a version: {v:?}"))?;
    Ok(v)
}

// ---------- downloads ----------

/// The body of a URL (`file://` reads a local file); `timeout` bounds the whole request.
fn fetch(url: &str, timeout: Option<Duration>) -> anyhow::Result<Vec<u8>> {
    if let Some(path) = url.strip_prefix("file://") {
        return std::fs::read(path).with_context(|| format!("can't read {path}"));
    }
    let url = url.to_string();
    tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async move {
        let mut client = reqwest::Client::builder().user_agent(format!("hubness/{VERSION}")).connect_timeout(Duration::from_secs(10));
        if let Some(t) = timeout {
            client = client.timeout(t);
        }
        let resp = client.build()?.get(&url).send().await?;
        let status = resp.status();
        if !status.is_success() {
            bail!("{url}: HTTP {status}");
        }
        Ok(resp.bytes().await?.to_vec())
    })
}

/// The archive matches the first hash of its `.sha256` file (`<hex>  <name>`).
fn verify(archive: &[u8], sums: &str) -> anyhow::Result<()> {
    let want = sums.split_whitespace().next().unwrap_or_default().to_ascii_lowercase();
    let got: String = Sha256::digest(archive).iter().map(|b| format!("{b:02x}")).collect();
    if want != got {
        bail!("checksum mismatch: expected {want}, got {got}; nothing was changed");
    }
    Ok(())
}

/// The `hubness` binary out of a release `.tar.gz`.
fn extract(archive: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    for entry in tar.entries().context("not a .tar.gz archive")? {
        let mut entry = entry.context("broken archive")?;
        let is_binary = entry.header().entry_type().is_file() && entry.path().is_ok_and(|p| p.file_name().is_some_and(|n| n == "hubness"));
        if is_binary {
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).context("broken archive")?;
            return Ok(bytes);
        }
    }
    bail!("the archive has no hubness binary")
}

// ---------- replacing the binary ----------

/// Write the new binary next to the old one, check that it runs, then rename it over the
/// old one (atomic; a running hub keeps its old file until it restarts).
fn install(exe: &Path, binary: &[u8], version: &str) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let dir = exe.parent().context("the binary has no folder")?;
    let new = dir.join(".hubness.new");
    let _ = std::fs::remove_file(&new);
    let written = std::fs::File::create(&new).and_then(|mut f| {
        f.write_all(binary)?;
        f.sync_all()?;
        f.set_permissions(std::fs::Permissions::from_mode(0o755))
    });
    if let Err(e) = written {
        let _ = std::fs::remove_file(&new);
        return Err(replace_error(exe, e));
    }
    // a binary for another system or a broken download fails here, before anything changes
    let out = Command::new(&new).arg("--version").output();
    let reported = out.as_ref().map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default();
    if !reported.starts_with(&format!("hubness {version}")) {
        let _ = std::fs::remove_file(&new);
        match out {
            Err(e) => bail!("the downloaded binary doesn't run here ({e}); nothing was changed"),
            Ok(_) => bail!(
                "the downloaded binary isn't hubness {version} (it reports {:?}); nothing was changed",
                reported.lines().next().unwrap_or_default()
            ),
        }
    }
    std::fs::rename(&new, exe).map_err(|e| {
        let _ = std::fs::remove_file(&new);
        replace_error(exe, e)
    })
}

fn replace_error(exe: &Path, e: std::io::Error) -> anyhow::Error {
    let hint = match e.kind() {
        std::io::ErrorKind::PermissionDenied => "run it with sudo, or re-install into a folder you own:",
        std::io::ErrorKind::CrossesDevices => "the binary is on another file system; re-install it:",
        _ => "re-install it:",
    };
    let install = if FLAVOR == "client" { format!("{INSTALL_FULL} -s -- --tui-only") } else { INSTALL_FULL.to_string() };
    anyhow::anyhow!("can't replace {}: {e}\n{hint}\n  {install}", exe.display())
}

/// `~/.cargo/bin` (or `$CARGO_HOME/bin`) when the binary lives there.
fn cargo_bin(exe: &Path) -> Option<PathBuf> {
    let home = std::env::var_os("CARGO_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").filter(|v| !v.is_empty()).map(|h| PathBuf::from(h).join(".cargo")))?;
    let bin = home.join("bin");
    let bin = bin.canonicalize().unwrap_or(bin);
    (exe.parent() == Some(bin.as_path())).then_some(bin)
}

fn confirm(yes: bool, question: &str) -> anyhow::Result<bool> {
    if yes {
        return Ok(true);
    }
    if !std::io::stdin().is_terminal() {
        return Ok(false);
    }
    print!("{question} [y/N] ");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(matches!(line.trim(), "y" | "Y" | "yes"))
}

/// The service runs the replaced binary: restart it with the new one (which also rewrites
/// the unit if it changed).
#[cfg(feature = "server")]
fn restart_service(exe: &Path) {
    if !crate::service::is_active() {
        return;
    }
    println!("restarting the hub service");
    match Command::new(exe).args(["server", "restart"]).status() {
        Ok(s) if s.success() => {}
        _ => eprintln!("the service didn't restart: run `hubness server restart`"),
    }
}

#[cfg(not(feature = "server"))]
fn restart_service(_exe: &Path) {}

// ---------- cached check ----------

fn cache_file() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").filter(|v| !v.is_empty()).map(|h| PathBuf::from(h).join(".cache")))?;
    Some(base.join("hubness").join("update-check"))
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn save_cache(latest: &str) {
    if let Some(path) = cache_file() {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, format!("{} {latest}\n", now()));
    }
}

/// A newer release than this binary according to the cached check. With `refresh`, a check
/// older than a day is repeated first (a few seconds at most, errors ignored);
/// `HUBNESS_NO_UPDATE_CHECK=1` turns that off.
pub fn cached_newer(refresh: bool) -> Option<String> {
    let path = cache_file()?;
    let cached = std::fs::read_to_string(&path).ok().and_then(|s| {
        let (at, v) = s.trim().split_once(' ')?;
        Some((at.parse::<u64>().ok()?, v.to_string()))
    });
    let stale = cached.as_ref().is_none_or(|(at, _)| now().saturating_sub(*at) > CHECK_EVERY.as_secs());
    let disabled = std::env::var_os("HUBNESS_NO_UPDATE_CHECK").is_some_and(|v| !v.is_empty() && v != "0");
    let latest = if refresh && stale && !disabled {
        match latest_version(&releases_url(), Duration::from_secs(3)) {
            Ok(v) => {
                save_cache(&v);
                Some(v)
            }
            Err(_) => {
                // don't retry on every command while offline
                if let Some((_, v)) = &cached {
                    save_cache(v);
                }
                cached.map(|(_, v)| v)
            }
        }
    } else {
        cached.map(|(_, v)| v)
    };
    latest.filter(|v| compare(v, VERSION) == Ordering::Greater)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_names() {
        assert_eq!(archive_name("0.2.0", "aarch64-apple-darwin", "client"), "hubness-0.2.0-aarch64-apple-darwin-client.tar.gz");
    }

    fn tar_gz(files: &[(&str, &[u8])]) -> Vec<u8> {
        let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        let mut b = tar::Builder::new(gz);
        for (path, data) in files {
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(0o755);
            h.set_cksum();
            b.append_data(&mut h, path, *data).unwrap();
        }
        b.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn extracts_and_verifies_the_binary() {
        let archive = tar_gz(&[("hubness-9.9.9-x-full/README.md", b"readme"), ("hubness-9.9.9-x-full/hubness", b"#!/bin/sh\n")]);
        assert_eq!(extract(&archive).unwrap(), b"#!/bin/sh\n");
        assert!(extract(&tar_gz(&[("x/README.md", b"r")])).is_err());
        let sum: String = Sha256::digest(&archive).iter().map(|b| format!("{b:02x}")).collect();
        assert!(verify(&archive, &format!("{sum}  hubness.tar.gz\n")).is_ok());
        assert!(verify(&archive, &format!("{}  hubness.tar.gz\n", "0".repeat(64))).is_err());
    }

    #[test]
    fn replaces_a_binary_only_when_the_new_one_runs() {
        let dir = std::env::temp_dir().join(format!("hubness-update-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("hubness");
        std::fs::write(&exe, "old").unwrap();
        assert!(install(&exe, b"#!/bin/sh\necho 'hubness 0.0.1 (full) x'\n", "9.9.9").is_err());
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "old");
        assert!(!dir.join(".hubness.new").exists());
        let new = b"#!/bin/sh\necho 'hubness 9.9.9 (full) x'\n";
        install(&exe, new, "9.9.9").unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), new);
        let _ = std::fs::remove_dir_all(dir);
    }
}
