//! ClipSync - Cross-platform clipboard synchronization service
//!
//! This is the main entry point for the ClipSync daemon.

use anyhow::Result;
use clap::Parser;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use clipsync::cli::{Cli, CliHandler};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize logging
    let log_level = if cli.verbose { "debug" } else { "info" };
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| format!("clipsync={}", log_level).into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    info!("ClipSync v{}", env!("CARGO_PKG_VERSION"));

    let mut handler = CliHandler::new(cli.config).await?;
    handler.handle_command(cli.command).await?;

    Ok(())
}
