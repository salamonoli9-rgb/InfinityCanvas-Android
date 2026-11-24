//! Infinite canvas drawing app. The same code runs on desktop (winit) and Android
//! (android-activity); only the launcher differs.

pub mod app;
pub mod gestures;
pub mod i18n;
pub mod measure;
pub mod grid;
pub mod photos;
pub mod projects;
pub mod render;
pub mod selection;
pub mod settings;
pub mod shapes;
pub mod text;
pub mod ui;
pub mod video;

#[cfg(not(target_os = "android"))]
pub mod desktop;

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "android")]
mod android_photos;
