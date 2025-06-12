use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use clap::Parser;
use tempfile::TempDir;

use clipsync::{
    cli::{Cli, CliHandler, Commands, ConfigAction},
    config::Config,
};

#[tokio::test]
async fn test_cli_parsing() -> Result<()> {
    // Test basic command parsing
    let cli = Cli::try_parse_from(&["clipsync", "status"])?;
    assert!(matches!(cli.command, Commands::Status));

    let cli = Cli::try_parse_from(&["clipsync", "start", "--foreground"])?;
    assert!(matches!(cli.command, Commands::Start { foreground: true }));

    let cli = Cli::try_parse_from(&["clipsync", "history", "--limit", "5"])?;
    assert!(matches!(
        cli.command,
        Commands::History {
            limit: 5,
            interactive: false
        }
    ));

    Ok(())
}

#[tokio::test]
async fn test_cli_handler_creation() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let config_path = temp_dir.path().join("config.toml");

    // Create a test config
    let config = Config::default_with_path(config_path.clone());
    config.save().await?;

    // Test CLI handler creation
    let handler = CliHandler::new(Some(config_path)).await?;

    // Handler should be created successfully
    // This tests that all dependencies can be initialized

    Ok(())
}

#[tokio::test]
async fn test_status_command() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let config_path = temp_dir.path().join("config.toml");

    let config = Config::default_with_path(config_path.clone());
    config.save().await?;

    let mut handler = CliHandler::new(Some(config_path)).await?;

    // Test status command
    let result = handler.handle_command(Commands::Status).await;
    assert!(result.is_ok());

    Ok(())
}

#[tokio::test]
async fn test_copy_paste_commands() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let config_path = temp_dir.path().join("config.toml");

    let config = Config::default_with_path(config_path.clone());
    config.save().await?;

    let mut handler = CliHandler::new(Some(config_path)).await?;

    // Test copy command
    let copy_result = handler
        .handle_command(Commands::Copy {
            text: "Test clipboard content".to_string(),
        })
        .await;

    // Copy might fail if no clipboard provider is available in test environment
    // but the command structure should be valid

    // Test paste command
    let paste_result = handler.handle_command(Commands::Paste).await;

    // Both commands should complete without panicking
    // Actual clipboard operations may fail in headless test environment

    Ok(())
}

#[tokio::test]
async fn test_history_command() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let config_path = temp_dir.path().join("config.toml");

    let config = Config::default_with_path(config_path.clone());
    config.save().await?;

    let mut handler = CliHandler::new(Some(config_path)).await?;

    // Test history command
    let result = handler
        .handle_command(Commands::History {
            limit: 10,
            interactive: false,
        })
        .await;

    assert!(result.is_ok());

    Ok(())
}

#[tokio::test]
async fn test_config_commands() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let config_path = temp_dir.path().join("config.toml");

    let config = Config::default_with_path(config_path.clone());
    config.save().await?;

    let mut handler = CliHandler::new(Some(config_path)).await?;

    // Test config show
    let show_result = handler
        .handle_command(Commands::Config {
            action: ConfigAction::Show,
        })
        .await;
    assert!(show_result.is_ok());

    // Test config validate
    let validate_result = handler
        .handle_command(Commands::Config {
            action: ConfigAction::Validate,
        })
        .await;
    assert!(validate_result.is_ok());

    Ok(())
}

#[tokio::test]
async fn test_peers_command() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let config_path = temp_dir.path().join("config.toml");

    let config = Config::default_with_path(config_path.clone());
    config.save().await?;

    let mut handler = CliHandler::new(Some(config_path)).await?;

    // Test peers command (should show no peers when daemon not running)
    let result = handler.handle_command(Commands::Peers).await;
    assert!(result.is_ok());

    Ok(())
}

#[tokio::test]
async fn test_sync_command() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let config_path = temp_dir.path().join("config.toml");

    let config = Config::default_with_path(config_path.clone());
    config.save().await?;

    let mut handler = CliHandler::new(Some(config_path)).await?;

    // Test sync command (should handle gracefully when daemon not running)
    let result = handler.handle_command(Commands::Sync).await;
    assert!(result.is_ok());

    Ok(())
}

#[tokio::test]
async fn test_verbose_flag() -> Result<()> {
    let cli = Cli::try_parse_from(&["clipsync", "--verbose", "status"])?;
    assert!(cli.verbose);

    let cli = Cli::try_parse_from(&["clipsync", "status"])?;
    assert!(!cli.verbose);

    Ok(())
}

#[tokio::test]
async fn test_config_flag() -> Result<()> {
    let cli = Cli::try_parse_from(&["clipsync", "--config", "/path/to/config.toml", "status"])?;

    assert_eq!(cli.config, Some(PathBuf::from("/path/to/config.toml")));

    Ok(())
}

#[tokio::test]
async fn test_daemon_flag() -> Result<()> {
    let cli = Cli::try_parse_from(&["clipsync", "--daemon", "start"])?;
    assert!(cli.daemon);

    Ok(())
}

// Test error handling for invalid commands
#[tokio::test]
async fn test_invalid_commands() {
    // Test invalid subcommand
    let result = Cli::try_parse_from(&["clipsync", "invalid"]);
    assert!(result.is_err());

    // Test invalid flag
    let result = Cli::try_parse_from(&["clipsync", "--invalid-flag", "status"]);
    assert!(result.is_err());
}

#[tokio::test]
async fn test_help_output() -> Result<()> {
    // Test that help can be generated
    let cli = Cli::command();
    let help = cli.render_help();

    assert!(help.to_string().contains("ClipSync"));
    assert!(help.to_string().contains("start"));
    assert!(help.to_string().contains("status"));
    assert!(help.to_string().contains("history"));

    Ok(())
}
