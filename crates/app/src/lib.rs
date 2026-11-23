//! Infinite canvas drawing app. The same code runs on desktop (winit) and Android
//! (android-activity); only the launcher differs.

pub mod app;
pub mod gestures;
pub mod projects;
pub mod render;
pub mod ui;

#[cfg(not(target_os = "android"))]
pub mod desktop;

#[cfg(target_os = "android")]
mod android;
