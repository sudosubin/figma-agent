mod config;
mod fonts;
mod routes;
mod server;
#[cfg(feature = "tls")]
mod tls;
mod util;

use anyhow::Result;
use clap::Parser;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(version, about = "Local font helper for Figma (Linux and macOS)")]
struct Cli {
    /// Path to a JSON config file (overrides default location).
    #[arg(long)]
    config: Option<std::path::PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("figma_agent=info")),
        )
        .init();
    let cli = Cli::parse();
    let config = config::Config::load(cli.config.as_deref())?;
    server::serve(config).await
}
