//! Unified cross-platform navigation state machine, Auto-Tilt curve, HUD interaction,
//! and smooth camera settle animations.

use std::time::Duration;

use cgmath::{Deg, EuclideanSpace, InnerSpace, Point2, Rad, Vector2};
use instant::Instant;
use maplibre::{
    context::MapContext,
    coords::{Zoom, TILE_SIZE},
    hud::{HudButton, NavigationHudState, HUD_HEIGHT, HUD_WIDTH},
    projection::ProjectionType,
    render::view_state::ViewState,
    terrain::interaction::{center_pixel, set_location_at_pixel},
};

use super::projection::{
    pan_globe_by_pixels, pan_plane_by_pixels, zoom_globe_around_pixel_with_pitch,
};

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

    /// Whether the given logical pixel lies inside the floating HUD capsule.
    pub fn is_over_hud(&self, view_state: &ViewState, cursor: Vector2<f64>) -> bool {
        if !self.show_hud {
            return false;
        }
        let origin = NavigationHudState::panel_origin(
            view_state.width(),
            view_state.height(),
            view_state.edge_insets(),
        );
        let rel = cursor - origin;
        rel.x >= -2.0 && rel.x <= HUD_WIDTH + 2.0 && rel.y >= -2.0 && rel.y <= HUD_HEIGHT + 2.0
    }

    /// Updates cursor hover and active HUD compass drag / selection rectangle.
    /// Returns `true` if the HUD consumed the cursor movement.
    pub fn process_cursor_moved(
        &mut self,
        position: Vector2<f64>,
        view_width: f64,
        view_height: f64,
    ) -> bool {
        self.cursor_position = Some(position);

        if let Some((start, _)) = self.selection_drag {
            self.selection_drag = Some((start, position));
        }

        if self.pressed_button == Some(HudButton::Compass) {
            if let Some(start) = self.compass_press_start {
                if (position - start).magnitude() > 3.5 {
                    self.compass_dragged = true;
                }
            }
            return true;
        }

        if self.show_hud && view_width > 0.0 && view_height > 0.0 {
            let insets = maplibre::render::camera::EdgeInsets::default();
            self.hovered_button =
                NavigationHudState::hit_test(position, view_width, view_height, &insets);
        } else {
            self.hovered_button = None;
        }

        self.pressed_button.is_some()
    }

    /// Handles a primary (left) press or touch start at `position`.
    /// Returns `true` if the floating HUD intercepted the press.
    pub fn process_hud_press(
        &mut self,
        position: Vector2<f64>,
        view_width: f64,
        view_height: f64,
    ) -> bool {
        if !self.show_hud || view_width <= 0.0 || view_height <= 0.0 {
            return false;
        }
        let insets = maplibre::render::camera::EdgeInsets::default();
        if let Some(button) =
            NavigationHudState::hit_test(position, view_width, view_height, &insets)
        {
            self.pressed_button = Some(button);
            self.hovered_button = Some(button);
            if button == HudButton::Compass {
                self.compass_press_start = Some(position);
                self.compass_drag_last = Some(position);
                self.compass_dragged = false;
            }
            return true;
        }
        false
    }

    /// Handles releasing a primary (left) press or touch end.
    /// Returns `true` if a HUD button was active and handled the release.
    pub fn process_hud_release(&mut self, current_zoom: f64, current_bearing: Rad<f64>) -> bool {
        let Some(button) = self.pressed_button.take() else {
            return false;
        };
        let was_compass_drag = self.compass_dragged;
        self.compass_press_start = None;
        self.compass_drag_last = None;
        self.compass_dragged = false;

        match button {
            HudButton::Compass => {
                if !was_compass_drag {
                    self.trigger_compass(current_bearing);
                }
            }
            HudButton::Mode3D => self.cycle_3d_mode(current_zoom),
            HudButton::ZoomIn => {
                self.fly_home = false;
                self.pending_smooth_zoom += 1.0;
            }
            HudButton::ZoomOut => {
                self.fly_home = false;
                self.pending_smooth_zoom -= 1.0;
            }
            HudButton::Home => self.trigger_home(),
            HudButton::PointerMode => self.toggle_pointer_profile(),
        }
        true
    }

    /// Checks whether a primary press at `position` is a double-click / double-tap, and if so,
    /// queues a smooth `+1.0` zoom (`-1.0` when `Shift` or `Alt` is held).
    pub fn check_double_click(&mut self, now: Instant, position: Vector2<f64>) -> bool {
        const DOUBLE_CLICK_MAX_DELAY: Duration = Duration::from_millis(320);
        const DOUBLE_CLICK_MAX_DIST: f64 = 24.0;

        if let Some((last_time, last_pos)) = self.last_primary_click {
            if now.saturating_duration_since(last_time) <= DOUBLE_CLICK_MAX_DELAY
                && (position - last_pos).magnitude() <= DOUBLE_CLICK_MAX_DIST
            {
                self.last_primary_click = None;
                self.fly_home = false;
                let delta = if self.shift_held || self.alt_held {
                    -1.0
                } else {
                    1.0
                };
                self.pending_smooth_zoom += delta;
                return true;
            }
        }
        self.last_primary_click = Some((now, position));
        false
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

    /// Advances HUD compass drag, smooth settle animations, fullscreen/RTS edge-scrolling,
    /// and syncs `NavigationHudState` into `world.resources` for GPU rendering.
    pub fn update_frame(&mut self, map_context: &mut MapContext, dt: Duration) {
        self.ensure_initialized(map_context);

        // Update hovered button using actual viewport dimensions and edge insets.
        if let Some(cursor) = self.cursor_position {
            if self.show_hud {
                self.hovered_button = NavigationHudState::hit_test(
                    cursor,
                    map_context.view_state.width(),
                    map_context.view_state.height(),
                    map_context.view_state.edge_insets(),
                );
            } else {
                self.hovered_button = None;
            }
        }

        // 1. Handle active Compass drag (horizontal = bearing, vertical = manual pitch).
        if self.pressed_button == Some(HudButton::Compass) && self.compass_dragged {
            if let (Some(cursor), Some(last)) = (self.cursor_position, self.compass_drag_last) {
                let delta = cursor - last;
                self.compass_drag_last = Some(cursor);
                if delta.magnitude2() > f64::EPSILON {
                    let max_pitch = map_context.view_state.camera().max_pitch().0.to_degrees();
                    if !self.north_locked && delta.x.abs() > 0.0 {
                        let current_bearing = map_context.view_state.camera().get_bearing();
                        let next_bearing = current_bearing - Rad::from(Deg(delta.x * 0.65));
                        map_context
                            .view_state
                            .camera_mut()
                            .set_bearing(next_bearing);
                        self.on_manual_bearing();
                    }
                    if delta.y.abs() > 0.0 {
                        let current_pitch =
                            map_context.view_state.camera().get_pitch().0.to_degrees();
                        let next_pitch = (current_pitch - delta.y * 0.5).clamp(0.0, max_pitch);
                        self.on_manual_tilt();
                        set_pitch_keeping_center(&mut map_context.view_state, Deg(next_pitch));
                    }
                }
            }
        }

        // 2. Apply pending trackpad two-finger rotation around the cursor (or view center).
        if self.pending_trackpad_rotation.0.abs() > 1e-6 {
            let delta = self.pending_trackpad_rotation;
            self.pending_trackpad_rotation = Rad(0.0);
            if !self.north_locked {
                rotate_bearing_around_cursor(
                    &map_context.style,
                    &mut map_context.view_state,
                    self.cursor_position,
                    delta,
                );
            }
        }

        // 3. Enforce North-Lock if active.
        if self.north_locked {
            let current_bearing = map_context.view_state.camera().get_bearing().0;
            if normalize_angle_signed(current_bearing).abs() > 1e-4 {
                map_context.view_state.camera_mut().set_bearing(Rad(0.0));
            }
        }

        // 4. Advance smooth settle animations (`fly_home`, `target_bearing_rad`, `target_pitch_deg`).
        let alpha = (1.0 - (-dt.as_secs_f64() * 14.0).exp()).clamp(0.05, 1.0);

        if self.fly_home {
            if let Some(home) = self.home {
                let current_zoom = map_context.view_state.zoom().value();
                let next_zoom = current_zoom + (home.zoom - current_zoom) * alpha;
                let center_px = center_pixel(&map_context.view_state);
                let screen = Vector2::new(center_px.x, center_px.y);
                let _ = zoom_globe_around_pixel_with_pitch(
                    &map_context.style,
                    &mut map_context.view_state,
                    screen,
                    Zoom::new(next_zoom),
                    None,
                );
                map_context.view_state.zoom_to(Zoom::new(next_zoom));

                let world_size = TILE_SIZE * 2.0_f64.powf(map_context.view_state.zoom().value());
                let current_pos = map_context.view_state.camera().position();
                let target_pos = home.normalized_center * world_size;
                let next_pos = current_pos + (target_pos - current_pos) * alpha;
                map_context.view_state.camera_mut().move_to(next_pos);

                if (home.zoom - next_zoom).abs() < 0.01 && (target_pos - next_pos).magnitude() < 0.5
                {
                    map_context.view_state.zoom_to(Zoom::new(home.zoom));
                    let final_world_size = TILE_SIZE * 2.0_f64.powf(home.zoom);
                    map_context
                        .view_state
                        .camera_mut()
                        .move_to(home.normalized_center * final_world_size);
                    self.fly_home = false;
                }
            } else {
                self.fly_home = false;
            }
        }

        if let Some(target_bearing) = self.target_bearing_rad {
            let current = map_context.view_state.camera().get_bearing().0;
            let diff = normalize_angle_signed(target_bearing - current);
            if diff.abs() <= 0.0015 {
                map_context
                    .view_state
                    .camera_mut()
                    .set_bearing(Rad(target_bearing));
                self.target_bearing_rad = None;
            } else {
                map_context
                    .view_state
                    .camera_mut()
                    .set_bearing(Rad(current + diff * alpha));
            }
        }

        if let Some(target_pitch) = self.target_pitch_deg {
            let max_pitch = map_context.view_state.camera().max_pitch().0.to_degrees();
            let clamped_target = target_pitch.clamp(0.0, max_pitch);
            let current = map_context.view_state.camera().get_pitch().0.to_degrees();
            let diff = clamped_target - current;
            if diff.abs() <= 0.08 {
                set_pitch_keeping_center(&mut map_context.view_state, Deg(clamped_target));
                self.target_pitch_deg = None;
            } else {
                set_pitch_keeping_center(&mut map_context.view_state, Deg(current + diff * alpha));
            }
        } else if self.three_dimensional && self.auto_tilt && !self.fly_home {
            // Smoothly recover pitch toward the Auto-Tilt curve whenever terrain clearance
            // allows (e.g. when panning off a steep ridge back over a valley).
            let max_pitch = map_context.view_state.camera().max_pitch().0.to_degrees();
            let zoom = map_context.view_state.zoom().value();
            let desired = self.pitch_for_zoom(zoom).clamp(0.0, max_pitch);
            let current = map_context.view_state.camera().get_pitch().0.to_degrees();
            let diff = desired - current;
            if diff.abs() > 0.12 {
                set_pitch_keeping_center(&mut map_context.view_state, Deg(current + diff * alpha));
            }
        }

        // 4. Screen-edge scrolling in RTS mode (or when edge_scroll_override is active).
        if (self.pointer_profile == PointerProfile::Rts || self.edge_scroll_override)
            && self.pressed_button.is_none()
            && self.selection_drag.is_none()
        {
            if let Some(cursor) = self.cursor_position {
                let w = map_context.view_state.width();
                let h = map_context.view_state.height();
                if w > 100.0
                    && h > 100.0
                    && cursor.x >= 0.0
                    && cursor.x <= w
                    && cursor.y >= 0.0
                    && cursor.y <= h
                    && !self.is_over_hud(&map_context.view_state, cursor)
                {
                    const EDGE_ZONE_PX: f64 = 24.0;
                    let max_edge_speed = self.edge_scroll_speed_px_per_sec.max(0.0);
                    let mut vx = 0.0;
                    let mut vy = 0.0;
                    if cursor.x < EDGE_ZONE_PX {
                        let t = ((EDGE_ZONE_PX - cursor.x) / EDGE_ZONE_PX).clamp(0.0, 1.0);
                        vx = -t * t;
                    } else if cursor.x > w - EDGE_ZONE_PX {
                        let t = ((cursor.x - (w - EDGE_ZONE_PX)) / EDGE_ZONE_PX).clamp(0.0, 1.0);
                        vx = t * t;
                    }
                    if cursor.y < EDGE_ZONE_PX {
                        let t = ((EDGE_ZONE_PX - cursor.y) / EDGE_ZONE_PX).clamp(0.0, 1.0);
                        vy = -t * t;
                    } else if cursor.y > h - EDGE_ZONE_PX {
                        let t = ((cursor.y - (h - EDGE_ZONE_PX)) / EDGE_ZONE_PX).clamp(0.0, 1.0);
                        vy = t * t;
                    }
                    if vx != 0.0 || vy != 0.0 {
                        let speed_boost = if self.shift_held { 1.8 } else { 1.0 };
                        let step = Vector2::new(vx, vy)
                            * (max_edge_speed * speed_boost * dt.as_secs_f64());
                        let center = super::projection::center_pixel(&map_context.view_state);
                        let plane = map_context.view_state.center_elevation();
                        if !pan_globe_by_pixels(
                            &map_context.style,
                            &mut map_context.view_state,
                            center,
                            -step,
                        ) {
                            pan_plane_by_pixels(&mut map_context.view_state, center, -step, plane);
                        }
                    }
                }
            }
        }

        // 5. Sync NavigationHudState into world.resources for the GPU HUD overlay pass.
        let hud_state = map_context
            .world
            .resources
            .get_or_init_mut::<NavigationHudState>();
        hud_state.visible = self.show_hud;
        hud_state.three_dimensional = self.three_dimensional;
        hud_state.auto_tilt = self.auto_tilt;
        hud_state.north_locked = self.north_locked;
        hud_state.rts_mode = self.pointer_profile == PointerProfile::Rts;
        hud_state.hovered = self.hovered_button;
        hud_state.pressed = self.pressed_button;
        hud_state.selection_box = self.selection_drag.and_then(|(start, end)| {
            ((end - start).magnitude() >= 3.0).then_some([
                start.x.min(end.x) as f32,
                start.y.min(end.y) as f32,
                start.x.max(end.x) as f32,
                start.y.max(end.y) as f32,
            ])
        });
    }
}

