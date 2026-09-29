//! Exercises resumed and suspended callbacks with an actual desktop window and GPU surface.
#![allow(clippy::expect_used, clippy::panic)]

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
#[path = "lifecycle/desktop.rs"]
mod desktop;

fn main() {
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    desktop::run();
}
