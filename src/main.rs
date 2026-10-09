//! `ClipSync` - Cross-platform clipboard synchronization service
//!
//! This is the main entry point for the `ClipSync` daemon.

use anyhow::Result;
use clap::Parser;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use clipsync::cli::{Cli, CliHandler, Commands, ConfigAction};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Unit generation is a pure command: no config, clipboard, or logging side effects.
    if let Commands::PrintUserUnit { binary } = &cli.command {
        let path = binary
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Executable path must be UTF-8"))?;
        print!(
            "{}",
            clipsync::service_install::render_systemd_user_unit(path)
                .map_err(anyhow::Error::msg)?
        );
        return Ok(());
    }
    // Config initialization must also work when an existing config is invalid.
    if let Commands::Config {
        action: ConfigAction::Init { force },
    } = &cli.command
    {
        if let Some(path) = &cli.config {
            clipsync::config::Config::generate_example_config_at(path, *force).await?;
        } else {
            clipsync::config::Config::generate_example_config(*force).await?;
        }
        println!("Example configuration generated");
        return Ok(());
    }

    // Initialize logging
    let log_level = if cli.verbose { "debug" } else { "info" };
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| format!("clipsync={log_level}").into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    info!("ClipSync v{}", env!("CARGO_PKG_VERSION"));

    let mut handler = CliHandler::new(cli.config).await?;
    handler.handle_command(cli.command).await?;

    Ok(())
}
