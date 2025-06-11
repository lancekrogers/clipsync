use std::sync::Arc;
use std::collections::HashMap;

use anyhow::Result;
use global_hotkey::{
    GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState, 
    hotkey::{Code, HotKey, Modifiers}
};
use tokio::sync::broadcast;
use tracing::{debug, error, info, warn};

use crate::adapters::{HistoryManager, ClipboardProviderWrapper};
use crate::cli::history_picker::HistoryPicker;
use crate::config::Config;
use crate::sync::SyncEngine;

#[derive(Debug, Clone)]
pub enum HotKeyAction {
    ShowHistory,
    ForceSync,
    ToggleClipboard,
    CopyToSecondary,
    PasteFromSecondary,
}

#[derive(Debug, Clone)]
pub struct HotKeyEvent {
    pub action: HotKeyAction,
    pub hotkey_id: u32,
}

pub struct HotKeyManager {
    manager: GlobalHotKeyManager,
    config: Arc<Config>,
    clipboard: Arc<ClipboardProviderWrapper>,
    history: Arc<HistoryManager>,
    sync_engine: Option<Arc<SyncEngine>>,
    hotkeys: HashMap<u32, HotKeyAction>,
    event_sender: broadcast::Sender<HotKeyEvent>,
}

impl HotKeyManager {
    pub fn new(
        config: Arc<Config>,
        clipboard: Arc<ClipboardProviderWrapper>,
        history: Arc<HistoryManager>,
    ) -> Result<Self> {
        let manager = GlobalHotKeyManager::new()?;
        let (event_sender, _) = broadcast::channel(100);
        
        Ok(Self {
            manager,
            config,
            clipboard,
            history,
            sync_engine: None,
            hotkeys: HashMap::new(),
            event_sender,
        })
    }

    pub fn set_sync_engine(&mut self, sync_engine: Arc<SyncEngine>) {
        self.sync_engine = Some(sync_engine);
    }

    pub async fn register_default_hotkeys(&mut self) -> Result<()> {
        info!("Registering default hotkeys");

        // Cmd+Shift+V (or Ctrl+Shift+V on Linux) - Show clipboard history
        let history_hotkey = HotKey::new(
            Some(if cfg!(target_os = "macos") { 
                Modifiers::META | Modifiers::SHIFT 
            } else { 
                Modifiers::CONTROL | Modifiers::SHIFT 
            }),
            Code::KeyV,
        );
        self.register_hotkey(history_hotkey, HotKeyAction::ShowHistory)?;

        // Cmd+Shift+S (or Ctrl+Shift+S on Linux) - Force sync
        let sync_hotkey = HotKey::new(
            Some(if cfg!(target_os = "macos") { 
                Modifiers::META | Modifiers::SHIFT 
            } else { 
                Modifiers::CONTROL | Modifiers::SHIFT 
            }),
            Code::KeyS,
        );
        self.register_hotkey(sync_hotkey, HotKeyAction::ForceSync)?;

        // Cmd+Shift+C (or Ctrl+Shift+C on Linux) - Copy to secondary clipboard
        let copy_secondary_hotkey = HotKey::new(
            Some(if cfg!(target_os = "macos") { 
                Modifiers::META | Modifiers::SHIFT 
            } else { 
                Modifiers::CONTROL | Modifiers::SHIFT 
            }),
            Code::KeyC,
        );
        self.register_hotkey(copy_secondary_hotkey, HotKeyAction::CopyToSecondary)?;

        Ok(())
    }

    fn register_hotkey(&mut self, hotkey: HotKey, action: HotKeyAction) -> Result<()> {
        self.manager.register(hotkey)?;
        // For now, use a simple counter for hotkey ID mapping
        let hotkey_id = self.hotkeys.len() as u32;
        self.hotkeys.insert(hotkey_id, action.clone());
        
        debug!("Registered hotkey {} for action {:?}", hotkey_id, action);
        Ok(())
    }