/// Sets the camera pitch while keeping the ground point at the viewport center pinned.
pub fn set_pitch_keeping_center(view_state: &mut ViewState, pitch: impl Into<Rad<f64>>) {
    let pitch: Rad<f64> = pitch.into();
    if view_state.navigation_mode() == maplibre::render::view_state::NavigationMode::FreeGlobe {
        if let Some(pose) = view_state.globe_pose() {
            let current = view_state.camera().get_pitch();
            let _ = view_state.orbit_globe_pose(pose, Rad(0.0), pitch - current);
            return;
        }
    }
    view_state.camera_mut().set_pitch(pitch);
}

/// Rotates camera bearing by `delta` around the ground point under `cursor` (falling back to
/// the viewport center when the cursor is off the surface).
fn rotate_bearing_around_cursor(
    style: &maplibre::style::Style,
    view_state: &mut ViewState,
    cursor: Option<Vector2<f64>>,
    delta: Rad<f64>,
) {
    if view_state.navigation_mode() == maplibre::render::view_state::NavigationMode::FreeGlobe {
        if let Some(pose) = view_state.globe_pose() {
            let _ = view_state.orbit_globe_pose(pose, delta, Rad(0.0));
        }
        return;
    }
    let center = center_pixel(view_state);
    let pivot = cursor.map_or(center, |c| Point2::new(c.x, c.y));

    if let Some(before_globe) = super::projection::active_globe_camera(style, view_state) {
        let anchor = before_globe.screen_point_to_location(pivot);
        let current = view_state.camera().get_bearing();
        view_state.camera_mut().set_bearing(current + delta);
        if let (Some(anchor), Some(after_globe)) = (
            anchor,
            super::projection::active_globe_camera(style, view_state),
        ) {
            if let Some(under) = after_globe.screen_point_to_location(pivot) {
                let update = maplibre::projection::globe::interaction::pan_center_to_anchor(
                    after_globe.center(),
                    after_globe.bearing_degrees(),
                    anchor,
                    under,
                );
                let zoom = view_state.zoom();
                let world_center = maplibre::coords::WorldCoords::from_lat_lon(update.center, zoom);
                view_state
                    .camera_mut()
                    .move_to(Point2::new(world_center.x, world_center.y));
            }
        }
        return;
    }

    let plane = view_state.center_elevation();
    let before = view_state
        .inverted_view_projection()
        .ok()
        .and_then(|inv| view_state.window_to_world_at_elevation(&pivot.to_vec(), &inv, plane));
    let current = view_state.camera().get_bearing();
    view_state.camera_mut().set_bearing(current + delta);
    if let Some(before) = before {
        set_location_at_pixel(view_state, before, plane, pivot);
    }
}

