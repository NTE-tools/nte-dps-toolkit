//! Windows / OS integration shared by the Rust core and Tauri adapter.

#[cfg(feature = "desktop")]
pub mod game_directory;
pub mod locale;
pub mod network;
#[cfg(all(windows, feature = "desktop"))]
pub mod passthrough_hotkey;
#[cfg(any(feature = "desktop", feature = "cli"))]
pub mod toolkit;
#[cfg(feature = "desktop")]
pub mod update_http;
#[cfg(feature = "desktop")]
pub mod update_install;
#[cfg(all(windows, feature = "desktop"))]
pub mod window_style;

#[cfg(any(feature = "desktop", feature = "cli"))]
pub mod capture_pipe;
