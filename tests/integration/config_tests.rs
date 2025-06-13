//! Integration tests for configuration module

use clipsync::config::{Config, ConfigError};
use std::fs;
use tempfile::TempDir;

#[test]
fn test_config_defaults() {
    let config = Config::default();

    assert_eq!(config.listen_addr, ":8484");
    assert!(config.advertise_name.contains("-clipsync"));
    assert_eq!(config.clipboard.max_size, 5_242_880);
    assert_eq!(config.clipboard.history_size, 20);
    assert!(config.clipboard.sync_primary);
    assert_eq!(config.log_level, "info");
}

#[test]
fn test_config_from_toml() {
    let toml_str = r#"
        listen_addr = ":9090"
        advertise_name = "test-machine"
        log_level = "debug"

        [auth]
        ssh_key = "~/.ssh/test_key"

        [clipboard]
        max_size = 2_097_152
        sync_primary = false
        history_size = 50

        [hotkeys]
        toggle_sync = "Ctrl+Alt+S"

        [security]
        encryption = "aes-256-gcm"
        compression = "zstd"
    "#;

    let config = Config::from_toml(toml_str).unwrap();

    assert_eq!(config.listen_addr, ":9090");
    assert_eq!(config.advertise_name, "test-machine");
    assert_eq!(config.log_level, "debug");
    assert_eq!(config.clipboard.max_size, 2_097_152);
    assert!(!config.clipboard.sync_primary);
    assert_eq!(config.clipboard.history_size, 50);
    assert_eq!(config.hotkeys.toggle_sync, "Ctrl+Alt+S");
}

#[test]
fn test_config_validation_max_size_too_small() {
    let toml_str = r#"
        [clipboard]
        max_size = 500
    "#;

    let result = Config::from_toml(toml_str);
    assert!(matches!(result, Err(ConfigError::Validation(_))));
}

#[test]
fn test_config_validation_max_size_too_large() {
    let toml_str = r#"
        [clipboard]
        max_size = 60_000_000
    "#;

    let result = Config::from_toml(toml_str);
    assert!(matches!(result, Err(ConfigError::Validation(_))));
}

#[test]
fn test_config_validation_history_size() {
    // Too small
    let toml_str = r#"
        [clipboard]
        history_size = 0
    "#;

    let result = Config::from_toml(toml_str);
    assert!(matches!(result, Err(ConfigError::Validation(_))));

    // Too large
    let toml_str = r#"
        [clipboard]
        history_size = 200
    "#;

    let result = Config::from_toml(toml_str);
    assert!(matches!(result, Err(ConfigError::Validation(_))));
}

#[test]
fn test_config_save_and_load() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("config.toml");

    // Create a config
    let mut config = Config::default();
    config.listen_addr = ":7777".to_string();
    config.clipboard.max_size = 1_000_000;

    // Save to temp file
    let toml_string = toml::to_string_pretty(&config).unwrap();
    fs::write(&config_path, toml_string).unwrap();

    // Load from file
    let loaded = Config::load_from_path(&config_path).unwrap();

    assert_eq!(loaded.listen_addr, ":7777");
    assert_eq!(loaded.clipboard.max_size, 1_000_000);
}

#[test]
fn test_config_generate_example() {
    let example = Config::generate_example();

    assert!(example.contains("ClipSync Configuration File"));
    assert!(example.contains("listen_addr"));
    assert!(example.contains("[auth]"));
    assert!(example.contains("[clipboard]"));
    assert!(example.contains("[hotkeys]"));
    assert!(example.contains("[security]"));
}

#[test]
fn test_config_platform_specific_hotkeys() {
    let config = Config::default();

    #[cfg(target_os = "macos")]
    assert!(config.hotkeys.toggle_sync.contains("Cmd"));

    #[cfg(not(target_os = "macos"))]
    assert!(config.hotkeys.toggle_sync.contains("Alt"));
}