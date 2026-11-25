//! Command-line arguments of `hubness tui`, shared by the full and the client build so both
//! take the same flags and only this file changes when they do.

/// Connection settings of the terminal UI.
#[derive(Debug, Clone, clap::Args)]
pub struct Args {
    /// Hub address: a URL, a domain or an IP (with :port); saved for the next start.
    /// Without it the last hub opens, or the first run asks for one
    #[arg(long, env = "HUBNESS_URL", value_name = "ADDRESS")]
    pub url: Option<String>,
    /// Username to prefill on the sign-in screen of a hub with accounts
    #[arg(long)]
    pub user: Option<String>,
}

impl Args {
    /// Start a runtime and run the TUI until the user quits.
    pub fn run(self) -> anyhow::Result<()> {
        tokio::runtime::Builder::new_multi_thread().enable_all().build()?.block_on(crate::run(self.url.as_deref(), self.user.as_deref()))
    }
}
