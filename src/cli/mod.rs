use std::path::PathBuf;
use std::sync::Arc;
use std::io::{BufRead, BufReader};
use std::fs::File;
use std::process::Command;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing::{error, info};
use chrono::{DateTime, Local};
use serde_json;

use crate::adapters::{
    get_clipboard_provider, ClipboardProviderWrapper, HistoryManager, PeerDiscovery,
};
use crate::auth::{AuthorizedKey, AuthorizedKeys, PublicKey};
use crate::config::Config;
#[cfg(target_os = "linux")]
use crate::daemon;
// use crate::hotkey::HotKeyManager; // Removed - we work with system clipboard
use crate::sync::{SyncEngine, TrustAwareSyncEngine};
use crate::transport::{TransportConfig, TransportManager};

pub mod commands;
pub mod history_picker;

#[derive(Parser)]
#[command(name = "clipsync")]
#[command(about = "Cross-platform clipboard synchronization service")]
#[command(version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    #[arg(short, long)]
    pub config: Option<PathBuf>,

    #[arg(short, long)]
    pub verbose: bool,

    #[arg(short, long)]
    pub daemon: bool,
}

#[derive(Subcommand)]
pub enum Commands {
    #[command(about = "Start the ClipSync daemon")]
    Start {
        #[arg(short, long)]
        foreground: bool,
    },

    #[command(about = "Stop the ClipSync daemon")]
    Stop,

    #[command(about = "Get the status of ClipSync")]
    Status,

    #[command(about = "Show clipboard history")]
    History {
        #[arg(short, long, default_value = "10")]
        limit: usize,

        #[arg(short, long)]
        interactive: bool,

        #[arg(long)]
        search: Option<String>,
    },

    #[command(about = "Force synchronization with peers")]
    Sync,

    #[command(about = "List connected peers")]
    Peers {
        #[arg(long)]
        discover: bool,
    },

    #[command(about = "Copy text to clipboard")]
    Copy { text: String },

    #[command(about = "Get current clipboard content")]
    Paste,

    #[command(about = "Restart the ClipSync daemon")]
    Restart,

    #[command(about = "Clear clipboard content")]
    Clear,

    #[command(about = "Show version information")]
    Version,

    #[command(about = "Configuration management")]
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },

    #[command(about = "Authentication and authorization management")]
    Auth {
        #[command(subcommand)]
        action: AuthAction,
    },

    #[command(about = "Run connectivity diagnostics")]
    Doctor,

    #[command(about = "Show recent log entries")]
    Logs {
        #[arg(long, default_value = "50")]
        limit: usize,

        #[arg(long)]
        follow: bool,
    },
}

#[derive(Subcommand)]
pub enum ConfigAction {
    #[command(about = "Show current configuration")]
    Show,

    #[command(about = "Generate example configuration")]
    Init {
        #[arg(long)]
        force: bool,
    },

    #[command(about = "Validate configuration")]
    Validate,

    #[command(about = "Edit configuration file")]
    Edit,
}

#[derive(Subcommand)]
pub enum AuthAction {
    #[command(about = "Add authorized device")]
    Add {
        /// Public key in OpenSSH format or path to public key file
        public_key: String,
        /// Optional name/comment for the key
        #[arg(long)]
        name: Option<String>,
    },

    #[command(about = "List authorized keys")]
    List,

    #[command(about = "Remove device access by key ID/fingerprint")]
    Remove {
        /// Key fingerprint or comment to remove
        key_id: String,
    },
}

pub struct CliHandler {
    config: Arc<Config>,
    config_path: Option<PathBuf>,
    clipboard: Option<Arc<ClipboardProviderWrapper>>,
    history: Option<Arc<HistoryManager>>,
    discovery: Option<Arc<PeerDiscovery>>,
    transport: Option<Arc<TransportManager>>,
    sync_engine: Option<Arc<TrustAwareSyncEngine>>,
}

impl CliHandler {
    pub async fn new(config_path: Option<PathBuf>) -> Result<Self> {
        let config = Arc::new(Config::load_config(config_path.clone()).await?);

        Ok(Self {
            config,
            config_path,
            clipboard: None,
            history: None,
            discovery: None,
            transport: None,
            sync_engine: None,
        })
    }

    /// Get the resolved config path for validation
    fn get_config_path_for_validation(&self) -> Option<PathBuf> {
        if let Some(ref path) = self.config_path {
            // CLI explicitly specified a config path
            Some(path.clone())
        } else {
            // Use the same logic as Config::find_config_path()
            // Check environment variable first
            if let Ok(path) = std::env::var("CLIPSYNC_CONFIG") {
                let path = PathBuf::from(path);
                if path.exists() {
                    return Some(path);
                }
            }

            // Check default location
            dirs::config_dir()
                .map(|p| p.join("clipsync").join("config.toml"))
                .filter(|p| p.exists())
        }
    }