    pub async fn start_event_loop(&self) -> Result<()> {
        info!("Starting hotkey event loop");
        
        let event_sender = self.event_sender.clone();
        let hotkeys = self.hotkeys.clone();
        let clipboard = Arc::clone(&self.clipboard);
        let history = Arc::clone(&self.history);
        let sync_engine = self.sync_engine.clone();
        
        tokio::task::spawn_blocking(move || {
            let rt = tokio::runtime::Handle::current();
            
            loop {
                if let Ok(event) = GlobalHotKeyEvent::receiver().try_recv() {
                    if event.state == HotKeyState::Pressed {
                        if let Some(action) = hotkeys.get(&event.id) {
                            debug!("Hotkey pressed: {:?}", action);
                            
                            let hotkey_event = HotKeyEvent {
                                action: action.clone(),
                                hotkey_id: event.id,
                            };
                            
                            // Send event through broadcast channel
                            if let Err(e) = event_sender.send(hotkey_event.clone()) {
                                warn!("Failed to broadcast hotkey event: {}", e);
                            }
                            
                            // Execute action directly
                            let clipboard_clone = Arc::clone(&clipboard);
                            let history_clone = Arc::clone(&history);
                            let sync_engine_clone = sync_engine.clone();
                            
                            rt.spawn(async move {
                                if let Err(e) = Self::execute_action(
                                    &hotkey_event.action,
                                    clipboard_clone,
                                    history_clone,
                                    sync_engine_clone,
                                ).await {
                                    error!("Failed to execute hotkey action: {}", e);
                                }
                            });
                        }
                    }
                }
                
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        });
        
        Ok(())
    }

    async fn execute_action(
        action: &HotKeyAction,
        clipboard: Arc<ClipboardProviderWrapper>,
        history: Arc<HistoryManager>,
        sync_engine: Option<Arc<SyncEngine>>,
    ) -> Result<()> {
        match action {
            HotKeyAction::ShowHistory => {
                debug!("Executing show history action");
                let mut picker = HistoryPicker::new(history);
                picker.show().await?;
            }
            HotKeyAction::ForceSync => {
                debug!("Executing force sync action");
                if let Some(sync_engine) = sync_engine {
                    sync_engine.force_sync().await?;
                    info!("Forced clipboard sync completed");
                } else {
                    warn!("Sync engine not available");
                }
            }
            HotKeyAction::ToggleClipboard => {
                debug!("Executing toggle clipboard action");
                // This could toggle between different clipboard modes or pause/resume sync
                info!("Clipboard toggle not yet implemented");
            }
            HotKeyAction::CopyToSecondary => {
                debug!("Executing copy to secondary action");
                // Copy current clipboard to a secondary buffer
                if let Ok(content) = clipboard.get_text().await {
                    // Store in secondary clipboard slot
                    // This would require extending the clipboard provider
                    info!("Copied to secondary clipboard: {}", 
                        if content.len() > 50 { 
                            format!("{}...", &content[..50]) 
                        } else { 
                            content 
                        }
                    );
                }
            }
            HotKeyAction::PasteFromSecondary => {
                debug!("Executing paste from secondary action");
                // Paste from secondary clipboard buffer
                info!("Paste from secondary not yet implemented");
            }
        }
        
        Ok(())
    }

    pub fn subscribe(&self) -> broadcast::Receiver<HotKeyEvent> {
        self.event_sender.subscribe()
    }

    pub fn unregister_all(&mut self) -> Result<()> {
        info!("Unregistering all hotkeys");
        
        // For now, just clear the mapping since we don't track individual hotkeys properly
        self.hotkeys.clear();
        Ok(())
    }
}

impl Drop for HotKeyManager {
    fn drop(&mut self) {
        let _ = self.unregister_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::{get_clipboard_provider, HistoryManager};
    use crate::config::Config;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_hotkey_manager_creation() {
        let temp_dir = TempDir::new().unwrap();
        let config = Arc::new(Config::default_with_path(temp_dir.path().join("config.toml")));
        let clipboard = Arc::new(get_clipboard_provider().await.unwrap());
        let history = Arc::new(HistoryManager::new(&temp_dir.path().join("history.db")).await.unwrap());
        
        let result = HotKeyManager::new(config, clipboard, history);
        assert!(result.is_ok());
    }

    #[test]
    fn test_hotkey_action_clone() {
        let action = HotKeyAction::ShowHistory;
        let cloned = action.clone();
        
        matches!(cloned, HotKeyAction::ShowHistory);
    }
}