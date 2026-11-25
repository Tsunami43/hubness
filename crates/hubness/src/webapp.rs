//! `hubness webapp`: open the hub's web app in the browser.
//!
//! The URL comes from `--url`, else the local service settings (hub.env), else the hub the
//! TUI opened last, else the default local address. `HUBNESS_OPENER` replaces the system
//! opener (`xdg-open`, `open` on macOS); tests point it at a fake script.

use std::process::{Command, Stdio};

use anyhow::Context;

use crate::hubenv::{DEFAULT_HOST, DEFAULT_PORT, EnvFile, Paths, local_url};

#[derive(clap::Args)]
pub struct Args {
    /// Hub address (a URL, a domain or an IP with :port) instead of the local or last one
    #[arg(long, value_name = "ADDRESS")]
    url: Option<String>,
    /// Only print the URL, don't open a browser
    #[arg(long)]
    print: bool,
}

pub fn run(args: Args) -> anyhow::Result<()> {
    let url = match args.url.as_deref() {
        Some(u) => normalize(u),
        None => default_url(),
    };
    if args.print {
        println!("{url}");
        return Ok(());
    }
    let opener = std::env::var("HUBNESS_OPENER")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| if cfg!(target_os = "macos") { "open" } else { "xdg-open" }.into());
    println!("opening {url}");
    let status = Command::new(&opener)
        .arg(&url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .and_then(|mut child| child.wait())
        .with_context(|| format!("can't run {opener} to open a browser; open {url} yourself"))?;
    if !status.success() {
        anyhow::bail!("{opener} failed to open a browser; open {url} yourself");
    }
    Ok(())
}

/// hub.env of the local service, then the TUI's last hub, then the default address.
fn default_url() -> String {
    if let Ok(paths) = Paths::from_env()
        && paths.env.exists()
        && let Ok(env) = EnvFile::load(&paths.env)
    {
        return local_url(&env);
    }
    if let Some(last) = hubness_tui::hubs::Store::load().last {
        return last;
    }
    format!("http://{DEFAULT_HOST}:{DEFAULT_PORT}")
}

/// `hub.example.com` → `https://hub.example.com`; local and IP addresses get `http://` and
/// the default port, like the TUI does for a typed address.
fn normalize(input: &str) -> String {
    let input = input.trim().trim_end_matches('/');
    if input.contains("://") {
        return input.to_string();
    }
    let host = input.split(':').next().unwrap_or(input);
    let local = host == "localhost" || host.parse::<std::net::IpAddr>().is_ok();
    if local {
        if input.contains(':') { format!("http://{input}") } else { format!("http://{input}:{DEFAULT_PORT}") }
    } else {
        format!("https://{input}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_become_urls() {
        assert_eq!(normalize("http://10.0.0.2:9000/"), "http://10.0.0.2:9000");
        assert_eq!(normalize("127.0.0.1"), "http://127.0.0.1:8787");
        assert_eq!(normalize("localhost:9000"), "http://localhost:9000");
        assert_eq!(normalize("hub.example.com"), "https://hub.example.com");
    }
}