    /// Lazily initialize the history manager when needed
    async fn ensure_history(&mut self) -> Result<Arc<HistoryManager>> {
        if self.history.is_none() {
            info!("Initializing history manager");
            let history = Arc::new(HistoryManager::new(&self.config.database_path()).await?);
            self.history = Some(history);
        }
        Ok(self.history.as_ref().unwrap().clone())
    }

    /// Lazily initialize the clipboard provider when needed
    async fn ensure_clipboard(&mut self) -> Result<Arc<ClipboardProviderWrapper>> {
        if self.clipboard.is_none() {
            info!("Initializing clipboard provider");
            let clipboard = Arc::new(get_clipboard_provider().await?);
            self.clipboard = Some(clipboard);
        }
        Ok(self.clipboard.as_ref().unwrap().clone())
    }

    /// Lazily initialize the discovery service when needed
    async fn ensure_discovery(&mut self) -> Result<Arc<PeerDiscovery>> {
        if self.discovery.is_none() {
            info!("Initializing discovery service");
            let discovery = Arc::new(PeerDiscovery::new(self.config.clone()).await?);
            self.discovery = Some(discovery);
        }
        Ok(self.discovery.as_ref().unwrap().clone())
    }

    /// Lazily initialize the transport manager when needed
    async fn ensure_transport(&mut self) -> Result<Arc<TransportManager>> {
        if self.transport.is_none() {
            info!("Initializing transport manager");
            let transport = Arc::new(TransportManager::new(TransportConfig::default()));
            self.transport = Some(transport);
        }
        Ok(self.transport.as_ref().unwrap().clone())
    }

    pub async fn handle_command(&mut self, command: Commands) -> Result<()> {
        match command {
            Commands::Start { foreground } => self.start_daemon(foreground).await,
            Commands::Stop => self.stop_daemon().await,
            Commands::Status => self.show_status().await,
            Commands::History { limit, interactive, search } => {
                if interactive {
                    self.show_interactive_history().await
                } else if let Some(search_term) = search {
                    self.search_history(&search_term, limit).await
                } else {
                    self.show_history(limit).await
                }
            }
            Commands::Sync => self.force_sync().await,
            Commands::Peers { discover } => {
                if discover {
                    self.discover_peers().await
                } else {
                    self.show_peers().await
                }
            }
            Commands::Copy { text } => self.copy_text(text).await,
            Commands::Paste => self.paste_text().await,
            Commands::Restart => self.restart_daemon().await,
            Commands::Clear => self.clear_clipboard().await,
            Commands::Version => self.show_version().await,
            Commands::Config { action } => self.handle_config_action(action).await,
            Commands::Auth { action } => self.handle_auth_action(action).await,
            Commands::Doctor => self.run_diagnostics().await,
            Commands::Logs { limit, follow } => {
                if follow {
                    self.follow_logs().await
                } else {
                    self.show_logs(limit).await
                }
            }
        }
    }

