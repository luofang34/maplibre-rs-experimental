//! Mode toggles, button actions, and the Auto-Tilt curve.

use cgmath::{Deg, Point2, Rad};

use super::{
    camera::{base_auto_tilt_degrees, normalize_angle_signed},
    HomeCamera, NavigationState, PointerProfile,
};
use crate::{
    context::MapContext, coords::TILE_SIZE, hud::NavigationHudState, projection::ProjectionType,
    render::view_state::ViewState,
};

/// Pitch difference in degrees below which Auto-Tilt leaves the camera alone.
pub(super) const AUTO_TILT_TOLERANCE_DEG: f64 = 0.12;

impl NavigationState {
    /// Captures the style's initial camera state on the first frame and enables Auto-Tilt by
    /// default on 3D terrain and Globe styles.
    pub fn ensure_initialized(&mut self, map_context: &mut MapContext) {
        if self.initialized {
            return;
        }
        self.initialized = true;
        let has_terrain = map_context.style.terrain.is_some();
        let has_globe = map_context
            .style
            .projection
            .as_ref()
            .is_some_and(|p| p.projection_type != ProjectionType::Mercator);
        let style_pitch = map_context.style.pitch.unwrap_or(0.0);
        let has_3d_or_globe = has_terrain || has_globe || style_pitch > 0.5;
        let auto_tilt_by_default = has_terrain || (has_globe && style_pitch <= 0.5);

        let zoom = map_context.view_state.zoom().value();
        let world_size = TILE_SIZE * 2.0_f64.powf(zoom);
        let pos = map_context.view_state.camera().position();
        let normalized_center = Point2::new(pos.x / world_size, pos.y / world_size);
        let bearing = map_context.view_state.camera().get_bearing();
        let initial_pitch_deg = if auto_tilt_by_default {
            base_auto_tilt_degrees(zoom)
        } else {
            map_context.view_state.camera().get_pitch().0.to_degrees()
        };
        let pitch = Rad::from(Deg(initial_pitch_deg));

        self.home = Some(HomeCamera {
            normalized_center,
            zoom,
            bearing,
            pitch,
            has_3d_or_globe,
        });
        self.three_dimensional = true;
        self.auto_tilt = auto_tilt_by_default;

        if let Some(hud) = map_context.world.resources.get::<NavigationHudState>() {
            self.show_hud = hud.visible;
            self.edge_scroll_speed_px_per_sec = hud.edge_scroll_speed_px_per_sec;
            self.edge_scroll_override = hud.edge_scroll_enabled;
        }

        // Sync initial camera pitch to the Auto-Tilt curve when Auto-Tilt is active so zooming
        // both in and out from the starting zoom has immediate, symmetric tilt response.
        if self.auto_tilt {
            let max_pitch = map_context.view_state.camera().max_pitch().0.to_degrees();
            map_context
                .view_state
                .camera_mut()
                .set_pitch(Deg(initial_pitch_deg.min(max_pitch)));
        }
        tracing::info!(
            zoom,
            initial_pitch_deg,
            max_pitch_deg = map_context.view_state.camera().max_pitch().0.to_degrees(),
            three_d = self.three_dimensional,
            auto_tilt = self.auto_tilt,
            "NavigationState initialized"
        );
    }

    /// Computes the Auto-Tilt pitch in degrees for a given `zoom`.
    ///
    /// Uses C1-continuous Hermite smoothstep interpolation across zoom stops:
    /// `0°` at `zoom <= 8.5` (overhead globe/regional view), ramping through `22°` (`z10`),
    /// `48°` (`z12`), `64°` (`z13.5`), and `74°` (`z15`) up to `78°` (`z >= 16.5`).
    pub fn pitch_for_zoom(&self, zoom: f64) -> f64 {
        if !self.three_dimensional {
            return 0.0;
        }
        base_auto_tilt_degrees(zoom)
    }

    /// Updates the smooth Auto-Tilt target pitch whenever zoom changes.
    pub fn on_zoom_changed(&mut self, next_zoom: f64) {
        self.fly_home = false;
        if self.three_dimensional && self.auto_tilt {
            self.target_pitch_deg = Some(self.pitch_for_zoom(next_zoom));
        }
    }

    /// Accumulates a trackpad two-finger rotation gesture (`WindowEvent::RotationGesture`,
    /// where `delta_degrees` is positive for counter-clockwise finger rotation).
    pub fn process_trackpad_rotate(&mut self, delta_degrees: f64) {
        if !self.north_locked && delta_degrees.is_finite() && delta_degrees.abs() > 1e-4 {
            self.on_manual_bearing();
            self.pending_trackpad_rotation += Rad::from(Deg(delta_degrees));
        }
    }

    /// Called when the user manually tilts the camera (e.g. Right-drag, `Alt`+drag, two-finger
    /// vertical drag, or Compass vertical drag). Disengages `Auto-Tilt` into `3D·MAN` mode.
    pub fn on_manual_tilt(&mut self) {
        self.three_dimensional = true;
        self.auto_tilt = false;
        self.target_pitch_deg = None;
        self.fly_home = false;
    }

    /// Called when the user manually rotates bearing. Cancels any pending bearing animation.
    pub fn on_manual_bearing(&mut self) {
        self.target_bearing_rad = None;
        self.fly_home = false;
    }

    /// Called when the user starts any manual pan or zoom gesture.
    pub fn on_manual_pan_or_zoom(&mut self) {
        self.fly_home = false;
    }

