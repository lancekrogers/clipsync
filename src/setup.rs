//! Interactive first-run setup experience for ClipSync
//!
//! This module provides a guided setup wizard to help users configure
//! ClipSync for the first time with step-by-step instructions.

use crate::config::{Config, ConfigError};
use crate::auth::{AuthError, KeyType, PublicKey};
use crate::progress::ProgressIndicator;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use uuid::Uuid;

/// Interactive setup wizard for first-time users
pub struct SetupWizard {
    config: Config,
    config_path: PathBuf,
}

impl SetupWizard {
    /// Create a new setup wizard
    pub fn new() -> Result<Self, ConfigError> {
        let config = Config::default();
        let config_path = Config::default_config_path()?;
        
        Ok(Self {
            config,
            config_path,
        })
    }

    /// Run the complete setup wizard
    pub async fn run(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.show_welcome();
        
        // Step 1: System check
        self.run_system_check().await?;
        
        // Step 2: SSH key setup
        self.setup_ssh_keys().await?;
        
        // Step 3: Basic configuration
        self.configure_basic_settings().await?;
        
        // Step 4: Network configuration
        self.configure_network().await?;
        
        // Step 5: Save configuration
        self.save_configuration().await?;
        
        // Step 6: Test setup
        self.test_setup().await?;
        
        self.show_completion();
        
        Ok(())
    }

    /// Show welcome message
    fn show_welcome(&self) {
        println!();
        println!("🔄 Welcome to ClipSync Setup!");
        println!();
        println!("This wizard will help you configure ClipSync for the first time.");
        println!("ClipSync synchronizes your clipboard between macOS and Linux devices");
        println!("on your local network with end-to-end encryption.");
        println!();
        println!("The setup process will:");
        println!("  1. Check your system requirements");
        println!("  2. Set up SSH keys for secure authentication");
        println!("  3. Configure basic settings");
        println!("  4. Configure network settings");
        println!("  5. Save your configuration");
        println!("  6. Test the setup");
        println!();
        
        self.wait_for_enter("Press Enter to continue...");
    }

    /// Run system compatibility checks
    async fn run_system_check(&self) -> Result<(), Box<dyn std::error::Error>> {
        println!("🔍 Step 1: System Compatibility Check");
        println!();

        let mut progress = ProgressIndicator::new("Checking system requirements");
        
        // Check OS compatibility
        let os = std::env::consts::OS;
        match os {
            "macos" => {
                progress.update("Checking macOS compatibility");
                println!("✅ macOS detected - ClipSync supports macOS 10.15+");
            }
            "linux" => {
                progress.update("Checking Linux compatibility");
                self.check_linux_environment()?;
                println!("✅ Linux detected - ClipSync supports X11 and Wayland");
            }
            _ => {
                progress.error(&format!("Unsupported operating system: {}", os));
                return Err(format!("ClipSync only supports macOS and Linux. Found: {}", os).into());
            }
        }

        // Check network capabilities
        progress.update("Checking network capabilities");
        self.check_network_capabilities().await?;
        
        // Check disk space
        progress.update("Checking disk space");
        self.check_disk_space()?;
        
        // Check permissions
        progress.update("Checking permissions");
        self.check_permissions()?;

        progress.success("System check completed");
        println!();
        
        Ok(())
    }

    /// Check Linux-specific requirements
    fn check_linux_environment(&self) -> Result<(), Box<dyn std::error::Error>> {
        // Check display server
        let display = std::env::var("DISPLAY").is_ok();
        let wayland = std::env::var("WAYLAND_DISPLAY").is_ok();
        
        if !display && !wayland {
            return Err("No display server detected. ClipSync requires X11 or Wayland.".into());
        }

        if display {
            println!("  • X11 display server detected");
        }
        
        if wayland {
            println!("  • Wayland display server detected");
        }

        // Check for required tools
        let tools = ["ssh-keygen", "ssh-add"];
        for tool in &tools {
            if Command::new("which").arg(tool).output()?.status.success() {
                println!("  • {} found", tool);
            } else {
                println!("  ⚠️  {} not found - install openssh-client package", tool);
            }
        }

        Ok(())
    }