    async fn start_daemon(&mut self, foreground: bool) -> Result<()> {
        info!("Starting ClipSync daemon");

        #[cfg(target_os = "linux")]
        {
            // Check if daemon is already running
            if daemon::is_daemon_running()? {
                println!("ClipSync daemon is already running");
                return Ok(());
            }

            if !foreground {
                info!("Running in daemon mode");
                daemon::daemonize()?;
            } else {
                info!("Running in foreground mode");
                daemon::run_foreground()?;
            }
        }

        #[cfg(target_os = "macos")]
        {
            if !foreground {
                // On macOS, use launchctl to start the service
                let plist_path = dirs::home_dir()
                    .ok_or_else(|| anyhow::anyhow!("Could not determine home directory"))?
                    .join("Library/LaunchAgents/com.clipsync.plist");

                if !plist_path.exists() {
                    println!("ClipSync LaunchAgent not installed.");
                    println!("Please run 'make install-user' first to install the service.");
                    return Err(anyhow::anyhow!("LaunchAgent not installed"));
                }

                // Check if already loaded
                let status_output = Command::new("launchctl")
                    .args(&["list", "com.clipsync"])
                    .output()?;

                if status_output.status.success() {
                    println!("ClipSync daemon is already running");
                    return Ok(());
                }

                // Load and start the LaunchAgent
                println!("Starting ClipSync daemon via launchctl...");
                let load_output = Command::new("launchctl")
                    .args(&["load", "-w", plist_path.to_str().unwrap()])
                    .output()?;

                if !load_output.status.success() {
                    let error_msg = String::from_utf8_lossy(&load_output.stderr);
                    return Err(anyhow::anyhow!("Failed to start daemon: {}", error_msg));
                }

                println!("ClipSync daemon started successfully");
                println!("Use 'clipsync status' to check if it's running");
                return Ok(());
            }
        }

        #[cfg(all(not(target_os = "linux"), not(target_os = "macos")))]
        {
            if !foreground {
                println!("Warning: Daemon mode not supported on this platform, running in foreground");
            }
        }

        // Ensure all components are initialized for daemon mode
        let clipboard = self.ensure_clipboard().await?;
        let history = self.ensure_history().await?;
        let discovery = self.ensure_discovery().await?;
        let transport = self.ensure_transport().await?;

        // Initialize trust-aware sync engine
        let sync_engine = Arc::new(
            TrustAwareSyncEngine::new(
                Arc::clone(&self.config),
                Arc::clone(&clipboard),
                Arc::clone(&history),
                Arc::clone(&discovery),
                Arc::clone(&transport),
            )
            .await?,
        );

        // Skip hotkey registration - we want to work with system clipboard, not override it
        // let mut hotkey_manager = HotKeyManager::new(
        //     Arc::clone(&self.config),
        //     Arc::clone(&clipboard),
        //     Arc::clone(&history),
        // )?;
        //
        // hotkey_manager.set_sync_engine(Arc::clone(&sync_engine));
        // hotkey_manager.register_default_hotkeys().await?;

        self.sync_engine = Some(Arc::clone(&sync_engine));

        // Start trust processing
        sync_engine
            .start_trust_processing(Arc::clone(&discovery))
            .await?;

        // Start services
        let sync_engine_task = Arc::clone(&sync_engine);

        info!("ClipSync daemon started successfully");

        // Setup signal handler for graceful shutdown
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        #[cfg(target_os = "linux")]
        daemon::setup_signal_handlers(shutdown_tx)?;
        
        #[cfg(not(target_os = "linux"))]
        {
            // For non-Linux platforms, we'll handle Ctrl+C manually
            let shutdown_tx = Arc::new(tokio::sync::Mutex::new(Some(shutdown_tx)));
            tokio::spawn(async move {
                tokio::signal::ctrl_c().await.ok();
                if let Some(tx) = shutdown_tx.lock().await.take() {
                    let _ = tx.send(());
                }
            });
        }

        // Run services until shutdown signal
        tokio::select! {
            result = sync_engine_task.start() => {
                if let Err(e) = result {
                    error!("Service error: {}", e);
                }
            }
            _ = shutdown_rx => {
                info!("Received shutdown signal");
            }
        }

        // Cleanup
        #[cfg(target_os = "linux")]
        daemon::remove_pidfile()?;
        info!("ClipSync daemon stopped");

        Ok(())
    }

    async fn stop_daemon(&self) -> Result<()> {
        info!("Stopping ClipSync daemon");
        #[cfg(target_os = "linux")]
        {
            daemon::stop_daemon()?;
            println!("ClipSync daemon stopped");
        }
        #[cfg(target_os = "macos")]
        {
            // Check if service is loaded
            let status_output = Command::new("launchctl")
                .args(&["list", "com.clipsync"])
                .output()?;

            if !status_output.status.success() {
                println!("ClipSync daemon is not running");
                return Ok(());
            }

            // Unload the LaunchAgent
            let plist_path = dirs::home_dir()
                .ok_or_else(|| anyhow::anyhow!("Could not determine home directory"))?
                .join("Library/LaunchAgents/com.clipsync.plist");

            let unload_output = Command::new("launchctl")
                .args(&["unload", plist_path.to_str().unwrap()])
                .output()?;

            if !unload_output.status.success() {
                let error_msg = String::from_utf8_lossy(&unload_output.stderr);
                return Err(anyhow::anyhow!("Failed to stop daemon: {}", error_msg));
            }

            println!("ClipSync daemon stopped");
        }
        #[cfg(all(not(target_os = "linux"), not(target_os = "macos")))]
        {
            println!("Daemon stop not supported on this platform");
        }
        Ok(())
    }

    async fn show_status(&self) -> Result<()> {
        println!("ClipSync Status:");
        println!("  Version: {}", env!("CARGO_PKG_VERSION"));
        println!("  Config: Default");
        println!("  Node ID: {}", self.config.node_id());

        // Check if daemon is running
        #[cfg(target_os = "linux")]
        {
            if daemon::is_daemon_running()? {
                if let Some(pid) = daemon::read_pidfile()? {
                    println!("  Daemon: Running (PID: {})", pid);
                } else {
                    println!("  Daemon: Running");
                }
            } else {
                println!("  Daemon: Not running");
            }
        }
        #[cfg(target_os = "macos")]
        {
            // Check launchctl status
            let status_output = Command::new("launchctl")
                .args(&["list", "com.clipsync"])
                .output()?;

            if status_output.status.success() {
                // Parse PID from output
                let output_str = String::from_utf8_lossy(&status_output.stdout);
                let parts: Vec<&str> = output_str.trim().split_whitespace().collect();
                if parts.len() >= 1 && parts[0] != "-" {
                    if let Ok(pid) = parts[0].parse::<i32>() {
                        println!("  Daemon: Running (PID: {})", pid);
                    } else {
                        println!("  Daemon: Running");
                    }
                } else {
                    println!("  Daemon: Loaded but not running");
                }
            } else {
                println!("  Daemon: Not running");
            }
        }
        #[cfg(all(not(target_os = "linux"), not(target_os = "macos")))]
        {
            println!("  Daemon: Not supported on this platform");
        }

        if let Some(sync_engine) = &self.sync_engine {
            let peers = sync_engine.get_connected_peers().await;
            println!("  Connected Peers: {}", peers.len());
        }

        Ok(())
    }

