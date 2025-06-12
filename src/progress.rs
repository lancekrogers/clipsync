//! Progress indicators for long-running operations
//!
//! This module provides user-friendly progress feedback for operations
//! that may take time to complete.

use std::io::{self, Write};
use std::time::{Duration, Instant};
use tokio::time::sleep;

/// Progress indicator for operations with known duration
pub struct ProgressIndicator {
    message: String,
    start_time: Instant,
    spinner_chars: Vec<char>,
    current_char: usize,
    show_spinner: bool,
}

impl ProgressIndicator {
    /// Create a new progress indicator
    pub fn new(message: &str) -> Self {
        Self {
            message: message.to_string(),
            start_time: Instant::now(),
            spinner_chars: vec!['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'],
            current_char: 0,
            show_spinner: true,
        }
    }

    /// Start showing the progress indicator
    pub async fn start(&mut self) {
        self.show_initial_message();
        
        // Start async spinner task
        let message = self.message.clone();
        let spinner_chars = self.spinner_chars.clone();
        
        tokio::spawn(async move {
            let mut char_index = 0;
            loop {
                // Update spinner
                print!("\r{} {} ({:.1}s)", 
                    spinner_chars[char_index], 
                    message,
                    Instant::now().duration_since(Instant::now()).as_secs_f32()
                );
                io::stdout().flush().unwrap_or(());
                
                char_index = (char_index + 1) % spinner_chars.len();
                sleep(Duration::from_millis(100)).await;
            }
        });
    }

    /// Show initial message without spinner
    fn show_initial_message(&self) {
        print!("{} ", self.message);
        io::stdout().flush().unwrap_or(());
    }

    /// Update the progress message
    pub fn update(&mut self, new_message: &str) {
        self.message = new_message.to_string();
        self.show_current_progress();
    }

    /// Show current progress
    fn show_current_progress(&self) {
        let elapsed = self.start_time.elapsed().as_secs_f32();
        if self.show_spinner {
            print!("\r{} {} ({:.1}s)", 
                self.spinner_chars[self.current_char], 
                self.message, 
                elapsed
            );
        } else {
            print!("\r{} ({:.1}s)", self.message, elapsed);
        }
        io::stdout().flush().unwrap_or(());
    }

    /// Complete the operation successfully
    pub fn success(&self, final_message: &str) {
        let elapsed = self.start_time.elapsed().as_secs_f32();
        println!("\r✅ {} ({:.1}s)", final_message, elapsed);
    }

    /// Complete the operation with an error
    pub fn error(&self, error_message: &str) {
        let elapsed = self.start_time.elapsed().as_secs_f32();
        println!("\r❌ {} ({:.1}s)", error_message, elapsed);
    }

    /// Complete the operation with a warning
    pub fn warning(&self, warning_message: &str) {
        let elapsed = self.start_time.elapsed().as_secs_f32();
        println!("\r⚠️  {} ({:.1}s)", warning_message, elapsed);
    }
}

/// Simple progress bar for operations with known progress
pub struct ProgressBar {
    message: String,
    total: u64,
    current: u64,
    width: usize,
    start_time: Instant,
}

impl ProgressBar {
    /// Create a new progress bar
    pub fn new(message: &str, total: u64) -> Self {
        Self {
            message: message.to_string(),
            total,
            current: 0,
            width: 50,
            start_time: Instant::now(),
        }
    }

    /// Update progress
    pub fn update(&mut self, current: u64) {
        self.current = current.min(self.total);
        self.show_progress();
    }

    /// Increment progress by one
    pub fn increment(&mut self) {
        self.update(self.current + 1);
    }

    /// Show current progress
    fn show_progress(&self) {
        let percentage = if self.total > 0 {
            (self.current as f32 / self.total as f32 * 100.0) as u8
        } else {
            0
        };

        let filled = (self.current as f32 / self.total as f32 * self.width as f32) as usize;
        let empty = self.width - filled;

        let bar = "█".repeat(filled) + &"░".repeat(empty);
        let elapsed = self.start_time.elapsed().as_secs_f32();

        // Calculate ETA
        let eta = if self.current > 0 && self.current < self.total {
            let rate = self.current as f32 / elapsed;
            let remaining = self.total - self.current;
            Some(remaining as f32 / rate)
        } else {
            None
        };

        print!("\r{} [{}] {}% ({}/{}) {:.1}s", 
            self.message,
            bar,
            percentage,
            self.current,
            self.total,
            elapsed
        );

        if let Some(eta) = eta {
            print!(" ETA: {:.1}s", eta);
        }

        io::stdout().flush().unwrap_or(());
    }

    /// Complete the progress bar
    pub fn finish(&self, final_message: &str) {
        let elapsed = self.start_time.elapsed().as_secs_f32();
        println!("\r✅ {} - completed in {:.1}s", final_message, elapsed);
    }
}

/// Macro for showing progress during async operations
#[macro_export]
macro_rules! with_progress {
    ($message:expr, $operation:expr) => {{
        use $crate::progress::ProgressIndicator;
        
        let mut progress = ProgressIndicator::new($message);
        print!("{} ", $message);
        std::io::Write::flush(&mut std::io::stdout()).unwrap_or(());
        
        let result = $operation.await;
        
        match &result {
            Ok(_) => progress.success("completed"),
            Err(e) => progress.error(&format!("failed: {}", e)),
        }
        
        result
    }};
}

