//! Shared state and controller for the desktop and terminal companions.
pub mod app;
pub mod client;
pub mod model;
#[cfg(feature = "tui")]
pub mod terminal;

#[cfg(feature = "tui")]
mod terminal_devices;
