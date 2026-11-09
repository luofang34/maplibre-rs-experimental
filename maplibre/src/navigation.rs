//! Navigation controls shared by every map host: 2D/3D mode, Auto-Tilt, North-Lock, the RTS
//! pointer profile, HUD button interaction, and smooth camera settling.
//!
//! A host feeds pointer positions in logical pixels, maps its buttons and gestures onto the
//! [`NavigationState`] actions, and calls [`NavigationState::update_frame`] once per frame, so
//! the winit and web hosts and embedders that draw their own controls behave the same.

use cgmath::{Point2, Rad, Vector2};
use instant::Instant;

use crate::hud::HudButton;

mod camera;
mod frame;
mod modes;
mod pointer;
pub mod projection;

pub use camera::{
    normalize_angle_signed, rotate_bearing_around_cursor, set_bearing_keeping_center,
    set_pitch_keeping_center,
};

#[cfg(test)]
mod tests;

/// Pointer interaction semantics for mouse/trackpad input.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PointerProfile {
    /// Standard map controls: Left-drag pans (`Shift`+Left-drag box-selects), Right-drag orbits.
    #[default]
    Map,
    /// RTS controls (RA2 / C&C Generals style): Left-drag box-selects, Right-drag pans,
    /// Middle-drag or `Alt`+drag orbits, and screen edges scroll the viewport.
    Rts,
}

/// Snapshot of the initial camera pose used by the `Home` reset action and Auto-Tilt calibration.
#[derive(Clone, Copy, Debug)]
pub struct HomeCamera {
    /// Normalized Mercator coordinates in `0..1`.
    pub normalized_center: Point2<f64>,
    /// Initial zoom level.
    pub zoom: f64,
    /// Initial camera bearing.
    pub bearing: Rad<f64>,
    /// Initial camera pitch.
    pub pitch: Rad<f64>,
    /// Whether the style uses 3D terrain or a non-Mercator globe projection.
    pub has_3d_or_globe: bool,
}

/// Shared state across input handlers for 2D/3D mode, Auto-Tilt, North-Lock, RTS mode,
/// HUD button interactions, and smooth settle animations.
pub struct NavigationState {
    initialized: bool,
    home: Option<HomeCamera>,
    /// Whether 3D pitch is allowed (`true` = 3D, `false` = locked 2D top-down).
    pub three_dimensional: bool,
    /// Whether zoom gestures automatically adjust camera pitch along the zoom-tilt curve.
    pub auto_tilt: bool,
    /// Whether camera bearing is locked to North (`0°`).
    pub north_locked: bool,
    /// Active mouse pointer profile (`Map` vs `Rts`).
    pub pointer_profile: PointerProfile,
    /// Whether the floating top-right navigation HUD is visible.
    pub show_hud: bool,
    /// Latest cursor position in logical window pixels.
    pub cursor_position: Option<Vector2<f64>>,
    /// Currently hovered HUD button.
    pub hovered_button: Option<HudButton>,
    /// Currently pressed HUD button.
    pub pressed_button: Option<HudButton>,
    /// Initial press position when interacting with the HUD compass, to distinguish click vs drag.
    compass_press_start: Option<Vector2<f64>>,
    /// Previous cursor position while dragging the HUD compass.
    compass_drag_last: Option<Vector2<f64>>,
    /// Whether the current HUD compass press turned into a drag gesture.
    compass_dragged: bool,
    /// Active rubber-band selection box (`start_px`, `current_px`) in logical window pixels.
    pub selection_drag: Option<(Vector2<f64>, Vector2<f64>)>,
    /// Smoothly animated target pitch in degrees.
    target_pitch_deg: Option<f64>,
    /// Smoothly animated target bearing in radians.
    target_bearing_rad: Option<f64>,
    /// Whether a smooth fly-to-home animation is running.
    fly_home: bool,
    /// Smooth zoom delta requested by HUD buttons or double-click, consumed by `ZoomHandler`.
    pending_smooth_zoom: f64,
    /// Last left-click / tap time and logical pixel position for double-click detection.
    last_primary_click: Option<(Instant, Vector2<f64>)>,
    /// Pending trackpad rotation gesture delta in radians (`WindowEvent::RotationGesture`).
    pending_trackpad_rotation: Rad<f64>,
    /// Whether `Shift` modifier is currently held.
    pub shift_held: bool,
    /// Whether `Alt` / `Option` modifier is currently held.
    pub alt_held: bool,
    /// Whether `Control` / `Super` modifier is currently held.
    pub ctrl_held: bool,
    /// Whether screen-edge scrolling is explicitly enabled even in windowed Map mode.
    pub edge_scroll_override: bool,
    /// Maximum screen-edge scrolling speed in logical pixels per second.
    pub edge_scroll_speed_px_per_sec: f64,
}

impl Default for NavigationState {
    fn default() -> Self {
        Self {
            initialized: false,
            home: None,
            three_dimensional: true,
            auto_tilt: false,
            north_locked: false,
            pointer_profile: PointerProfile::Map,
            show_hud: true,
            cursor_position: None,
            hovered_button: None,
            pressed_button: None,
            compass_press_start: None,
            compass_drag_last: None,
            compass_dragged: false,
            selection_drag: None,
            target_pitch_deg: None,
            target_bearing_rad: None,
            fly_home: false,
            pending_smooth_zoom: 0.0,
            last_primary_click: None,
            pending_trackpad_rotation: Rad(0.0),
            shift_held: false,
            alt_held: false,
            ctrl_held: false,
            edge_scroll_override: false,
            edge_scroll_speed_px_per_sec: 1100.0,
        }
    }
}