    async fn show_history(&mut self, limit: usize) -> Result<()> {
        let history = self.ensure_history().await?;
        let entries = history.get_recent_entries(limit).await?;

        if entries.is_empty() {
            println!("No clipboard history found");
            return Ok(());
        }

        println!("Clipboard History (showing {} entries):", entries.len());
        for (i, entry) in entries.iter().enumerate() {
            println!(
                "{}. [{}] {}",
                i + 1,
                entry.timestamp.format("%Y-%m-%d %H:%M:%S"),
                match &entry.content {
                    crate::adapters::ClipboardData::Text(text) => {
                        if text.len() > 50 {
                            format!("{}...", &text[..50])
                        } else {
                            text.clone()
                        }
                    }
                }
            );
        }

        Ok(())
    }

    async fn show_interactive_history(&mut self) -> Result<()> {
        let history = self.ensure_history().await?;
        let mut picker = history_picker::HistoryPicker::new(Arc::clone(&history));
        picker.show().await
    }

    async fn force_sync(&self) -> Result<()> {
        if let Some(sync_engine) = &self.sync_engine {
            sync_engine.force_sync().await?;
            println!("Clipboard sync completed");
        } else {
            println!("ClipSync daemon is not running");
        }
        Ok(())
    }

    async fn show_peers(&self) -> Result<()> {
        if let Some(sync_engine) = &self.sync_engine {
            let peers = sync_engine.get_connected_peers().await;

            if peers.is_empty() {
                println!("No connected peers");
                return Ok(());
            }

            println!("Connected Peers ({}):", peers.len());
            for peer in peers {
                println!("  {} - {} ({})", peer.id, peer.hostname, peer.address);
            }
        } else {
            println!("ClipSync daemon is not running");
        }

        Ok(())
    }

    async fn copy_text(&mut self, text: String) -> Result<()> {
        let clipboard = self.ensure_clipboard().await?;
        clipboard.set_text(&text).await?;
        println!("Text copied to clipboard");
        Ok(())
    }

    async fn paste_text(&mut self) -> Result<()> {
        let clipboard = self.ensure_clipboard().await?;
        match clipboard.get_text().await {
            Ok(text) => {
                println!("{}", text);
            }
            Err(e) => {
                error!("Failed to get clipboard content: {}", e);
            }
        }
        Ok(())
    }

    async fn restart_daemon(&mut self) -> Result<()> {
        info!("Restarting ClipSync daemon");
        
        #[cfg(target_os = "linux")]
        {
            // Stop daemon if running
            if daemon::is_daemon_running()? {
                println!("Stopping ClipSync daemon...");
                daemon::stop_daemon()?;
                
                // Wait briefly for clean shutdown
                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
            }
            
            // Start daemon again
            println!("Starting ClipSync daemon...");
            self.start_daemon(false).await?;
            println!("ClipSync daemon restarted successfully");
        }
        
        #[cfg(target_os = "macos")]
        {
            println!("Restarting ClipSync daemon...");
            
            // Stop if running
            let _ = self.stop_daemon().await;
            
            // Wait briefly
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
            
            // Start again
            self.start_daemon(false).await?;
            println!("ClipSync daemon restarted successfully");
        }
        
        #[cfg(all(not(target_os = "linux"), not(target_os = "macos")))]
        {
            println!("Daemon restart not supported on this platform");
        }
        
        Ok(())
    }

    async fn clear_clipboard(&mut self) -> Result<()> {
        info!("Clearing clipboard content");
        let clipboard = self.ensure_clipboard().await?;
        clipboard.clear().await?;
        println!("Clipboard cleared");
        Ok(())
    }

    async fn show_version(&self) -> Result<()> {
        println!("ClipSync {}", env!("CARGO_PKG_VERSION"));
        println!("Build Information:");
        println!("  Target: {}", env!("TARGET"));
        println!("  Profile: {}", env!("PROFILE"));
        println!("  Rust Version: {}", env!("RUSTC_VERSION"));
        println!("  Build Date: {}", env!("BUILD_DATE"));
        
        // Show additional runtime information
        println!("Runtime Information:");
        println!("  Platform: {}", std::env::consts::OS);
        println!("  Architecture: {}", std::env::consts::ARCH);
        println!("  Node ID: {}", self.config.node_id());
        
        Ok(())
    }

