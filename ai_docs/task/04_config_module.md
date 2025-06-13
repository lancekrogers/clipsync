# Task 04: Configuration Module

## Objective
Implement the configuration system for loading and managing ClipSync settings.

## Steps

1. **Create src/config/mod.rs**
   - Define Config struct matching TOML schema
   - Implement loading from file
   - Handle default values
   - Validate configuration

2. **Configuration structure**
   ```rust
   use serde::{Deserialize, Serialize};
   use std::path::PathBuf;
   
   #[derive(Debug, Clone, Serialize, Deserialize)]
   pub struct Config {
       pub listen_addr: String,
       pub advertise_name: String,
       pub auth: AuthConfig,
       pub clipboard: ClipboardConfig,
       pub hotkeys: HotkeyConfig,
       pub security: SecurityConfig,
       pub log_level: String,
   }
   
   #[derive(Debug, Clone, Serialize, Deserialize)]
   pub struct AuthConfig {
       pub ssh_key: PathBuf,
       pub authorized_keys: PathBuf,
   }
   
   #[derive(Debug, Clone, Serialize, Deserialize)]
   pub struct ClipboardConfig {
       pub max_size: usize,
       pub sync_primary: bool,
       pub history_size: usize,
       pub history_db: PathBuf,
   }
   ```

3. **Implement config loading**
   - Check multiple paths: CLI arg, env var, default locations
   - Expand ~ in paths
   - Create default config if none exists

4. **Add config validation**
   - Verify SSH key exists and is readable
   - Check max_size is reasonable (1KB - 50MB)
   - Validate network addresses

5. **Create example config**
   - Generate example.toml with all options documented

## Success Criteria
- Config loads from TOML file
- Defaults are sensible
- Validation prevents misconfiguration
- Path expansion works correctly