//! Keyboard, viewport edge-inset debug, and rendered-geometry query input handlers.

use std::time::Duration;

use cgmath::{Deg, InnerSpace, Rad, Vector2, Zero};

use super::{
    events::{ButtonState, NavKey, PointerButton},
    navigation_state::{set_pitch_keeping_center, NavigationState},
    projection::{
        center_pixel, globe_world_at_screen, pan_globe_by_pixels, pan_plane_by_pixels, uses_globe,
    },
    UpdateState,
};
use crate::{
    context::MapContext, coords::WorldCoords, io::geometry_index::IndexedGeometry,
    render::tile_view_pattern::DEFAULT_TILE_SIZE,
};

#[derive(Default)]
pub(crate) struct DebugHandler {
    top_delta: f64,
    bottom_delta: f64,
    left_delta: f64,
    right_delta: f64,
}

impl UpdateState for DebugHandler {
    fn update_state(&mut self, MapContext { view_state, .. }: &mut MapContext, dt: Duration) {
        self.update_parts(view_state, dt);
    }
}

impl DebugHandler {
    pub(crate) fn update_parts(
        &mut self,
        view_state: &mut crate::render::view_state::ViewState,
        dt: Duration,
    ) {
        let dt = (dt.as_secs_f64() * 10.0).min(1.0);

        let top_delta = self.top_delta * dt;
        let bottom_delta = self.bottom_delta * dt;
        let left_delta = self.left_delta * dt;
        let right_delta = self.right_delta * dt;

        let mut edge_insets = view_state.edge_insets();
        edge_insets.top += top_delta;
        edge_insets.bottom += bottom_delta;
        edge_insets.left += left_delta;
        edge_insets.right += right_delta;
        view_state.set_edge_insets(edge_insets);
        self.top_delta -= top_delta;
        self.bottom_delta -= bottom_delta;
        self.right_delta -= right_delta;
        self.left_delta -= left_delta;
    }

    pub(crate) fn process_key(&mut self, key: NavKey, state: ButtonState) -> bool {
        let amount = if state == ButtonState::Pressed {
            100.0
        } else {
            0.0
        };

        match key {
            NavKey::DebugInsetLeft => {
                self.left_delta += amount;
                true
            }
            NavKey::DebugInsetTop => {
                self.top_delta += amount;
                true
            }
            NavKey::DebugInsetBottom => {
                self.bottom_delta += amount;
                true
            }
            NavKey::DebugInsetRight => {
                self.right_delta += amount;
                true
            }
            _ => false,
        }
    }
}

#[derive(Default)]
pub(crate) struct QueryHandler {
    window_position: Option<Vector2<f64>>,
    clicking: bool,
}

impl QueryHandler {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn process_touch_start(&mut self) -> bool {
        self.clicking = true;
        true
    }

    pub(crate) fn process_touch_end(&mut self) -> bool {
        self.clicking = false;
        true
    }

    pub(crate) fn process_window_position(
        &mut self,
        window_position: &Vector2<f64>,
        _touch: bool,
    ) -> bool {
        self.window_position = Some(*window_position);
        true
    }

    pub(crate) fn process_pointer_button(
        &mut self,
        button: PointerButton,
        state: ButtonState,
    ) -> bool {
        if button != PointerButton::Primary {
            return false;
        }
        self.clicking = state == ButtonState::Pressed;
        true
    }
}

impl UpdateState for QueryHandler {
    fn update_state(
        &mut self,
        MapContext {
            style,
            view_state,
            world,
            ..
        }: &mut MapContext,
        _dt: Duration,
    ) {
        self.update_parts(style, view_state, world);
    }
}

