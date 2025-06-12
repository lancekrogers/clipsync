//! Example demonstrating clipboard operations

use clipsync::clipboard::{create_provider, ClipboardContent};
use std::time::Duration;
use tokio::time::sleep;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("ClipSync Clipboard Demo");
    println!("=======================\n");

    // Create clipboard provider for current platform
    let provider = create_provider().await?;
    println!("Using clipboard provider: {}", provider.name());

    // Example 1: Set and get text
    println!("\n1. Setting text to clipboard...");
    let text_content = ClipboardContent::text("Hello from ClipSync!");
    provider.set_content(&text_content).await?;
    println!("   ✓ Set text: 'Hello from ClipSync!'");

    // Read it back
    let retrieved = provider.get_content().await?;
    if let Some(text) = retrieved.as_text() {
        println!("   ✓ Retrieved text: '{}'", text);
    }

    // Example 2: Watch for clipboard changes
    println!("\n2. Starting clipboard watcher...");
    println!("   Copy some text to see it detected!");
    println!("   Press Ctrl+C to stop\n");

    let mut watcher = provider.watch().await?;

    // Listen for clipboard changes
    tokio::select! {
        _ = async {
            while let Some(event) = watcher.receiver.recv().await {
                println!("Clipboard changed!");
                if let Some(text) = event.content.as_text() {
                    println!("  Type: text/plain");
                    println!("  Content: {}", text);
                    println!("  Size: {} bytes", event.content.size());
                } else {
                    println!("  Type: {}", event.content.mime_type);
                    println!("  Size: {} bytes", event.content.size());
                }

                if let Some(selection) = event.selection {
                    println!("  Selection: {:?}", selection);
                }
                println!();
            }
        } => {},
        _ = tokio::signal::ctrl_c() => {
            println!("\nStopping watcher...");
        }
    }

    println!("Demo complete!");
    Ok(())
}
