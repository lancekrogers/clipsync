//! Basic functionality tests to ensure the build is working

use clipsync::config::Config;

#[test]
fn test_version() {
    assert_eq!(clipsync::VERSION, "0.1.0");
}

#[test]
fn test_default_config() {
    let config = Config::default();
    assert!(!config.listen_addr.is_empty());
    assert!(!config.advertise_name.is_empty());
}

#[test]
fn test_max_payload_size() {
    assert_eq!(clipsync::MAX_PAYLOAD_SIZE, 5 * 1024 * 1024);
}

#[tokio::test]
async fn test_config_creation() {
    let config = Config::default();
    // Basic validation
    assert!(config.clipboard.max_size > 0);
    assert!(config.clipboard.history_size > 0);
    assert!(!config.auth.ssh_key.as_os_str().is_empty());
}

#[test]
fn test_config_paths() {
    let config = Config::default();

    // Check that paths are set
    assert!(!config.clipboard.history_db.as_os_str().is_empty());
    assert!(!config.auth.authorized_keys.as_os_str().is_empty());
}
