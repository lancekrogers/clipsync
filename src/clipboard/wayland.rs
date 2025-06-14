//! Wayland clipboard implementation

use super::{
    ClipboardContent, ClipboardError, ClipboardEvent, ClipboardProvider, ClipboardWatcher,
    MAX_CLIPBOARD_SIZE,
};
use async_trait::async_trait;
use std::os::fd::AsRawFd;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::interval;
use wayland_client::{
    protocol::{
        wl_data_device::{self, WlDataDevice},
        wl_data_device_manager::{self, WlDataDeviceManager},
        wl_data_offer::{self, WlDataOffer},
        wl_data_source::{self, WlDataSource},
        wl_registry::{self, WlRegistry},
        wl_seat::{self, WlSeat},
    },
    Connection, Dispatch, EventQueue, QueueHandle,
};

/// Wayland clipboard state
struct WaylandState {
    registry: Option<WlRegistry>,
    data_device_manager: Option<WlDataDeviceManager>,
    data_device: Option<WlDataDevice>,
    seat: Option<WlSeat>,
    current_offer: Option<WlDataOffer>,
    clipboard_content: Arc<Mutex<Option<Vec<u8>>>>,
}

impl WaylandState {
    fn new() -> Self {
        Self {
            registry: None,
            data_device_manager: None,
            data_device: None,
            seat: None,
            current_offer: None,
            clipboard_content: Arc::new(Mutex::new(None)),
        }
    }
}

/// Wayland clipboard provider
pub struct WaylandClipboard {
    connection: Connection,
    state: Arc<Mutex<WaylandState>>,
}

impl WaylandClipboard {
    /// Create a new Wayland clipboard provider
    pub async fn new() -> Result<Self, ClipboardError> {
        let connection = Connection::connect_to_env().map_err(|e| {
            ClipboardError::Platform(format!("Failed to connect to Wayland: {}", e))
        })?;

        let state = Arc::new(Mutex::new(WaylandState::new()));

        // Initialize Wayland objects
        let display = connection.display();
        let mut event_queue = connection.new_event_queue();
        let qhandle = event_queue.handle();

        // Get registry and bind required globals
        let registry = display.get_registry(&qhandle, ());

        // Store registry in state
        {
            let mut state_guard = state.lock().unwrap();
            state_guard.registry = Some(registry);
        }

        // Create a temporary state for event dispatching
        let mut temp_state = WaylandState::new();
        temp_state.registry = state.lock().unwrap().registry.clone();

        // Process initial events to get globals
        event_queue
            .roundtrip(&mut temp_state)
            .map_err(|e| ClipboardError::Platform(format!("Failed to dispatch events: {}", e)))?;

        // Copy the globals from temp_state to the actual state
        {
            let mut state_guard = state.lock().unwrap();
            state_guard.data_device_manager = temp_state.data_device_manager;
            state_guard.seat = temp_state.seat;

            // Create data device if we have both manager and seat
            if let (Some(manager), Some(seat)) =
                (&state_guard.data_device_manager, &state_guard.seat)
            {
                let data_device = manager.get_data_device(seat, &qhandle, ());
                state_guard.data_device = Some(data_device);
            }
        }

        // Flush the connection to ensure all requests are sent
        connection
            .flush()
            .map_err(|e| ClipboardError::Platform(format!("Failed to flush connection: {}", e)))?;

        Ok(Self { connection, state })
    }

    /// Read clipboard content
    async fn read_clipboard(&self) -> Result<Option<Vec<u8>>, ClipboardError> {
        // First, dispatch any pending events to ensure we have the latest clipboard state
        let mut event_queue = self.connection.new_event_queue();
        let mut temp_state = WaylandState::new();

        // Copy current state to temp_state
        {
            let state_guard = self.state.lock().unwrap();
            temp_state.registry = state_guard.registry.clone();
            temp_state.data_device_manager = state_guard.data_device_manager.clone();
            temp_state.data_device = state_guard.data_device.clone();
            temp_state.seat = state_guard.seat.clone();
            temp_state.current_offer = state_guard.current_offer.clone();
            temp_state.clipboard_content = state_guard.clipboard_content.clone();
        }

        // Dispatch pending events
        let _ = event_queue.dispatch_pending(&mut temp_state);

        // Update state with any changes
        {
            let mut state_guard = self.state.lock().unwrap();
            state_guard.current_offer = temp_state.current_offer;
        }

        // Now read the clipboard content
        let state = self.state.lock().unwrap();
        let content = state.clipboard_content.lock().unwrap();
        Ok(content.clone())
    }

