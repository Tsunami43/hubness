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
    /// Terminal UI: chats with agents on a running hub (the same sessions as the web app).
    Tui {
        /// Hub URL.
        #[arg(long, env = "HUBNESS_URL", default_value = "http://127.0.0.1:8787")]
        url: String,
        /// Access token of a hub that requires one.
        #[arg(long, env = "HUBNESS_TOKEN", hide_env_values = true)]
        token: Option<String>,
    },
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
        // no runtime, no .env: it starts on every git request and only reads its environment
        Command::GitCredentialHelper { action } => {
            Ok(hubness_server::git_auth::credential_helper(&action, &mut std::io::stdin().lock(), &mut std::io::stdout().lock())?)
        }
        Command::Serve => tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?
            .block_on(hubness_server::serve(hubness_server::Config::from_env()?)),
        Command::Tui { url, token } => {
            tokio::runtime::Builder::new_multi_thread().enable_all().build()?.block_on(hubness_tui::run(&url, token.as_deref()))
        }
        Command::Token => {
            println!("{}", hubness_server::auth::generate()?);
            Ok(())
        }
        Command::Mcp => tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(hubness_server::mcp::run_stdio()),
    }
}
