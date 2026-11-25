//! `hubness`: the one command of hubness.
//!
//! The full build (default feature `server`) runs and manages the hub; the client build
//! (`--no-default-features`) has no server code: the TUI, `webapp` and `update` only.

mod hubenv;
#[cfg(feature = "server")]
mod service;
mod update;
mod webapp;

use clap::{Parser, Subcommand};

#[cfg(feature = "server")]
const EXAMPLES: &str = "\
Examples:
  hubness server start         run the hub in the background (systemd user service)
  hubness user add alice       create an account (needed to use the hub from elsewhere)
  hubness server status        is it up? URL, data folder, accounts
  hubness tui                  open the terminal UI
  hubness webapp               open the web app in the browser
  hubness update               install the latest release

Docs: https://github.com/Tsunami43/hubness";

#[cfg(not(feature = "server"))]
const EXAMPLES: &str = "\
Examples:
  hubness tui                  open the terminal UI (asks for the hub's address the first time)
  hubness tui --url hub.example.com
  hubness webapp               open the web app in the browser
  hubness update               install the latest release

This is the client build: it connects to a hub that runs elsewhere.
Docs: https://github.com/Tsunami43/hubness";

/// The `hubness` command line.
#[derive(Parser)]
#[command(
    name = "hubness",
    version = concat!(env!("CARGO_PKG_VERSION"), " (", env!("HUBNESS_FLAVOR"), ") ", env!("HUBNESS_TARGET")),
    about = "hubness — a self-hosted hub for coding agents (web app, terminal UI, API)",
    after_help = EXAMPLES,
    disable_help_subcommand = true
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the hub in the foreground (HUBNESS_* variables and ./.env)
    #[cfg(feature = "server")]
    #[command(after_help = "Settings: HUBNESS_HOST (127.0.0.1), HUBNESS_PORT (8787), HUBNESS_DATA_DIR (./.data).\n\
                            For a background service use `hubness server start` instead.")]
    Serve,
    /// Run the hub as a background service: start | stop | restart | status | logs | config
    #[cfg(feature = "server")]
    #[command(
        subcommand,
        after_help = "Examples:\n  hubness server start\n  hubness server start --data-dir ~/old-hub/.data\n  hubness server logs -f\n  hubness server config port 9000 && hubness server restart"
    )]
    Server(service::ServerCommand),
    /// Manage hub accounts: add | passwd | rm | list
    #[cfg(feature = "server")]
    #[command(subcommand)]
    User(UserCommand),
    /// Open the terminal UI
    Tui(hubness_tui::cli::Args),
    /// Open the hub's web app in the browser
    Webapp(webapp::Args),
    /// Update hubness to the latest release (or --version X)
    Update(update::Args),
    /// The hub MCP server for agent sessions (stdio; started by the hub)
    #[cfg(feature = "server")]
    #[command(hide = true)]
    Mcp,
    /// git credential helper for the hub's own git runs (started by git)
    #[cfg(feature = "server")]
    #[command(name = "git-credential-helper", hide = true)]
    GitCredentialHelper {
        /// get | store | erase
        action: String,
    },
    /// Server commands of the full build: explained instead of "unrecognized subcommand"
    #[cfg(not(feature = "server"))]
    #[command(external_subcommand)]
    Other(Vec<String>),
}

#[cfg(feature = "server")]
#[derive(Subcommand)]
#[command(after_help = "Passwords are asked for (hidden), or read from stdin when it is not a terminal.\n\
                        Works on the service's data folder (or HUBNESS_DATA_DIR), also while the hub runs.")]
enum UserCommand {
    /// Create an account (the first one turns sign-in on)
    Add { name: String },
    /// Set a new password (signs the account out everywhere)
    Passwd { name: String },
    /// Delete an account
    Rm { name: String },
    /// List the accounts
    List,
}

fn main() -> anyhow::Result<()> {
    // `--version` also mentions a newer release when the cached daily check knows one
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() == 1 && (args[0] == "--version" || args[0] == "-V") {
        println!("{}", update::long_version());
        if let Some(latest) = update::cached_newer(false) {
            println!("hubness {latest} is available: hubness update");
        }
        return Ok(());
    }
    match Cli::parse().command {
        #[cfg(feature = "server")]
        Command::Serve => serve(),
        #[cfg(feature = "server")]
        Command::Server(cmd) => service::run(cmd),
        #[cfg(feature = "server")]
        Command::Mcp => mcp(),
        #[cfg(feature = "server")]
        Command::User(cmd) => user(cmd),
        #[cfg(feature = "server")]
        Command::GitCredentialHelper { action } => git_credential_helper(&action),
        Command::Tui(args) => args.run(),
        Command::Webapp(args) => webapp::run(args),
        Command::Update(args) => update::run(args),
        #[cfg(not(feature = "server"))]
        Command::Other(args) => client_only(&args),
    }
}

#[cfg(not(feature = "server"))]
fn client_only(args: &[String]) -> anyhow::Result<()> {
    let cmd = args.first().map(String::as_str).unwrap_or_default();
    if matches!(cmd, "serve" | "server" | "user" | "mcp" | "git-credential-helper") {
        eprintln!("this is the client build of hubness: it has no `{cmd}` command. Install the full one:");
        eprintln!("  {}", update::INSTALL_FULL);
    } else {
        eprintln!("error: unrecognized subcommand '{cmd}' (see hubness --help)");
    }
    std::process::exit(2);
}

#[cfg(feature = "server")]
fn serve() -> anyhow::Result<()> {
    let config = hubness_server::Config::from_env()?;
    tokio::runtime::Builder::new_multi_thread().enable_all().build()?.block_on(hubness_server::serve(config))
}

#[cfg(feature = "server")]
fn user(cmd: UserCommand) -> anyhow::Result<()> {
    let args: Vec<String> = match cmd {
        UserCommand::Add { name } => vec!["add".into(), name],
        UserCommand::Passwd { name } => vec!["passwd".into(), name],
        UserCommand::Rm { name } => vec!["rm".into(), name],
        UserCommand::List => vec!["list".into()],
    };
    // accounts live where the service keeps its data, unless HUBNESS_DATA_DIR says otherwise
    if std::env::var_os("HUBNESS_DATA_DIR").is_none_or(|v| v.is_empty())
        && let Some(dir) = hubenv::service_data_dir()
    {
        eprintln!("hubness: data folder {} (from hub.env)", dir.display());
        // SAFETY: single-threaded at this point, before any runtime or thread starts
        unsafe { std::env::set_var("HUBNESS_DATA_DIR", dir) };
    }
    hubness_server::users::cli(&args)
}

#[cfg(feature = "server")]
fn mcp() -> anyhow::Result<()> {
    tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(hubness_server::mcp::run_stdio())
}

/// No runtime, no .env: it starts on every git request and only reads its environment.
#[cfg(feature = "server")]
fn git_credential_helper(action: &str) -> anyhow::Result<()> {
    Ok(hubness_server::git_auth::credential_helper(action, &mut std::io::stdin().lock(), &mut std::io::stdout().lock())?)
}
