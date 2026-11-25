use clap::{Parser, Subcommand};

/// hubness — a self-hosted hub for coding agents.
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
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Serve => hubness_server::serve(hubness_server::Config::from_env()?).await,
        Command::Mcp => hubness_server::mcp::run_stdio().await,
    }
}