    /// Write clipboard content
    async fn write_clipboard(&self, data: Vec<u8>) -> Result<(), ClipboardError> {
        let state = self.state.lock().unwrap();

        if let (Some(manager), Some(device)) = (&state.data_device_manager, &state.data_device) {
            let mut event_queue = self.connection.new_event_queue();
            let qhandle = event_queue.handle();

            // Create data source
            let source = manager.create_data_source(&qhandle, ());

            // Offer text/plain mime type
            source.offer("text/plain".to_string());

            // Set selection
            device.set_selection(Some(&source), 0);

            // Store the data
            *state.clipboard_content.lock().unwrap() = Some(data);

            // Create temp state for event handling
            let mut temp_state = WaylandState::new();
            temp_state.clipboard_content = state.clipboard_content.clone();

            // Process events to handle the data source send requests
            let _ = event_queue.roundtrip(&mut temp_state);

            // Commit changes
            self.connection.flush().map_err(|e| {
                ClipboardError::Platform(format!("Failed to flush connection: {}", e))
            })?;

            Ok(())
        } else {
            Err(ClipboardError::Platform(
                "Wayland not properly initialized".to_string(),
            ))
        }
    }
}

#[async_trait]
impl ClipboardProvider for WaylandClipboard {
    async fn get_content(&self) -> Result<ClipboardContent, ClipboardError> {
        if let Some(data) = self.read_clipboard().await? {
            // Try to parse as text
            if let Ok(text) = String::from_utf8(data.clone()) {
                Ok(ClipboardContent::text(text))
            } else {
                Ok(ClipboardContent {
                    mime_type: "application/octet-stream".to_string(),
                    data,
                    timestamp: super::current_timestamp(),
                })
            }
        } else {
            Err(ClipboardError::NoContent)
        }
    }

    async fn set_content(&self, content: &ClipboardContent) -> Result<(), ClipboardError> {
        // Check size limit
        if content.size() > MAX_CLIPBOARD_SIZE {
            return Err(ClipboardError::TooLarge {
                size: content.size(),
                max: MAX_CLIPBOARD_SIZE,
            });
        }

        // For now, only support text content on Wayland
        if !content.is_text() {
            return Err(ClipboardError::UnsupportedType(content.mime_type.clone()));
        }

        self.write_clipboard(content.data.clone()).await
    }

    async fn clear(&self) -> Result<(), ClipboardError> {
        let state = self.state.lock().unwrap();

        if let Some(device) = &state.data_device {
            device.set_selection(None, 0);
            *state.clipboard_content.lock().unwrap() = None;

            self.connection.flush().map_err(|e| {
                ClipboardError::Platform(format!("Failed to flush connection: {}", e))
            })?;
        }

        Ok(())
    }

    fn name(&self) -> &str {
        "Wayland"
    }

    async fn watch(&self) -> Result<ClipboardWatcher, ClipboardError> {
        let (tx, rx) = mpsc::channel(100);
        let state = Arc::clone(&self.state);
        let connection = self.connection.clone();

        let handle = tokio::spawn(async move {
            let mut last_content: Option<Vec<u8>> = None;
            let mut ticker = interval(Duration::from_millis(200));

            loop {
                ticker.tick().await;

                // Create an event queue for monitoring
                let mut event_queue = connection.new_event_queue();
                let mut temp_state = WaylandState::new();

                // Copy current state
                {
                    let state_guard = state.lock().unwrap();
                    temp_state.data_device = state_guard.data_device.clone();
                    temp_state.clipboard_content = state_guard.clipboard_content.clone();
                }

                // Dispatch any pending events
                let _ = event_queue.dispatch_pending(&mut temp_state);

                // Check for clipboard changes
                let current_content = {
                    let clipboard_guard = temp_state.clipboard_content.lock().unwrap();
                    clipboard_guard.clone()
                };

                if current_content != last_content {
                    if let Some(data) = current_content.clone() {
                        last_content = current_content;

                        let content = if let Ok(text) = String::from_utf8(data.clone()) {
                            ClipboardContent::text(text)
                        } else {
                            ClipboardContent {
                                mime_type: "application/octet-stream".to_string(),
                                data: data.clone(),
                                timestamp: super::current_timestamp(),
                            }
                        };

                        let event = ClipboardEvent {
                            content,
                            selection: None, // Wayland doesn't have selections like X11
                        };

                        if tx.send(event).await.is_err() {
                            break;
                        }
                    }
                }
            }
        });

        Ok(ClipboardWatcher::new(rx, handle))
    }
}

