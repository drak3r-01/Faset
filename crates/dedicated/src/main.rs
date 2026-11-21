//! `dedicated`: headless Faset server. See [`dedicated::Cli`] for the options.
use anyhow::Result;
use clap::Parser;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let cli = dedicated::Cli::parse();
    dedicated::run(&cli)
}