/// Base Auto-Tilt curve in degrees as a function of continuous zoom.
///
/// Smoothly transitions from `0°` (overhead) at `zoom <= 7.5` through regional (`25°` at `z9.0`),
/// mountain range (`50°` at `z10.5`), alpine overview (`68°` at `z12.0`, matching the 3D terrain
/// demo), valley (`73°` at `z14.0`), up to `76°` at `zoom >= 16.0`.
fn base_auto_tilt_degrees(zoom: f64) -> f64 {
    const STOPS: &[(f64, f64)] = &[
        (7.5, 0.0),
        (9.0, 25.0),
        (10.5, 50.0),
        (12.0, 68.0),
        (14.0, 73.0),
        (16.0, 76.0),
    ];
    if zoom <= STOPS[0].0 {
        return STOPS[0].1;
    }
    for window in STOPS.windows(2) {
        let (z0, p0) = window[0];
        let (z1, p1) = window[1];
        if zoom <= z1 {
            let t = ((zoom - z0) / (z1 - z0)).clamp(0.0, 1.0);
            let s = t * t * (3.0 - 2.0 * t);
            return p0 + (p1 - p0) * s;
        }
    }
    STOPS[STOPS.len() - 1].1
}

/// Normalizes an angle in radians to `[-PI, PI]`.
pub fn normalize_angle_signed(angle: f64) -> f64 {
    let two_pi = std::f64::consts::TAU;
    let mut a = (angle + std::f64::consts::PI).rem_euclid(two_pi) - std::f64::consts::PI;
    if a <= -std::f64::consts::PI {
        a += two_pi;
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_tilt_curve_is_zero_at_globe_zooms_and_monotonic() {
        let state = NavigationState {
            three_dimensional: true,
            auto_tilt: true,
            ..Default::default()
        };
        assert_eq!(state.pitch_for_zoom(2.0), 0.0);
        assert_eq!(state.pitch_for_zoom(7.5), 0.0);
        let z9 = state.pitch_for_zoom(9.0);
        let z12 = state.pitch_for_zoom(12.0);
        let z14 = state.pitch_for_zoom(14.0);
        let z17 = state.pitch_for_zoom(17.0);
        assert!((z9 - 25.0).abs() < 1e-6);
        assert!((z12 - 68.0).abs() < 1e-6);
        assert!((z14 - 73.0).abs() < 1e-6);
        assert!((z17 - 76.0).abs() < 1e-6);
    }

    #[test]
    fn trackpad_rotation_accumulates_when_unlocked_and_ignores_when_north_locked() {
        let mut state = NavigationState::default();
        state.process_trackpad_rotate(15.0);
        assert!(state.pending_trackpad_rotation.0 > 0.0);

        state.pending_trackpad_rotation = Rad(0.0);
        state.north_locked = true;
        state.process_trackpad_rotate(15.0);
        assert_eq!(state.pending_trackpad_rotation.0, 0.0);
    }

    #[test]
    fn cycle_3d_mode_transitions_cleanly_across_all_three_states() {
        let mut state = NavigationState {
            three_dimensional: true,
            auto_tilt: true,
            ..Default::default()
        };
        // 3D·AUTO -> 2D
        state.cycle_3d_mode(13.0);
        assert!(!state.three_dimensional);
        assert!(!state.auto_tilt);
        assert_eq!(state.target_pitch_deg, Some(0.0));

        // 2D -> 3D·AUTO
        state.cycle_3d_mode(13.0);
        assert!(state.three_dimensional);
        assert!(state.auto_tilt);
        assert!(state.target_pitch_deg.unwrap_or(0.0) > 50.0);

        // Manual tilt -> 3D·MAN -> cycle back to 3D·AUTO
        state.on_manual_tilt();
        assert!(state.three_dimensional);
        assert!(!state.auto_tilt);
        state.cycle_3d_mode(13.0);
        assert!(state.three_dimensional);
        assert!(state.auto_tilt);
    }

    #[test]
    fn set_pitch_keeping_center_clamps_to_max_pitch_in_degrees_not_radians() {
        use maplibre::{
            coords::{LatLon, WorldCoords},
            window::PhysicalSize,
        };
        let zoom = Zoom::new(12.0);
        let mut view = ViewState::new(
            PhysicalSize::new(800, 600).expect("valid viewport"),
            WorldCoords::from_lat_lon(LatLon::new(47.27574, 11.39085), zoom),
            zoom,
            Deg(0.0),
            Deg(45.0),
        );
        view.camera_mut().set_max_pitch(Deg(85.0));

        let nav = NavigationState {
            three_dimensional: true,
            auto_tilt: true,
            ..Default::default()
        };
        let max_pitch_deg = view.camera().max_pitch().0.to_degrees();
        let desired_deg = nav.pitch_for_zoom(12.0).clamp(0.0, max_pitch_deg);
        set_pitch_keeping_center(&mut view, Deg(desired_deg));

        let actual_deg = view.camera().get_pitch().0.to_degrees();
        assert!(
            (actual_deg - 68.0).abs() < 1e-4,
            "expected 68.0 deg pitch at z12, got {actual_deg}"
        );
    }
}
