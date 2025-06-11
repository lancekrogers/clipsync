use std::io::{self, Write};
use std::sync::Arc;

use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use tracing::debug;

use crate::adapters::{HistoryManager, ClipboardEntry};

pub struct HistoryPicker {
    history: Arc<HistoryManager>,
    entries: Vec<ClipboardEntry>,
    selected_index: usize,
    search_term: String,
}

impl HistoryPicker {
    pub fn new(history: Arc<HistoryManager>) -> Self {
        Self {
            history,
            entries: Vec::new(),
            selected_index: 0,
            search_term: String::new(),
        }
    }

    pub async fn show(&mut self) -> Result<()> {
        self.load_entries().await?;
        
        if self.entries.is_empty() {
            println!("No clipboard history found");
            return Ok(());
        }

        enable_raw_mode()?;
        execute!(io::stdout(), EnterAlternateScreen)?;

        let result = self.run_picker().await;

        disable_raw_mode()?;
        execute!(io::stdout(), LeaveAlternateScreen)?;

        result
    }

    async fn load_entries(&mut self) -> Result<()> {
        self.entries = self.history.get_recent_entries(100).await?;
        Ok(())
    }

    async fn run_picker(&mut self) -> Result<()> {
        loop {
            self.draw()?;

            if let Event::Key(key_event) = event::read()? {
                match self.handle_key_event(key_event).await? {
                    PickerAction::Exit => break,
                    PickerAction::Select => {
                        if let Some(entry) = self.entries.get(self.selected_index) {
                            self.copy_entry_to_clipboard(entry).await?;
                        }
                        break;
                    }
                    PickerAction::Continue => {}
                }
            }
        }

        Ok(())
    }

    fn draw(&self) -> Result<()> {
        print!("\x1B[2J\x1B[H"); // Clear screen and move cursor to top

        println!("ClipSync History Picker");
        println!("======================");
        println!("Use ↑/↓ to navigate, Enter to select, Esc to exit");
        
        if !self.search_term.is_empty() {
            println!("Search: {}", self.search_term);
        }
        
        println!();

        let visible_entries = self.get_filtered_entries();
        
        for (i, entry) in visible_entries.iter().enumerate() {
            let prefix = if i == self.selected_index { "► " } else { "  " };
            
            let content_preview = match &entry.content {
                crate::adapters::ClipboardData::Text(text) => {
                    let preview = if text.len() > 80 {
                        format!("{}...", &text[..80])
                    } else {
                        text.clone()
                    };
                    
                    // Replace newlines with spaces for display
                    preview.replace('\n', " ").replace('\r', "")
                }
            };

            println!("{}{} | {}", 
                prefix,
                entry.timestamp.format("%m-%d %H:%M"),
                content_preview
            );
        }

        io::stdout().flush()?;
        Ok(())
    }

    fn get_filtered_entries(&self) -> Vec<&ClipboardEntry> {
        if self.search_term.is_empty() {
            self.entries.iter().collect()
        } else {
            self.entries
                .iter()
                .filter(|entry| {
                    match &entry.content {
                        crate::adapters::ClipboardData::Text(text) => {
                            text.to_lowercase().contains(&self.search_term.to_lowercase())
                        }
                    }
                })
                .collect()
        }
    }

    async fn handle_key_event(&mut self, key_event: KeyEvent) -> Result<PickerAction> {
        match key_event.code {
            KeyCode::Esc => Ok(PickerAction::Exit),
            KeyCode::Enter => Ok(PickerAction::Select),
            KeyCode::Up => {
                if self.selected_index > 0 {
                    self.selected_index -= 1;
                }
                Ok(PickerAction::Continue)
            }
            KeyCode::Down => {
                let filtered_count = self.get_filtered_entries().len();
                if self.selected_index < filtered_count.saturating_sub(1) {
                    self.selected_index += 1;
                }
                Ok(PickerAction::Continue)
            }
            KeyCode::Char(c) => {
                self.search_term.push(c);
                self.selected_index = 0; // Reset selection when searching
                Ok(PickerAction::Continue)
            }
            KeyCode::Backspace => {
                self.search_term.pop();
                self.selected_index = 0;
                Ok(PickerAction::Continue)
            }
            _ => Ok(PickerAction::Continue),
        }
    }

    async fn copy_entry_to_clipboard(&self, entry: &ClipboardEntry) -> Result<()> {
        debug!("Copying entry {} to clipboard", entry.id);
        
        // In a real implementation, we would get the clipboard provider here
        // For now, we'll just print the selection
        match &entry.content {
            crate::adapters::ClipboardData::Text(text) => {
                println!("\nSelected clipboard entry:");
                println!("{}", text);
            }
        }
        
        Ok(())
    }
}

enum PickerAction {
    Continue,
    Select,
    Exit,
}

impl Drop for HistoryPicker {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
    }
}