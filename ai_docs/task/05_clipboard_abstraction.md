# Task 05: Clipboard Abstraction Layer

## Objective
Create a platform-agnostic clipboard interface with implementations for macOS, X11, and Wayland.

## Steps

1. **Define clipboard trait in src/clipboard/mod.rs**
   ```rust
   use async_trait::async_trait;
   
   #[async_trait]
   pub trait ClipboardProvider: Send + Sync {
       async fn get_content(&self) -> Result<ClipboardContent>;
       async fn set_content(&self, content: &ClipboardContent) -> Result<()>;
       async fn clear(&self) -> Result<()>;
       fn name(&self) -> &str;
   }
   
   #[derive(Debug, Clone)]
   pub struct ClipboardContent {
       pub mime_type: String,
       pub data: Vec<u8>,
       pub timestamp: i64,
   }
   ```

2. **Implement macOS provider (src/clipboard/macos.rs)**
   - Use `cocoa` and `objc` crates
   - Handle NSPasteboard
   - Support text, RTF, and images
   - Implement clipboard change detection

3. **Implement X11 provider (src/clipboard/x11.rs)**
   - Use `x11-clipboard` crate
   - Handle PRIMARY and CLIPBOARD selections
   - Implement polling for changes

4. **Implement Wayland provider (src/clipboard/wayland.rs)**
   - Use `wayland-client` crate
   - Handle wl_data_device_manager
   - Listen for data_device events

5. **Create clipboard factory**
   - Auto-detect platform
   - Fall back gracefully (Wayland -> X11 on Linux)
   - Return appropriate implementation

6. **Add clipboard monitoring**
   - Create watcher that polls/listens for changes
   - Debounce rapid changes
   - Filter out self-triggered updates

## Success Criteria
- Clipboard operations work on all platforms
- Change detection is reliable
- Large payloads (5MB) handled correctly
- MIME types preserved accurately