/// Connection progress tracker for multi-step operations
pub struct ConnectionProgress {
    steps: Vec<String>,
    current_step: usize,
    start_time: Instant,
}

impl ConnectionProgress {
    /// Create a new connection progress tracker
    pub fn new() -> Self {
        Self {
            steps: vec![
                "Connecting to device".to_string(),
                "Performing handshake".to_string(),
                "Authenticating".to_string(),
                "Establishing secure connection".to_string(),
            ],
            current_step: 0,
            start_time: Instant::now(),
        }
    }

    /// Start the connection process
    pub fn start_connecting(&mut self, device_name: &str) {
        self.steps[0] = format!("Connecting to device '{}'", device_name);
        self.show_current_step();
    }

    /// Move to handshake step
    pub fn start_handshake(&mut self) {
        self.current_step = 1;
        self.show_current_step();
    }

    /// Move to authentication step
    pub fn start_authentication(&mut self) {
        self.current_step = 2;
        self.show_current_step();
    }

    /// Move to final connection step
    pub fn finalizing_connection(&mut self) {
        self.current_step = 3;
        self.show_current_step();
    }

    /// Show current step
    fn show_current_step(&self) {
        let elapsed = self.start_time.elapsed().as_secs_f32();
        println!("({:.1}s) Step {}/{}: {}", 
            elapsed,
            self.current_step + 1, 
            self.steps.len(), 
            self.steps[self.current_step]
        );
    }

    /// Complete successfully
    pub fn success(&self, device_name: &str) {
        let elapsed = self.start_time.elapsed().as_secs_f32();
        println!("✅ Successfully connected to '{}' in {:.1}s", device_name, elapsed);
    }

    /// Complete with error
    pub fn error(&self, error_message: &str) {
        let elapsed = self.start_time.elapsed().as_secs_f32();
        println!("❌ Connection failed after {:.1}s: {}", elapsed, error_message);
    }
}

/// File transfer progress tracker
pub struct TransferProgress {
    filename: String,
    total_bytes: u64,
    transferred_bytes: u64,
    start_time: Instant,
    last_update: Instant,
    last_bytes: u64,
}

impl TransferProgress {
    /// Create a new transfer progress tracker
    pub fn new(filename: &str, total_bytes: u64) -> Self {
        let now = Instant::now();
        Self {
            filename: filename.to_string(),
            total_bytes,
            transferred_bytes: 0,
            start_time: now,
            last_update: now,
            last_bytes: 0,
        }
    }

    /// Update transfer progress
    pub fn update(&mut self, transferred_bytes: u64) {
        self.transferred_bytes = transferred_bytes.min(self.total_bytes);
        
        let now = Instant::now();
        let time_diff = now.duration_since(self.last_update).as_secs_f32();
        
        // Update every 100ms to avoid too frequent updates
        if time_diff >= 0.1 {
            self.show_progress();
            self.last_update = now;
            self.last_bytes = self.transferred_bytes;
        }
    }

    /// Show current transfer progress
    fn show_progress(&self) {
        let percentage = if self.total_bytes > 0 {
            (self.transferred_bytes as f32 / self.total_bytes as f32 * 100.0) as u8
        } else {
            0
        };

        let elapsed = self.start_time.elapsed().as_secs_f32();
        let rate = if elapsed > 0.0 {
            self.transferred_bytes as f32 / elapsed
        } else {
            0.0
        };

        let eta = if self.transferred_bytes > 0 && self.transferred_bytes < self.total_bytes {
            let remaining = self.total_bytes - self.transferred_bytes;
            Some(remaining as f32 / rate)
        } else {
            None
        };

        print!("\r📄 {} - {}% ({}) {}/s", 
            self.filename,
            percentage,
            format_bytes(self.transferred_bytes),
            format_bytes(rate as u64)
        );

        if let Some(eta) = eta {
            print!(" ETA: {}s", eta as u64);
        }

        io::stdout().flush().unwrap_or(());
    }

    /// Complete the transfer
    pub fn finish(&self) {
        let elapsed = self.start_time.elapsed().as_secs_f32();
        let avg_rate = if elapsed > 0.0 {
            self.total_bytes as f32 / elapsed
        } else {
            0.0
        };

        println!("\r✅ {} - transfer completed ({} in {:.1}s, avg {}/s)", 
            self.filename,
            format_bytes(self.total_bytes),
            elapsed,
            format_bytes(avg_rate as u64)
        );
    }
}

/// Format bytes in human-readable format
fn format_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB"];
    let mut size = bytes as f64;
    let mut unit_index = 0;

    while size >= 1024.0 && unit_index < UNITS.len() - 1 {
        size /= 1024.0;
        unit_index += 1;
    }

    if unit_index == 0 {
        format!("{} {}", bytes, UNITS[unit_index])
    } else {
        format!("{:.1} {}", size, UNITS[unit_index])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1023), "1023 B");
        assert_eq!(format_bytes(1024), "1.0 KB");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(1048576), "1.0 MB");
        assert_eq!(format_bytes(1073741824), "1.0 GB");
    }

    #[tokio::test]
    async fn test_progress_indicator() {
        let mut progress = ProgressIndicator::new("Testing");
        progress.update("In progress");
        progress.success("Test completed");
    }

    #[test]
    fn test_progress_bar() {
        let mut bar = ProgressBar::new("Testing", 100);
        bar.update(50);
        bar.finish("Test completed");
    }
}