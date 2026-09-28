//! Window, event-loop and platform adapters for maplibre.
#![deny(unused_imports)]

mod environment;
mod event_loop;
pub mod input;
mod window;

pub use environment::WinitEnvironment;
pub use event_loop::{RawEventLoopProxy, RawWinitEventLoop, WinitEventLoop, WinitEventLoopProxy};
pub use window::{RawWinitWindow, WinitMapWindow};
#[cfg(target_os = "android")]
pub use winit::platform::android::activity as android_activity;
#[cfg(not(target_arch = "wasm32"))]
mod noweb;
#[cfg(not(target_arch = "wasm32"))]
pub use noweb::*;
#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(target_arch = "wasm32")]
pub use web::*;
