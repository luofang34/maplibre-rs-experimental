//! Main (platform-specific) main loop which handles:
//! * Input (Mouse/Keyboard)
//! * Platform Events like suspend/resume
//! * Render a new frame

mod startup;
pub use startup::{
    default_plugins, run_headed_map, run_headed_map_with, HeadedMapError, HeadedMapHost, MapPlugin,
    NativeEnvironment,
};

/// Runtime options of a windowed map that the style does not carry.
#[derive(Clone, Copy, Debug)]
pub struct HeadedMapOptions {
    /// Ends the event loop after this many rendered frames, so a smoke test can drive the real
    /// windowed pipeline without input.
    pub max_frames: Option<u64>,
    /// Largest camera pitch in degrees, matching the GL JS `maxPitch` map option.
    pub max_pitch_degrees: f64,
    /// Outlines every tile of the view pattern in red, to inspect tile selection.
    pub debug_tiles: bool,
    /// Shows the interactive floating navigation HUD controls in the top-right corner.
    pub show_navigation_controls: bool,
    /// Multiplier on scroll-wheel / trackpad zoom rate (`1.0` matches GL JS's default rate).
    pub zoom_sensitivity: f64,
    /// Screen-edge scrolling speed in logical pixels per second in RTS / edge-scroll mode.
    pub edge_scroll_speed_px_per_sec: f64,
    /// Enables screen-edge scrolling even when in standard Map pointer profile.
    pub edge_scroll_enabled: bool,
}

impl Default for HeadedMapOptions {
    fn default() -> Self {
        Self {
            max_frames: None,
            max_pitch_degrees: maplibre::render::camera::DEFAULT_MAX_PITCH.0,
            debug_tiles: false,
            show_navigation_controls: true,
            zoom_sensitivity: 1.0,
            edge_scroll_speed_px_per_sec: 1100.0,
            edge_scroll_enabled: false,
        }
    }
}