    async fn search_history(&mut self, search_term: &str, limit: usize) -> Result<()> {
        let history = self.ensure_history().await?;
        let entries = history.search_entries(search_term, limit).await?;

        if entries.is_empty() {
            println!("No clipboard history entries found matching '{}'", search_term);
            return Ok(());
        }

        println!("Clipboard History (showing {} entries matching '{}'):", entries.len(), search_term);
        for (i, entry) in entries.iter().enumerate() {
            println!(
                "{}. [{}] {}",
                i + 1,
                entry.timestamp.format("%Y-%m-%d %H:%M:%S"),
                match &entry.content {
                    crate::adapters::ClipboardData::Text(text) => {
                        if text.len() > 50 {
                            format!("{}...", &text[..50])
                        } else {
                            text.clone()
                        }
                    }
                }
            );
        }

        Ok(())
    }

    async fn discover_peers(&mut self) -> Result<()> {
        println!("Discovering peers on the network...");
        
        let discovery = self.ensure_discovery().await?;
        
        // Start discovery and wait for results
        println!("Scanning for 10 seconds...");
        
        let peer_infos = discovery.discover_peers_timeout(std::time::Duration::from_secs(10)).await?;
        
        if peer_infos.is_empty() {
            println!("No peers discovered on the network");
            return Ok(());
        }

        println!("Discovered Peers ({}):", peer_infos.len());
        for peer_info in peer_infos {
            println!("  {} - {}", peer_info.id, peer_info.name);
            if let Some(addr) = peer_info.best_address() {
                println!("    Address: {}", addr);
            }
            println!("    Port: {}", peer_info.port);
            if !peer_info.metadata.capabilities.is_empty() 
                || peer_info.metadata.ssh_fingerprint.is_some()
                || peer_info.metadata.ssh_public_key.is_some() 
                || peer_info.metadata.device_name.is_some() {
                println!("    Metadata: {:?}", peer_info.metadata);
            }
            println!();
        }

        Ok(())
    }