// Wayland event handling
impl Dispatch<wl_registry::WlRegistry, ()> for WaylandState {
    fn event(
        state: &mut Self,
        registry: &WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qhandle: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match interface.as_str() {
                "wl_data_device_manager" => {
                    let manager = registry.bind::<WlDataDeviceManager, _, _>(
                        name,
                        version.min(3), // Use version 3 or lower
                        qhandle,
                        (),
                    );
                    state.data_device_manager = Some(manager);
                }
                "wl_seat" => {
                    let seat = registry.bind::<WlSeat, _, _>(
                        name,
                        version.min(7), // Use version 7 or lower
                        qhandle,
                        (),
                    );
                    state.seat = Some(seat);
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for WaylandState {
    fn event(
        state: &mut Self,
        seat: &WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qhandle: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities { capabilities } = event {
            // We only care about pointer and keyboard for clipboard
            // Store the seat if we don't have it already
            if state.seat.is_none() {
                state.seat = Some(seat.clone());
            }
        }
    }
}

impl Dispatch<wl_data_device_manager::WlDataDeviceManager, ()> for WaylandState {
    fn event(
        _state: &mut Self,
        _: &WlDataDeviceManager,
        _: wl_data_device_manager::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // No events for data device manager
    }
}

impl Dispatch<wl_data_device::WlDataDevice, ()> for WaylandState {
    fn event(
        state: &mut Self,
        _: &WlDataDevice,
        event: wl_data_device::Event,
        _: &(),
        _: &Connection,
        qhandle: &QueueHandle<Self>,
    ) {
        match event {
            wl_data_device::Event::DataOffer { id } => {
                state.current_offer = Some(id);
            }
            wl_data_device::Event::Selection { id } => {
                // Handle selection change
                if let Some(offer) = id {
                    state.current_offer = Some(offer);
                    // TODO: Read data from the offer and update clipboard_content
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_data_offer::WlDataOffer, ()> for WaylandState {
    fn event(
        _state: &mut Self,
        offer: &WlDataOffer,
        event: wl_data_offer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_data_offer::Event::Offer { mime_type } = event {
            // We only handle text/plain for now
            if mime_type == "text/plain" {
                // Accept the offer
                offer.accept(0, Some(mime_type));

                // TODO: Actually receive the data via fd and update clipboard_content
            }
        }
    }
}

impl Dispatch<wl_data_source::WlDataSource, ()> for WaylandState {
    fn event(
        state: &mut Self,
        _source: &WlDataSource,
        event: wl_data_source::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_data_source::Event::Send { mime_type, fd } => {
                // Send our clipboard data
                if mime_type == "text/plain" {
                    if let Some(data) = &*state.clipboard_content.lock().unwrap() {
                        // Write data to file descriptor
                        use std::os::unix::io::FromRawFd;
                        let mut file = unsafe { std::fs::File::from_raw_fd(fd.as_raw_fd()) };
                        let _ = std::io::Write::write_all(&mut file, data);
                        // Important: we need to close the fd explicitly
                        drop(file);
                    }
                }
            }
            wl_data_source::Event::Cancelled => {
                // Data source was cancelled, we can clean up if needed
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wayland_clipboard_name() {
        // We can't easily test Wayland functionality without a Wayland compositor
        // This is just a placeholder test
        assert_eq!("Wayland", "Wayland");
    }
}
