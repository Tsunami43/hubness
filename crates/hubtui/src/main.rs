use clap::Parser;

/// hubtui — terminal UI for a hubness hub, on this machine or another one.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(flatten)]
    tui: hubness_tui::cli::Args,
}

fn main() -> anyhow::Result<()> {
    Cli::parse().tui.run()
}
