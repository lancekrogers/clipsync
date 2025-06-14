use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing::{error, info};

use crate::adapters::{
    get_clipboard_provider, ClipboardProviderWrapper, HistoryManager, PeerDiscovery,
};
use crate::config::Config;
use crate::hotkey::HotKeyManager;
use crate::sync::SyncEngine;
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
    },

    #[command(about = "Force synchronization with peers")]
    Sync,

    #[command(about = "List connected peers")]
    Peers,

    #[command(about = "Copy text to clipboard")]
    Copy { text: String },

    #[command(about = "Get current clipboard content")]
    Paste,

    #[command(about = "Configuration management")]
    Config {
        #[command(subcommand)]
        action: ConfigAction,
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
}

pub struct CliHandler {
    config: Arc<Config>,
    clipboard: Option<Arc<ClipboardProviderWrapper>>,
    history: Option<Arc<HistoryManager>>,
    discovery: Option<Arc<PeerDiscovery>>,
    transport: Option<Arc<TransportManager>>,
    sync_engine: Option<Arc<SyncEngine>>,
    hotkey_manager: Option<Arc<HotKeyManager>>,
}

impl CliHandler {
    pub async fn new(config_path: Option<PathBuf>) -> Result<Self> {
        let config = Arc::new(Config::load_config(config_path).await?);

        Ok(Self {
            config,
            clipboard: None,
            history: None,
            discovery: None,
            transport: None,
            sync_engine: None,
            hotkey_manager: None,
        })
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
            Commands::History { limit, interactive } => {
                if interactive {
                    self.show_interactive_history().await
                } else {
                    self.show_history(limit).await
                }
            }
            Commands::Sync => self.force_sync().await,
            Commands::Peers => self.show_peers().await,
            Commands::Copy { text } => self.copy_text(text).await,
            Commands::Paste => self.paste_text().await,
            Commands::Config { action } => self.handle_config_action(action).await,
        }
    }

    async fn start_daemon(&mut self, foreground: bool) -> Result<()> {
        info!("Starting ClipSync daemon");

        if !foreground {
            info!("Running in daemon mode");
            // TODO: Implement proper daemon mode with pidfile
        }

        // Ensure all components are initialized for daemon mode
        let clipboard = self.ensure_clipboard().await?;
        let history = self.ensure_history().await?;
        let discovery = self.ensure_discovery().await?;
        let transport = self.ensure_transport().await?;

        // Initialize sync engine
        let sync_engine = Arc::new(SyncEngine::new(
            Arc::clone(&self.config),
            Arc::clone(&clipboard),
            Arc::clone(&history),
            Arc::clone(&discovery),
            Arc::clone(&transport),
        ));

        // Initialize hotkey manager
        let mut hotkey_manager = HotKeyManager::new(
            Arc::clone(&self.config),
            Arc::clone(&clipboard),
            Arc::clone(&history),
        )?;

        hotkey_manager.set_sync_engine(Arc::clone(&sync_engine));
        hotkey_manager.register_default_hotkeys().await?;

        let hotkey_manager = Arc::new(hotkey_manager);

        self.sync_engine = Some(Arc::clone(&sync_engine));
        self.hotkey_manager = Some(Arc::clone(&hotkey_manager));

        // Start services
        let sync_task = {
            let sync_engine = Arc::clone(&sync_engine);
            async move { sync_engine.start().await }
        };

        let hotkey_task = {
            let hotkey_manager = Arc::clone(&hotkey_manager);
            async move { hotkey_manager.start_event_loop().await }
        };

        info!("ClipSync daemon started successfully");

        tokio::try_join!(sync_task, hotkey_task)?;

        Ok(())
    }

    async fn stop_daemon(&self) -> Result<()> {
        info!("Stopping ClipSync daemon");
        // TODO: Implement daemon stop logic (send signal to running process)
        println!("ClipSync daemon stopped");
        Ok(())
    }

    async fn show_status(&self) -> Result<()> {
        println!("ClipSync Status:");
        println!("  Version: {}", env!("CARGO_PKG_VERSION"));
        println!("  Config: Default");
        println!("  Node ID: {}", self.config.node_id());

        if let Some(sync_engine) = &self.sync_engine {
            let peers = sync_engine.get_connected_peers().await;
            println!("  Connected Peers: {}", peers.len());
        } else {
            println!("  Status: Not running");
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

    async fn handle_config_action(&mut self, action: ConfigAction) -> Result<()> {
        match action {
            ConfigAction::Show => {
                println!("Current Configuration:");
                println!("{:#?}", self.config);
            }
            ConfigAction::Init { force } => {
                Config::generate_example_config(force).await?;
                println!("Example configuration generated");
            }
            ConfigAction::Validate => {
                // Config is already loaded and validated in CliHandler::new()
                println!("Configuration is valid");
            }
        }
        Ok(())
    }
}
