//! Exercises delayed worker delivery across real renderer resets.
#![allow(clippy::expect_used, clippy::panic)]

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
#[path = "reset/desktop.rs"]
mod desktop;

fn main() {
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    desktop::run();
}