impl QueryHandler {
    pub(crate) fn update_parts(
        &mut self,
        style: &crate::style::Style,
        view_state: &mut crate::render::view_state::ViewState,
        world: &crate::tcs::world::World,
    ) {
        if self.clicking {
            if let Some(window_position) = self.window_position {
                let Ok(inverted_view_proj) = view_state.inverted_view_projection() else {
                    return;
                };

                let z = view_state.zoom().zoom_level(DEFAULT_TILE_SIZE);
                let zoom = view_state.zoom();

                let coordinates = if uses_globe(style, view_state) {
                    globe_world_at_screen(style, view_state, window_position)
                        .map(|coordinates| Vector2::new(coordinates.x, coordinates.y))
                } else {
                    view_state.window_to_world_at_ground(
                        &window_position,
                        &inverted_view_proj,
                        false,
                    )
                };
                if let Some(coordinates) = coordinates {
                    if let Some(geometries) = world
                        .tiles
                        .as_ref()
                        .query_point(
                            &WorldCoords {
                                x: coordinates.x,
                                y: coordinates.y,
                            },
                            z,
                            zoom,
                        )
                        .map(|geometries| {
                            geometries
                                .iter()
                                .cloned()
                                .cloned()
                                .collect::<Vec<IndexedGeometry<f64>>>()
                        })
                    {
                        tracing::info!(
                            "Clicked on geometry: {:?}",
                            geometries
                                .iter()
                                .map(|geometry| &geometry.properties)
                                .collect::<Vec<_>>()
                        );
                    } else {
                        tracing::info!("No geometry found.");
                    }
                }
            }
            self.clicking = false;
        }
    }
}

pub(crate) struct ShiftHandler {
    camera_translate: Vector2<f64>,
    held_up: bool,
    held_down: bool,
    held_left: bool,
    held_right: bool,
    held_orbit_ccw: bool,
    held_orbit_cw: bool,
    speed: f64,
    sensitivity: f64,
}

impl UpdateState for ShiftHandler {
    fn update_state(&mut self, map_context: &mut MapContext, dt: Duration) {
        let mut dummy_nav = NavigationState::default();
        self.update_state_with_nav(map_context, dt, &mut dummy_nav);
    }
}

impl ShiftHandler {
    pub(crate) fn new(speed: f64, sensitivity: f64) -> Self {
        Self {
            camera_translate: Vector2::zero(),
            held_up: false,
            held_down: false,
            held_left: false,
            held_right: false,
            held_orbit_ccw: false,
            held_orbit_cw: false,
            speed,
            sensitivity,
        }
    }

    /// Whether any directional or orbit key is currently held down or decaying.
    pub(crate) fn is_active(&self) -> bool {
        self.held_up
            || self.held_down
            || self.held_left
            || self.held_right
            || self.held_orbit_ccw
            || self.held_orbit_cw
            || self.camera_translate.magnitude2() > 0.01
    }
}

fn pan_by(
    style: &crate::style::Style,
    view_state: &mut crate::render::view_state::ViewState,
    delta: Vector2<f64>,
) {
    let center = center_pixel(view_state);
    let plane = view_state.center_elevation();
    if !pan_globe_by_pixels(style, view_state, center, delta) {
        pan_plane_by_pixels(view_state, center, delta, plane);
    }
}

impl ShiftHandler {
    pub(crate) fn update_state_with_nav(
        &mut self,
        state: &mut MapContext,
        dt: Duration,
        nav: &mut NavigationState,
    ) {
        self.update_parts_with_nav(&state.style, &mut state.view_state, dt, nav);
    }

    fn held_direction(&self) -> Vector2<f64> {
        let mut dir = Vector2::zero();
        if self.held_up {
            dir.y -= 1.0;
        }
        if self.held_down {
            dir.y += 1.0;
        }
        if self.held_left {
            dir.x -= 1.0;
        }
        if self.held_right {
            dir.x += 1.0;
        }
        dir
    }

