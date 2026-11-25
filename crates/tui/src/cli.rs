//! Command-line arguments of the TUI, shared by `hubtui` and `hubcli tui` so both binaries
//! take the same flags and only this file changes when they do.

/// Connection settings of the terminal UI.
#[derive(Debug, Clone, clap::Args)]
pub struct Args {
    /// Hub URL.
    #[arg(long, env = "HUBNESS_URL", default_value = "http://127.0.0.1:8787")]
    pub url: String,
    /// Access token of a hub that requires one.
    #[arg(long, env = "HUBNESS_TOKEN", hide_env_values = true)]
    pub token: Option<String>,
}

impl Args {
    /// Start a runtime and run the TUI until the user quits.
    pub fn run(self) -> anyhow::Result<()> {
        tokio::runtime::Builder::new_multi_thread().enable_all().build()?.block_on(crate::run(&self.url, self.token.as_deref()))
    }
}
