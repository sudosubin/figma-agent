mod server;
#[cfg(feature = "tls")]
mod tls;

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
    let _cli = Cli::parse();
    tracing::info!("starting figma-agent v{}", env!("CARGO_PKG_VERSION"));
    server::serve().await
}