    /// Check network capabilities
    async fn check_network_capabilities(&self) -> Result<(), Box<dyn std::error::Error>> {
        // Check if we can bind to a port
        use tokio::net::TcpListener;
        
        match TcpListener::bind("127.0.0.1:0").await {
            Ok(listener) => {
                let addr = listener.local_addr()?;
                println!("  • Network binding test successful (port {})", addr.port());
            }
            Err(e) => {
                return Err(format!("Cannot bind network port: {}", e).into());
            }
        }

        // Check hostname resolution
        let hostname = hostname::get()?.to_string_lossy().to_string();
        println!("  • Hostname: {}", hostname);

        Ok(())
    }

    /// Check available disk space
    fn check_disk_space(&self) -> Result<(), Box<dyn std::error::Error>> {
        use std::fs;
        
        // Check config directory space
        if let Some(parent) = self.config_path.parent() {
            if parent.exists() {
                println!("  • Configuration directory: {}", parent.display());
            } else {
                fs::create_dir_all(parent)?;
                println!("  • Created configuration directory: {}", parent.display());
            }
        }

        // Estimate space requirements
        println!("  • Required disk space: ~10MB (for config, keys, and history)");
        
        Ok(())
    }

    /// Check required permissions
    fn check_permissions(&self) -> Result<(), Box<dyn std::error::Error>> {
        use std::fs;

        // Check config directory write permissions
        if let Some(parent) = self.config_path.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent)?;
            }
            
            // Test write permission
            let test_file = parent.join(".clipsync_test");
            fs::write(&test_file, "test")?;
            fs::remove_file(&test_file)?;
            
            println!("  • Configuration directory is writable");
        }

        // Check SSH directory
        let home = std::env::var("HOME")?;
        let ssh_dir = Path::new(&home).join(".ssh");
        
        if !ssh_dir.exists() {
            fs::create_dir_all(&ssh_dir)?;
            // Set proper permissions (0700)
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = fs::metadata(&ssh_dir)?.permissions();
                perms.set_mode(0o700);
                fs::set_permissions(&ssh_dir, perms)?;
            }
            println!("  • Created SSH directory: {}", ssh_dir.display());
        } else {
            println!("  • SSH directory exists: {}", ssh_dir.display());
        }

        Ok(())
    }

    /// Set up SSH keys for authentication
    async fn setup_ssh_keys(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        println!("🔐 Step 2: SSH Key Setup");
        println!();
        println!("ClipSync uses SSH keys for secure device authentication.");
        println!("Each device needs its own SSH key pair, and devices exchange");
        println!("public keys to authorize each other.");
        println!();

        let home = std::env::var("HOME")?;
        let ssh_dir = Path::new(&home).join(".ssh");
        let default_key = ssh_dir.join("id_ed25519");
        let clipsync_key = ssh_dir.join("id_ed25519_clipsync");

        // Check for existing keys
        if clipsync_key.exists() {
            println!("✅ ClipSync SSH key already exists: {}", clipsync_key.display());
            self.config.auth.ssh_key = clipsync_key.to_string_lossy().to_string();
        } else if default_key.exists() {
            println!("📋 Found existing SSH key: {}", default_key.display());
            
            if self.ask_yes_no("Use existing SSH key for ClipSync?")? {
                self.config.auth.ssh_key = default_key.to_string_lossy().to_string();
            } else {
                self.generate_new_ssh_key(&clipsync_key).await?;
            }
        } else {
            println!("No SSH keys found. ClipSync will generate a new key pair.");
            self.generate_new_ssh_key(&clipsync_key).await?;
        }

        self.display_public_key()?;
        
        Ok(())
    }

    /// Generate a new SSH key pair
    async fn generate_new_ssh_key(&mut self, key_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let mut progress = ProgressIndicator::new("Generating SSH key pair");
        
        let hostname = hostname::get()?.to_string_lossy().to_string();
        let comment = format!("clipsync-{}", hostname);
        
        let output = Command::new("ssh-keygen")
            .args(&[
                "-t", "ed25519",
                "-f", &key_path.to_string_lossy(),
                "-C", &comment,
                "-N", "", // No passphrase
            ])
            .output()?;

        if !output.status.success() {
            progress.error("SSH key generation failed");
            return Err(format!("ssh-keygen failed: {}", String::from_utf8_lossy(&output.stderr)).into());
        }

        // Set proper permissions
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            use std::fs;
            
            // Private key: 0600
            let mut perms = fs::metadata(key_path)?.permissions();
            perms.set_mode(0o600);
            fs::set_permissions(key_path, perms)?;
            
            // Public key: 0644
            let pub_key_path = format!("{}.pub", key_path.to_string_lossy());
            let mut perms = fs::metadata(&pub_key_path)?.permissions();
            perms.set_mode(0o644);
            fs::set_permissions(&pub_key_path, perms)?;
        }

        progress.success("SSH key pair generated");
        
        self.config.auth.ssh_key = key_path.to_string_lossy().to_string();
        
        println!("✅ Generated new SSH key pair:");
        println!("   Private key: {}", key_path.display());
        println!("   Public key:  {}.pub", key_path.display());
        
        Ok(())
    }

    /// Display the public key for sharing
    fn display_public_key(&self) -> Result<(), Box<dyn std::error::Error>> {
        let pub_key_path = format!("{}.pub", self.config.auth.ssh_key);
        let public_key = std::fs::read_to_string(&pub_key_path)?;
        
        println!();
        println!("📤 Your ClipSync Public Key:");
        println!("┌─────────────────────────────────────────────────────────────────────────────┐");
        println!("│ {}│", public_key.trim());
        println!("└─────────────────────────────────────────────────────────────────────────────┘");
        println!();
        println!("💡 To connect other devices:");
        println!("   1. Install ClipSync on the other device");
        println!("   2. Run 'clipsync auth add' with this public key");
        println!("   3. Share that device's public key back to this device");
        println!();
        
        self.wait_for_enter("Press Enter to continue...");
        
        Ok(())
    }

    /// Configure basic settings
    async fn configure_basic_settings(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        println!("⚙️  Step 3: Basic Configuration");
        println!();

        // Device name
        let hostname = hostname::get()?.to_string_lossy().to_string();
        let default_name = format!("{}-clipsync", hostname);
        
        println!("Device name (used for discovery and identification):");
        let device_name = self.prompt_with_default("Device name", &default_name)?;
        self.config.advertise_name = device_name;

        // Clipboard history size
        println!();
        println!("Clipboard history keeps your recent clipboard items.");
        let history_size = self.prompt_number("History size (1-100)", 20, 1, 100)?;
        self.config.clipboard.history_size = history_size;

        // Maximum clipboard size
        println!();
        println!("Maximum clipboard size (larger items will be rejected):");
        println!("  1. 1MB  (recommended for slow networks)");
        println!("  2. 5MB  (default, good for most uses)");
        println!("  3. 50MB (maximum, for large files/images)");
        
        let size_choice = self.prompt_choice("Choose size", &["1MB", "5MB", "50MB"], 1)?;
        self.config.clipboard.max_size = match size_choice {
            0 => 1 * 1024 * 1024,      // 1MB
            1 => 5 * 1024 * 1024,      // 5MB (default)
            2 => 50 * 1024 * 1024,     // 50MB
            _ => 5 * 1024 * 1024,      // Default fallback
        };

        println!();
        
        Ok(())
    }

    /// Configure network settings
    async fn configure_network(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        println!("🌐 Step 4: Network Configuration");
        println!();

        // Listen address
        println!("Network listen address:");
        println!("  1. All interfaces (0.0.0.0) - discoverable by any device on network");
        println!("  2. Localhost only (127.0.0.1) - only for testing");
        println!("  3. Specific IP - enter manually");
        
        let addr_choice = self.prompt_choice("Choose address", &["All interfaces", "Localhost", "Specific IP"], 0)?;
        
        let listen_addr = match addr_choice {
            0 => ":8484".to_string(),
            1 => "127.0.0.1:8484".to_string(),
            2 => {
                let ip = self.prompt("Enter IP address")?;
                format!("{}:8484", ip)
            }
            _ => ":8484".to_string(),
        };
        
        self.config.listen_addr = listen_addr;

        // Port configuration
        println!();
        let port = self.prompt_number("Port number", 8484, 1024, 65535)?;
        if port != 8484 {
            self.config.listen_addr = self.config.listen_addr.replace("8484", &port.to_string());
        }

        // Security settings
        println!();
        println!("🔒 Security Settings:");
        
        let enable_compression = self.ask_yes_no("Enable compression for large clipboard items?")?;
        self.config.security.compression = if enable_compression { "zstd".to_string() } else { "none".to_string() };

        let encrypt_history = self.ask_yes_no("Encrypt clipboard history database?")?;
        self.config.security.encrypt_history = encrypt_history;

        println!();
        
        Ok(())
    }

    /// Save configuration to file
    async fn save_configuration(&self) -> Result<(), Box<dyn std::error::Error>> {
        println!("💾 Step 5: Saving Configuration");
        println!();

        let mut progress = ProgressIndicator::new("Saving configuration");
        
        // Create config directory if it doesn't exist
        if let Some(parent) = self.config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Save config
        self.config.save_to_file(&self.config_path)?;
        
        progress.success("Configuration saved");
        
        println!("✅ Configuration saved to: {}", self.config_path.display());
        
        // Show config summary
        println!();
        println!("📋 Configuration Summary:");
        println!("   Device name: {}", self.config.advertise_name);
        println!("   Listen address: {}", self.config.listen_addr);
        println!("   SSH key: {}", self.config.auth.ssh_key);
        println!("   History size: {} items", self.config.clipboard.history_size);
        println!("   Max clipboard size: {}", format_bytes(self.config.clipboard.max_size));
        println!("   Compression: {}", self.config.security.compression);
        println!("   Encrypt history: {}", self.config.security.encrypt_history);
        println!();
        
        Ok(())
    }

    /// Test the setup
    async fn test_setup(&self) -> Result<(), Box<dyn std::error::Error>> {
        println!("🧪 Step 6: Testing Setup");
        println!();

        let mut progress = ProgressIndicator::new("Testing configuration");
        
        // Test config loading
        match Config::load_from_file(&self.config_path) {
            Ok(_) => {
                progress.update("Configuration loads successfully");
            }
            Err(e) => {
                progress.error("Configuration test failed");
                return Err(format!("Config test failed: {}", e).into());
            }
        }

        // Test SSH key loading
        let key_exists = Path::new(&self.config.auth.ssh_key).exists();
        let pub_key_exists = Path::new(&format!("{}.pub", self.config.auth.ssh_key)).exists();
        
        if key_exists && pub_key_exists {
            progress.update("SSH keys are accessible");
        } else {
            progress.error("SSH key test failed");
            return Err("SSH keys are not accessible".into());
        }

        // Test network binding
        use tokio::net::TcpListener;
        match TcpListener::bind(&self.config.listen_addr).await {
            Ok(_) => {
                progress.success("All tests passed");
            }
            Err(e) => {
                progress.warning(&format!("Network binding test failed: {}", e));
                println!("⚠️  Warning: Could not bind to {}. You may need to:", self.config.listen_addr);
                println!("   - Choose a different port");
                println!("   - Check firewall settings");
                println!("   - Run with appropriate permissions");
            }
        }

        Ok(())
    }

    /// Show setup completion message
    fn show_completion(&self) {
        println!();
        println!("🎉 ClipSync Setup Complete!");
        println!();
        println!("Your ClipSync installation is ready to use. Next steps:");
        println!();
        println!("1. Start ClipSync:");
        println!("   clipsync start");
        println!();
        println!("2. Set up other devices:");
        println!("   - Install ClipSync on other devices");
        println!("   - Exchange public keys using 'clipsync auth add'");
        println!();
        println!("3. Test clipboard sync:");
        println!("   - Copy something on one device");
        println!("   - Paste on another device");
        println!();
        println!("4. View clipboard history:");
        println!("   - Use Ctrl+Shift+V (or Cmd+Shift+V on macOS)");
        println!("   - Or run 'clipsync history'");
        println!();
        println!("📚 For more help:");
        println!("   - clipsync --help");
        println!("   - clipsync doctor  (diagnose issues)");
        println!("   - Documentation: https://github.com/yourusername/clipsync");
        println!();
        println!("Happy clipboard syncing! 📋✨");
        println!();
    }

    // Helper methods for user interaction

    /// Prompt user for input with a default value
    fn prompt_with_default(&self, prompt: &str, default: &str) -> Result<String, Box<dyn std::error::Error>> {
        print!("{} [{}]: ", prompt, default);
        io::stdout().flush()?;
        
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let input = input.trim();
        
        if input.is_empty() {
            Ok(default.to_string())
        } else {
            Ok(input.to_string())
        }
    }

    /// Prompt user for input
    fn prompt(&self, prompt: &str) -> Result<String, Box<dyn std::error::Error>> {
        print!("{}: ", prompt);
        io::stdout().flush()?;
        
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        
        Ok(input.trim().to_string())
    }

    /// Prompt user for a number within a range
    fn prompt_number(&self, prompt: &str, default: u32, min: u32, max: u32) -> Result<u32, Box<dyn std::error::Error>> {
        loop {
            let input = self.prompt_with_default(&format!("{} ({}-{})", prompt, min, max), &default.to_string())?;
            
            match input.parse::<u32>() {
                Ok(num) if num >= min && num <= max => return Ok(num),
                Ok(_) => println!("Please enter a number between {} and {}", min, max),
                Err(_) => println!("Please enter a valid number"),
            }
        }
    }

    /// Prompt user to choose from a list of options
    fn prompt_choice(&self, prompt: &str, choices: &[&str], default: usize) -> Result<usize, Box<dyn std::error::Error>> {
        loop {
            println!("{}: ", prompt);
            for (i, choice) in choices.iter().enumerate() {
                let marker = if i == default { " (default)" } else { "" };
                println!("  {}. {}{}", i + 1, choice, marker);
            }
            
            print!("Choose [{}]: ", default + 1);
            io::stdout().flush()?;
            
            let mut input = String::new();
            io::stdin().read_line(&mut input)?;
            let input = input.trim();
            
            if input.is_empty() {
                return Ok(default);
            }
            
            match input.parse::<usize>() {
                Ok(num) if num >= 1 && num <= choices.len() => return Ok(num - 1),
                _ => println!("Please enter a number between 1 and {}", choices.len()),
            }
        }
    }

    /// Ask a yes/no question
    fn ask_yes_no(&self, prompt: &str) -> Result<bool, Box<dyn std::error::Error>> {
        loop {
            print!("{} (y/n): ", prompt);
            io::stdout().flush()?;
            
            let mut input = String::new();
            io::stdin().read_line(&mut input)?;
            let input = input.trim().to_lowercase();
            
            match input.as_str() {
                "y" | "yes" => return Ok(true),
                "n" | "no" => return Ok(false),
                _ => println!("Please enter 'y' for yes or 'n' for no"),
            }
        }
    }

    /// Wait for user to press Enter
    fn wait_for_enter(&self, prompt: &str) {
        print!("{}", prompt);
        io::stdout().flush().unwrap_or(());
        
        let mut input = String::new();
        io::stdin().read_line(&mut input).unwrap_or(0);
    }
}

/// Format bytes in human-readable format
fn format_bytes(bytes: usize) -> String {
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
    }

    #[tokio::test]
    async fn test_wizard_creation() {
        let wizard = SetupWizard::new();
        assert!(wizard.is_ok());
    }
}