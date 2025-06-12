# Task 12: Hotkey Support

## Objective
Implement global hotkey support for clipboard history navigation and sync control.

## Steps

1. **Create src/hotkey/mod.rs**
   - Platform-specific hotkey registration
   - Event handling
   - Hotkey configuration

2. **Implement hotkey manager**
   ```rust
   pub struct HotkeyManager {
       handlers: HashMap<Hotkey, Box<dyn Fn() + Send>>,
       #[cfg(target_os = "macos")]
       tap: Option<CGEventTap>,
       #[cfg(target_os = "linux")]
       connection: Option<XConnection>,
   }
   
   impl HotkeyManager {
       pub fn new() -> Result<Self>;
       pub fn register(&mut self, hotkey: Hotkey, handler: impl Fn() + Send + 'static) -> Result<()>;
       pub fn unregister(&mut self, hotkey: &Hotkey) -> Result<()>;
       pub fn start(&mut self) -> Result<()>;
   }
   ```

3. **Add macOS implementation**
   - Use CGEventTap for global hotkeys
   - Handle modifier keys correctly
   - Request accessibility permissions

4. **Add Linux implementation**
   - X11: Use XGrabKey
   - Wayland: Use compositor-specific protocols
   - Fall back gracefully

5. **Implement hotkey actions**
   - Toggle sync on/off
   - Show history picker UI
   - Cycle previous/next in history
   - Clear clipboard

6. **Create history picker**
   - Terminal UI with tui-rs
   - Show last 20 entries
   - Search/filter support
   - Preview content

## Success Criteria
- Hotkeys work globally
- No conflicts with system hotkeys
- History picker is responsive
- Works without accessibility permissions (degraded)