    async fn run_diagnostics(&mut self) -> Result<()> {
        println!("Running ClipSync diagnostics...\n");

        let mut issues_found = 0;

        // Check configuration
        println!("🔧 Configuration Check:");
        if let Some(config_path) = self.get_config_path_for_validation() {
            match Config::validate(&config_path).await {
                Ok(_) => println!("  ✅ Configuration is valid"),
                Err(e) => {
                    println!("  ❌ Configuration validation failed: {}", e);
                    issues_found += 1;
                }
            }
        } else {
            println!("  ⚠️  No configuration file found");
            issues_found += 1;
        }

        // Check clipboard access
        println!("\n📋 Clipboard Access:");
        match self.ensure_clipboard().await {
            Ok(clipboard) => {
                match clipboard.get_text().await {
                    Ok(_) => println!("  ✅ Clipboard read access working"),
                    Err(e) => {
                        println!("  ❌ Clipboard read failed: {}", e);
                        issues_found += 1;
                    }
                }
                
                match clipboard.set_text("ClipSync diagnostic test").await {
                    Ok(_) => {
                        println!("  ✅ Clipboard write access working");
                        // Try to restore original content
                        let _ = clipboard.get_text().await;
                    }
                    Err(e) => {
                        println!("  ❌ Clipboard write failed: {}", e);
                        issues_found += 1;
                    }
                }
            }
            Err(e) => {
                println!("  ❌ Clipboard initialization failed: {}", e);
                issues_found += 1;
            }
        }

        // Check database access
        println!("\n🗄️  Database Access:");
        match self.ensure_history().await {
            Ok(history) => {
                match history.get_recent_entries(1).await {
                    Ok(_) => println!("  ✅ Database read access working"),
                    Err(e) => {
                        println!("  ❌ Database read failed: {}", e);
                        issues_found += 1;
                    }
                }
            }
            Err(e) => {
                println!("  ❌ Database initialization failed: {}", e);
                issues_found += 1;
            }
        }

        // Check network connectivity
        println!("\n🌐 Network Connectivity:");
        match self.ensure_discovery().await {
            Ok(_) => println!("  ✅ Network discovery service initialized"),
            Err(e) => {
                println!("  ❌ Network discovery failed: {}", e);
                issues_found += 1;
            }
        }

        // Check daemon status
        println!("\n🔄 Daemon Status:");
        #[cfg(target_os = "linux")]
        {
            match crate::daemon::is_daemon_running() {
                Ok(true) => {
                    println!("  ✅ ClipSync daemon is running");
                    if let Ok(Some(pid)) = crate::daemon::read_pidfile() {
                        println!("      PID: {}", pid);
                    }
                }
                Ok(false) => println!("  ℹ️  ClipSync daemon is not running"),
                Err(e) => {
                    println!("  ❌ Failed to check daemon status: {}", e);
                    issues_found += 1;
                }
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            println!("  ℹ️  Daemon status check not supported on this platform");
        }

        // Check system requirements
        println!("\n💻 System Requirements:");
        println!("  ✅ Platform: {}", std::env::consts::OS);
        println!("  ✅ Architecture: {}", std::env::consts::ARCH);
        
        // Check available ports
        println!("\n🔌 Port Availability:");
        let test_port = self.config.websocket_port();
        match std::net::TcpListener::bind(format!("127.0.0.1:{}", test_port)) {
            Ok(_) => println!("  ✅ Port {} is available", test_port),
            Err(_) => {
                println!("  ⚠️  Port {} is in use (this is normal if daemon is running)", test_port);
            }
        }

        // Summary
        println!("\n📊 Diagnostic Summary:");
        if issues_found == 0 {
            println!("  🎉 All diagnostics passed! ClipSync should work correctly.");
        } else {
            println!("  ⚠️  {} issue(s) found. Please address them before using ClipSync.", issues_found);
        }

        Ok(())
    }

    async fn follow_logs(&mut self) -> Result<()> {
        println!("Following logs (press Ctrl+C to stop)...\n");

        // Try to find log files in common locations
        let possible_log_paths = vec![
            Some("/var/log/clipsync.log".into()),
            Some("/tmp/clipsync.log".into()),
            dirs::cache_dir().map(|p| p.join("clipsync").join("clipsync.log")),
            dirs::home_dir().map(|p| p.join(".clipsync").join("clipsync.log")),
        ];

        let mut log_file_found = false;

        for path_opt in possible_log_paths {
            if let Some(path) = path_opt {
                if path.exists() {
                    log_file_found = true;
                    println!("Following logs from: {}", path.display());
                    
                    // For now, implement a simple polling-based tail
                    // In a production system, you'd want to use inotify or similar
                    let mut last_size = std::fs::metadata(&path)?.len();
                    
                    loop {
                        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                        
                        let current_size = std::fs::metadata(&path)?.len();
                        if current_size > last_size {
                            if let Ok(new_lines) = self.read_log_file_from_offset(&path, last_size) {
                                for line in new_lines {
                                    println!("{}", line);
                                }
                            }
                            last_size = current_size;
                        }
                    }
                }
            }
        }

        if !log_file_found {
            println!("No log files found in standard locations.");
            println!("You can follow logs using:");
            println!("  journalctl -u clipsync -f");
        }

        Ok(())
    }

    async fn show_logs(&mut self, limit: usize) -> Result<()> {
        println!("Showing recent log entries (limit: {})...\n", limit);

        // Try to find log files in common locations
        let possible_log_paths = vec![
            Some("/var/log/clipsync.log".into()),
            Some("/tmp/clipsync.log".into()),
            dirs::cache_dir().map(|p| p.join("clipsync").join("clipsync.log")),
            dirs::home_dir().map(|p| p.join(".clipsync").join("clipsync.log")),
        ];

        let mut log_file_found = false;

        for path_opt in possible_log_paths {
            if let Some(path) = path_opt {
                if path.exists() {
                    log_file_found = true;
                    println!("Reading logs from: {}", path.display());
                    
                    match self.read_log_file(&path, limit) {
                        Ok(lines) => {
                            if lines.is_empty() {
                                println!("Log file is empty");
                            } else {
                                for line in lines {
                                    println!("{}", line);
                                }
                            }
                        }
                        Err(e) => {
                            error!("Failed to read log file: {}", e);
                        }
                    }
                    break;
                }
            }
        }

        if !log_file_found {
            println!("No log files found in standard locations.");
            println!("Checking systemd/journald logs...\n");
            
            // Try user systemd first
            let user_cmd = Command::new("journalctl")
                .args(&["--user", "-u", "clipsync", "-n", &limit.to_string(), "--no-pager"])
                .output();
                
            if let Ok(output) = user_cmd {
                if output.status.success() && !output.stdout.is_empty() {
                    println!("User systemd logs:");
                    println!("{}", String::from_utf8_lossy(&output.stdout));
                    return Ok(());
                }
            }
            
            // Try system systemd
            let system_cmd = Command::new("journalctl")
                .args(&["-u", "clipsync", "-n", &limit.to_string(), "--no-pager"])
                .output();
                
            if let Ok(output) = system_cmd {
                if output.status.success() && !output.stdout.is_empty() {
                    println!("System systemd logs:");
                    println!("{}", String::from_utf8_lossy(&output.stdout));
                    return Ok(());
                }
            }
            
            println!("No logs found. Try running:");
            println!("  journalctl --user -u clipsync -n {}", limit);
            println!("  journalctl -u clipsync -n {}", limit);
            println!("\nOr check if logs are being written to stderr/stdout when running in foreground mode.");
        }

        Ok(())
    }

    fn read_log_file(&self, path: &std::path::Path, limit: usize) -> Result<Vec<String>> {
        let file = File::open(path)?;
        let reader = BufReader::new(file);
        
        let lines: Result<Vec<String>, std::io::Error> = reader.lines().collect();
        let lines = lines?;
        
        // Take the last 'limit' lines
        let start_idx = if lines.len() > limit {
            lines.len() - limit
        } else {
            0
        };
        
        Ok(lines[start_idx..].to_vec())
    }

    fn read_log_file_from_offset(&self, path: &std::path::Path, offset: u64) -> Result<Vec<String>> {
        use std::io::{Seek, SeekFrom};
        
        let mut file = File::open(path)?;
        file.seek(SeekFrom::Start(offset))?;
        
        let reader = BufReader::new(file);
        let lines: Result<Vec<String>, std::io::Error> = reader.lines().collect();
        
        Ok(lines?)
    }

    async fn handle_config_action(&mut self, action: ConfigAction) -> Result<()> {
        match action {
            ConfigAction::Show => {
                println!("Current Configuration:");
                println!("{:#?}", self.config);
            }
            ConfigAction::Init { force } => {
                Config::generate_example_config(force)
                    .await
                    .map_err(|e| anyhow::anyhow!("Config error: {}", e))?;
                println!("Example configuration generated");
            }
            ConfigAction::Validate => {
                if let Some(config_path) = self.get_config_path_for_validation() {
                    match Config::validate(&config_path).await {
                        Ok(_) => println!("Configuration is valid"),
                        Err(e) => {
                            error!("Configuration validation failed: {}", e);
                            return Err(anyhow::anyhow!("Config error: {}", e));
                        }
                    }
                } else {
                    println!("No configuration file found to validate");
                    println!("Checked locations:");
                    println!("  - CLIPSYNC_CONFIG environment variable");
                    if let Some(config_dir) = dirs::config_dir() {
                        println!(
                            "  - {}",
                            config_dir.join("clipsync").join("config.toml").display()
                        );
                    }
                }
            }
            ConfigAction::Edit => {
                self.edit_config().await?;
            }
        }
        Ok(())
    }

    async fn edit_config(&mut self) -> Result<()> {
        let config_path = if let Some(ref path) = self.config_path {
            path.clone()
        } else if let Some(path) = self.get_config_path_for_validation() {
            path
        } else {
            // Create default config path
            let config_dir = dirs::config_dir()
                .ok_or_else(|| anyhow::anyhow!("Could not determine config directory"))?
                .join("clipsync");
            
            std::fs::create_dir_all(&config_dir)?;
            config_dir.join("config.toml")
        };

        // Determine which editor to use
        let editor = std::env::var("EDITOR")
            .or_else(|_| std::env::var("VISUAL"))
            .unwrap_or_else(|_| {
                if cfg!(target_os = "windows") {
                    "notepad".to_string()
                } else {
                    "nano".to_string()
                }
            });

        println!("Opening configuration file with {}: {}", editor, config_path.display());

        // Check if config file exists, if not create a template
        if !config_path.exists() {
            println!("Configuration file doesn't exist, creating template...");
            Config::generate_example_config(false).await
                .map_err(|e| anyhow::anyhow!("Failed to create config template: {}", e))?;
        }

        // Launch editor
        let status = Command::new(&editor)
            .arg(&config_path)
            .status()?;

        if !status.success() {
            return Err(anyhow::anyhow!("Editor exited with non-zero status"));
        }

        // Validate the edited configuration
        match Config::validate(&config_path).await {
            Ok(_) => println!("Configuration updated and validated successfully"),
            Err(e) => {
                error!("Configuration validation failed after editing: {}", e);
                println!("Warning: The configuration file may contain errors.");
                println!("Run 'clipsync config validate' to check for issues.");
            }
        }

        Ok(())
    }

    async fn handle_auth_action(&mut self, action: AuthAction) -> Result<()> {
        match action {
            AuthAction::Add { public_key, name } => self.add_authorized_key(public_key, name).await,
            AuthAction::List => self.list_authorized_keys().await,
            AuthAction::Remove { key_id } => self.remove_authorized_key(key_id).await,
        }
    }

    async fn add_authorized_key(&self, public_key_input: String, name: Option<String>) -> Result<()> {
        let auth_keys_path = &self.config.auth.authorized_keys;
        
        // Parse the public key
        let public_key = if std::path::Path::new(&public_key_input).exists() {
            // It's a file path - read the public key from file
            let content = tokio::fs::read_to_string(&public_key_input).await
                .map_err(|e| anyhow::anyhow!("Failed to read key file: {}", e))?;
            
            // Extract just the key part (without comment)
            let key_line = content.lines()
                .find(|line| line.trim().starts_with("ssh-"))
                .ok_or_else(|| anyhow::anyhow!("No SSH public key found in file"))?;
            
            let parts: Vec<&str> = key_line.trim().split_whitespace().collect();
            if parts.len() < 2 {
                return Err(anyhow::anyhow!("Invalid public key format in file"));
            }
            
            let openssh_key = format!("{} {}", parts[0], parts[1]);
            PublicKey::from_openssh(&openssh_key)
                .map_err(|e| anyhow::anyhow!("Failed to parse public key from file: {}", e))?
        } else {
            // It's a direct key string
            PublicKey::from_openssh(&public_key_input)
                .map_err(|e| anyhow::anyhow!("Failed to parse public key: {}", e))?
        };

        // Load existing authorized keys or create new
        let mut auth_keys = if auth_keys_path.exists() {
            AuthorizedKeys::load_from_file(auth_keys_path).await
                .map_err(|e| anyhow::anyhow!("Failed to load authorized keys: {}", e))?
        } else {
            AuthorizedKeys::new()
        };

        // Check if key already exists
        if auth_keys.is_authorized(&public_key) {
            println!("Key is already authorized (fingerprint: {})", public_key.fingerprint());
            return Ok(());
        }

        // Add the key
        let authorized_key = AuthorizedKey {
            public_key: public_key.clone(),
            comment: name.clone(),
            options: vec![],
        };
        
        auth_keys.add_key(authorized_key);

        // Save updated keys
        auth_keys.save_to_file(auth_keys_path).await
            .map_err(|e| anyhow::anyhow!("Failed to save authorized keys: {}", e))?;

        println!("✓ Added authorized key");
        println!("  Fingerprint: {}", public_key.fingerprint());
        if let Some(ref comment) = name {
            println!("  Name: {}", comment);
        }
        println!("  Saved to: {}", auth_keys_path.display());

        Ok(())
    }

    async fn list_authorized_keys(&self) -> Result<()> {
        let auth_keys_path = &self.config.auth.authorized_keys;
        
        if !auth_keys_path.exists() {
            println!("No authorized keys file found at: {}", auth_keys_path.display());
            println!("Use 'clipsync auth add <public_key>' to add the first authorized key.");
            return Ok(());
        }

        let auth_keys = AuthorizedKeys::load_from_file(auth_keys_path).await
            .map_err(|e| anyhow::anyhow!("Failed to load authorized keys: {}", e))?;

        if auth_keys.is_empty() {
            println!("No authorized keys found.");
            return Ok(());
        }

        println!("Authorized Keys ({} total):", auth_keys.len());
        println!();
        
        for (i, key) in auth_keys.list_keys().iter().enumerate() {
            println!("{}. Key Type: {}", i + 1, key.public_key.key_type.ssh_name());
            println!("   Fingerprint: {}", key.public_key.fingerprint());
            
            if let Some(ref comment) = key.comment {
                println!("   Name/Comment: {}", comment);
            }
            
            if !key.options.is_empty() {
                println!("   Options: {}", key.options.join(", "));
            }
            
            println!();
        }

        println!("Authorized keys file: {}", auth_keys_path.display());
        Ok(())
    }

    async fn remove_authorized_key(&self, key_id: String) -> Result<()> {
        let auth_keys_path = &self.config.auth.authorized_keys;
        
        if !auth_keys_path.exists() {
            println!("No authorized keys file found at: {}", auth_keys_path.display());
            return Ok(());
        }

        let mut auth_keys = AuthorizedKeys::load_from_file(auth_keys_path).await
            .map_err(|e| anyhow::anyhow!("Failed to load authorized keys: {}", e))?;

        if auth_keys.is_empty() {
            println!("No authorized keys found to remove.");
            return Ok(());
        }

        // Try to find key by fingerprint first
        let removed = if auth_keys.remove_key_by_fingerprint(&key_id) {
            true
        } else {
            // Try to find by comment/name
            let keys_list = auth_keys.list_keys().to_vec();
            
            // Find key with matching comment
            if let Some(key_to_remove) = keys_list.iter().find(|k| {
                k.comment.as_ref().map_or(false, |c| c.contains(&key_id))
            }) {
                let fingerprint = key_to_remove.public_key.fingerprint();
                auth_keys.remove_key_by_fingerprint(&fingerprint)
            } else {
                false
            }
        };

        if !removed {
            println!("No key found matching '{}'", key_id);
            println!("Use 'clipsync auth list' to see available keys.");
            println!("You can remove keys by fingerprint or name/comment.");
            return Ok(());
        }

        // Save updated keys
        auth_keys.save_to_file(auth_keys_path).await
            .map_err(|e| anyhow::anyhow!("Failed to save authorized keys: {}", e))?;

        println!("✓ Removed authorized key matching '{}'", key_id);
        println!("  Remaining keys: {}", auth_keys.len());
        
        Ok(())
    }

}