    fn apply_alt_orbit_or_tilt(
        &mut self,
        view_state: &mut crate::render::view_state::ViewState,
        nav: &mut NavigationState,
        dir: Vector2<f64>,
        boost: f64,
        dt_secs: f64,
    ) {
        if dir.x != 0.0 && !nav.north_locked {
            nav.on_manual_bearing();
            let current = view_state.camera().get_bearing();
            let delta = Rad::from(Deg(dir.x * 95.0 * boost * dt_secs));
            view_state.camera_mut().set_bearing(current + delta);
        }
        if dir.y != 0.0 && nav.three_dimensional {
            nav.on_manual_tilt();
            let max_pitch: f64 = view_state.camera().max_pitch().0.to_degrees();
            let current: f64 = view_state.camera().get_pitch().0.to_degrees();
            let next = (current - dir.y * 55.0 * boost * dt_secs).clamp(0.0, max_pitch);
            set_pitch_keeping_center(view_state, Deg(next));
        }
        self.camera_translate = Vector2::zero();
    }

    pub(crate) fn update_parts_with_nav(
        &mut self,
        style: &crate::style::Style,
        view_state: &mut crate::render::view_state::ViewState,
        dt: Duration,
        nav: &mut NavigationState,
    ) {
        let dt_secs = dt.as_secs_f64();
        let boost = if nav.shift_held { 1.85 } else { 1.0 };
        let dir = self.held_direction();

        if nav.alt_held && (dir.x != 0.0 || dir.y != 0.0) {
            self.apply_alt_orbit_or_tilt(view_state, nav, dir, boost, dt_secs);
        } else if dir.magnitude2() > 0.0 {
            nav.on_manual_pan_or_zoom();
            let delta = dir.normalize() * (6.2 * self.sensitivity * boost * dt_secs);
            pan_by(style, view_state, -delta);
            self.camera_translate = Vector2::zero();
        } else {
            let decay = (dt_secs / self.speed).min(1.0);
            let delta = self.camera_translate * decay;
            self.camera_translate -= delta;
            if self.camera_translate.magnitude2() <= 0.01 {
                self.camera_translate = Vector2::zero();
            }
            if !delta.is_zero() {
                pan_by(style, view_state, -delta);
            }
        }

        // Continuous Q / E bearing rotation.
        let orbit_dir = f64::from(i8::from(self.held_orbit_cw) - i8::from(self.held_orbit_ccw));
        if orbit_dir != 0.0 && !nav.north_locked {
            nav.on_manual_bearing();
            let current = view_state.camera().get_bearing();
            let delta = Rad::from(Deg(orbit_dir * 95.0 * boost * dt_secs));
            view_state.camera_mut().set_bearing(current + delta);
        }
    }

    pub(crate) fn process_key(&mut self, key: NavKey, state: ButtonState) -> bool {
        let pressed = state == ButtonState::Pressed;
        let amount = if pressed {
            10.0 * self.sensitivity
        } else {
            0.0
        };
        match key {
            NavKey::Up => {
                self.held_up = pressed;
                self.camera_translate.y -= amount;
                true
            }
            NavKey::Down => {
                self.held_down = pressed;
                self.camera_translate.y += amount;
                true
            }
            NavKey::Left => {
                self.held_left = pressed;
                self.camera_translate.x -= amount;
                true
            }
            NavKey::Right => {
                self.held_right = pressed;
                self.camera_translate.x += amount;
                true
            }
            NavKey::OrbitCcw => {
                self.held_orbit_ccw = pressed;
                true
            }
            NavKey::OrbitCw => {
                self.held_orbit_cw = pressed;
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_idle_gap_clamps_decay_fraction_without_overshoot_or_sign_flip() {
        let mut handler = ShiftHandler::new(0.2, 100.0);
        assert!(handler.process_key(NavKey::Right, ButtonState::Pressed));
        let initial_x = handler.camera_translate.x;
        assert!(initial_x > 0.0);

        let factor = (Duration::from_secs(60).as_secs_f64() / handler.speed).min(1.0);
        let delta = handler.camera_translate * factor;
        handler.camera_translate -= delta;
        assert_eq!(delta.x, initial_x);
        assert_eq!(handler.camera_translate.x, 0.0);
    }
}
