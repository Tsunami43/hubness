use clap::{Parser, Subcommand};

/// hubcli — the hubness hub: a self-hosted hub for coding agents.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the hub server (settings come from HUBNESS_* variables and .env).
    Serve,
    /// The hub MCP server for agent sessions (stdio; started by the hub itself).
    Mcp,
    /// Terminal UI: chats with agents on a running hub (the same as the `hubtui` binary).
    Tui(hubness_tui::cli::Args),
    /// Print a new random access token for HUBNESS_TOKEN.
    Token,
    /// git credential helper for the hub's own git runs (started by git, not by hand).
    #[command(name = "git-credential-helper", hide = true)]
    GitCredentialHelper {
        /// get | store | erase
        action: String,
    },
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Serve => serve(),
        Command::Mcp => mcp(),
        Command::Tui(args) => args.run(),
        Command::Token => token(),
        Command::GitCredentialHelper { action } => git_credential_helper(&action),
    }
}

fn serve() -> anyhow::Result<()> {
    let config = hubness_server::Config::from_env()?;
    tokio::runtime::Builder::new_multi_thread().enable_all().build()?.block_on(hubness_server::serve(config))
}

fn mcp() -> anyhow::Result<()> {
    tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(hubness_server::mcp::run_stdio())
}

fn token() -> anyhow::Result<()> {
    println!("{}", hubness_server::auth::generate()?);
    Ok(())
}

/// No runtime, no .env: it starts on every git request and only reads its environment.
fn git_credential_helper(action: &str) -> anyhow::Result<()> {
    Ok(hubness_server::git_auth::credential_helper(action, &mut std::io::stdin().lock(), &mut std::io::stdout().lock())?)
}