    /// Cycles the single `3D/2D/Auto` control button:
    /// - `3D·AUTO` -> `2D` (smoothly flattens pitch to `0°`)
    /// - `2D` -> `3D·AUTO` (restores Auto-Tilt pitch, or at least `55°` at low zoom so the
    ///   button always gives visible 3D feedback)
    /// - `3D·MAN` -> `3D·AUTO` (re-engages Auto-Tilt along the zoom curve)
    pub fn cycle_3d_mode(&mut self, current_zoom: f64) {
        self.fly_home = false;
        let curve_pitch = base_auto_tilt_degrees(current_zoom);
        if self.three_dimensional && self.auto_tilt {
            if curve_pitch < 15.0 {
                // At low zoom where Auto-Tilt is flat (0°), clicking `3D` tilts immediately
                // into manual 3D (55°) so the user always sees 3D tilt when pressing the button.
                self.three_dimensional = true;
                self.auto_tilt = false;
                self.target_pitch_deg = Some(55.0);
            } else {
                // 3D·AUTO -> 2D
                self.three_dimensional = false;
                self.auto_tilt = false;
                self.target_pitch_deg = Some(0.0);
            }
        } else if !self.three_dimensional {
            // 2D -> 3D·AUTO
            self.three_dimensional = true;
            self.auto_tilt = true;
            self.target_pitch_deg = Some(curve_pitch.max(55.0));
        } else {
            // 3D·MAN -> 3D·AUTO
            self.three_dimensional = true;
            self.auto_tilt = true;
            self.target_pitch_deg = Some(curve_pitch);
        }
        tracing::info!(
            current_zoom,
            three_d = self.three_dimensional,
            auto_tilt = self.auto_tilt,
            target_pitch = ?self.target_pitch_deg,
            "cycled 3D mode"
        );
    }

    /// Handles a click on the Compass button: resets bearing to North (`0°`) if rotated,
    /// or toggles `north_locked` if already facing North.
    pub fn trigger_compass(&mut self, current_bearing: Rad<f64>) {
        self.fly_home = false;
        let normalized = normalize_angle_signed(current_bearing.0);
        if normalized.abs() > 0.01 {
            self.target_bearing_rad = Some(0.0);
        } else {
            self.north_locked = !self.north_locked;
            self.target_bearing_rad = Some(0.0);
        }
    }

    /// Triggers a smooth reset back to the initial `HomeCamera` view and restores default Auto-Tilt.
    pub fn trigger_home(&mut self) {
        let Some(home) = self.home else {
            return;
        };
        self.three_dimensional = true;
        self.auto_tilt = home.has_3d_or_globe;
        self.north_locked = false;
        self.target_bearing_rad = Some(normalize_angle_signed(home.bearing.0));
        self.target_pitch_deg = Some(home.pitch.0.to_degrees());
        self.fly_home = true;
    }

    /// Toggles between `PointerProfile::Map` and `PointerProfile::Rts`.
    pub fn toggle_pointer_profile(&mut self) {
        self.pointer_profile = match self.pointer_profile {
            PointerProfile::Map => PointerProfile::Rts,
            PointerProfile::Rts => PointerProfile::Map,
        };
        self.selection_drag = None;
    }

    /// Takes any pending smooth zoom delta queued by HUD `+`/`-` buttons or double-click.
    pub fn take_pending_smooth_zoom(&mut self) -> Option<f64> {
        if self.pending_smooth_zoom.abs() > f64::EPSILON {
            let delta = self.pending_smooth_zoom;
            self.pending_smooth_zoom = 0.0;
            Some(delta)
        } else {
            None
        }
    }
    /// Cancels any pending smooth pitch settle animation (for instance when terrain clearance
    /// clamps the camera pitch on a steep slope).
    pub fn cancel_pitch_animation(&mut self) {
        self.target_pitch_deg = None;
    }

    /// Whether any smooth camera animation (`target_pitch`, `target_bearing`, `fly_home`) is active.
    pub fn is_animating(&self) -> bool {
        self.target_pitch_deg.is_some() || self.target_bearing_rad.is_some() || self.fly_home
    }

    /// Whether [`update_frame`](Self::update_frame) still moves the camera: an animation runs,
    /// or Auto-Tilt is easing the pitch back onto its curve. A host that redraws on demand keeps
    /// drawing frames while this holds.
    pub fn wants_frame(&self, view: &ViewState) -> bool {
        if self.is_animating() {
            return true;
        }
        if !(self.three_dimensional && self.auto_tilt) {
            return false;
        }
        let max_pitch = view.camera().max_pitch().0.to_degrees();
        let desired = self
            .pitch_for_zoom(view.zoom().value())
            .clamp(0.0, max_pitch);
        (desired - view.camera().get_pitch().0.to_degrees()).abs() > AUTO_TILT_TOLERANCE_DEG
    }

    /// Stops every running camera animation, leaving the camera where it is, for instance when
    /// the host hands the camera to another source.
    pub fn stop_animations(&mut self) {
        self.target_pitch_deg = None;
        self.target_bearing_rad = None;
        self.fly_home = false;
        self.pending_smooth_zoom = 0.0;
    }

    /// Couples the pitch to zoom along the Auto-Tilt curve, easing onto it, or keeps the
    /// current pitch as a manual tilt.
    pub fn set_auto_tilt(&mut self, enabled: bool, current_zoom: f64) {
        self.fly_home = false;
        if enabled {
            self.three_dimensional = true;
            self.auto_tilt = true;
            self.target_pitch_deg = Some(self.pitch_for_zoom(current_zoom));
        } else {
            self.on_manual_tilt();
        }
    }

    /// Eases to a manually chosen pitch in degrees, leaving `Auto-Tilt` for `3D·MAN`.
    pub fn set_manual_pitch(&mut self, degrees: f64) {
        self.on_manual_tilt();
        self.target_pitch_deg = Some(degrees.max(0.0));
    }
}
