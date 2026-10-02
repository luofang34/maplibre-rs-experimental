//! The shipped panels — PFD, HSI, autoflight annunciator, and monitor —
//! as pure state→scene functions (ADR-0017).
//!
//! Each panel is a function from resolved display state
//! ([`indicate_instrument_state::PanelData`]) and a logical frame to
//! abstract drawing commands
//! ([`indicate_instrument_scene::SceneWriter`]); no panel knows what
//! renders it. Every panel here declares [`BUILTIN_FRAME`] as its floor,
//! its ceiling, and its one canonical size (the Garmin-G5 proportions
//! the geometry constants come from), so the frame it is drawn at is
//! always that one; backends scale it to their viewport.
//!
//! Signal statuses are honored, never hidden: `Missing` renders dashes,
//! `Stale`/`Degraded` render amber flags, `Failed` renders a red X in
//! place of the instrument (the pyG5 reference's single avionics-off flag
//! is exactly the shortfall this replaces).

#![no_std]

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod alert_stack_tests;
mod autoflight;
mod config;
mod descriptors;
mod hsi;
mod monitor;
mod pfd;

use indicate_instrument_descriptor::DesignFrame;

pub use autoflight::draw_autoflight;
pub use config::draw_config;
pub use descriptors::{
    ALL_SETS, AUTOFLIGHT_DESCRIPTOR, BUILTIN_CRITICALITY_BANDS, BUILTIN_PANELS,
    BUILTIN_SCENE_DIGEST, BUILTIN_SET, CONFIG_DESCRIPTOR, CONFIG_PANELS, CONFIG_SET,
    HSI_DESCRIPTOR, MONITOR_DESCRIPTOR, PFD_DESCRIPTOR,
};
pub use hsi::draw_hsi;
pub use monitor::draw_monitor;
pub use pfd::{BackgroundMode, PFD_CONFIG_SCHEMA, PfdConfig, SvsViewport, VSpeeds, draw_pfd};

/// Logical width of [`BUILTIN_FRAME`].
pub const PANEL_W: f32 = 480.0;

/// Logical height of [`BUILTIN_FRAME`].
pub const PANEL_H: f32 = 360.0;

/// The one frame every panel in this set declares and is drawn at.
///
/// Panel geometry reads the frame it is handed, never this constant.
/// The declared range is degenerate, so the two are the same value at
/// every call a shell can make — which is what keeps the constant an
/// honest name for the shipped size rather than a second source of it.
pub const BUILTIN_FRAME: DesignFrame = DesignFrame {
    width: PANEL_W,
    height: PANEL_H,
};
