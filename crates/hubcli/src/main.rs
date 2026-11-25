mod service;

use clap::{Parser, Subcommand};

const EXAMPLES: &str = "\
Examples:
  hubcli server start          run the hub in the background (systemd user service)
  hubcli user add alice        create an account (needed to use the hub from elsewhere)
  hubcli server status         is it up? URL, data folder, accounts
  hubtui                       open the terminal UI (or the web app at the URL)

Docs: https://github.com/Tsunami43/hubness";

/// The `hubcli` command line.
#[derive(Parser)]
#[command(version, about = "hubness — a self-hosted hub for coding agents (web app, terminal UI, API)", after_help = EXAMPLES, disable_help_subcommand = true)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the hub in the foreground (HUBNESS_* variables and ./.env)
    #[command(after_help = "Settings: HUBNESS_HOST (127.0.0.1), HUBNESS_PORT (8787), HUBNESS_DATA_DIR (./.data).\n\
                            For a background service use `hubcli server start` instead.")]
    Serve,
    /// Run the hub as a background service: start | stop | restart | status | logs | config
    #[command(
        subcommand,
        after_help = "Examples:\n  hubcli server start\n  hubcli server start --data-dir ~/old-hub/.data\n  hubcli server logs -f\n  hubcli server config port 9000 && hubcli server restart"
    )]
    Server(service::ServerCommand),
    /// Manage hub accounts: add | passwd | rm | list
    #[command(subcommand)]
    User(UserCommand),
    /// Open the terminal UI (the same as `hubtui`)
    Tui(hubness_tui::cli::Args),
    /// The hub MCP server for agent sessions (stdio; started by the hub)
    #[command(hide = true)]
    Mcp,
    /// git credential helper for the hub's own git runs (started by git)
    #[command(name = "git-credential-helper", hide = true)]
    GitCredentialHelper {
        /// get | store | erase
        action: String,
    },
}

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
    match Cli::parse().command {
        Command::Serve => serve(),
        Command::Server(cmd) => service::run(cmd),
        Command::Mcp => mcp(),
        Command::Tui(args) => args.run(),
        Command::User(cmd) => user(cmd),
        Command::GitCredentialHelper { action } => git_credential_helper(&action),
    }
}

fn serve() -> anyhow::Result<()> {
    let config = hubness_server::Config::from_env()?;
    tokio::runtime::Builder::new_multi_thread().enable_all().build()?.block_on(hubness_server::serve(config))
}

fn user(cmd: UserCommand) -> anyhow::Result<()> {
    let args: Vec<String> = match cmd {
        UserCommand::Add { name } => vec!["add".into(), name],
        UserCommand::Passwd { name } => vec!["passwd".into(), name],
        UserCommand::Rm { name } => vec!["rm".into(), name],
        UserCommand::List => vec!["list".into()],
    };
    // accounts live where the service keeps its data, unless HUBNESS_DATA_DIR says otherwise
    if std::env::var_os("HUBNESS_DATA_DIR").is_none_or(|v| v.is_empty())
        && let Some(dir) = service::service_data_dir()
    {
        eprintln!("hubcli: data folder {} (from hub.env)", dir.display());
        // SAFETY: single-threaded at this point, before any runtime or thread starts
        unsafe { std::env::set_var("HUBNESS_DATA_DIR", dir) };
    }
    hubness_server::users::cli(&args)
}

fn mcp() -> anyhow::Result<()> {
    tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(hubness_server::mcp::run_stdio())
}

/// No runtime, no .env: it starts on every git request and only reads its environment.
fn git_credential_helper(action: &str) -> anyhow::Result<()> {
    Ok(hubness_server::git_auth::credential_helper(action, &mut std::io::stdin().lock(), &mut std::io::stdout().lock())?)
}
