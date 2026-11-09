//! Handles the user input which is dispatched by the main event loop.

use std::time::Duration;

use cgmath::{InnerSpace, Rad, Vector2};
use instant::Instant;
use maplibre::{
    context::MapContext,
    terrain::interaction::{clamp_pitch_above_terrain_keeping_zoom, keep_camera_above_terrain},
};
use winit::{
    event::{DeviceEvent, ElementState, KeyEvent, MouseButton, TouchPhase, WindowEvent},
    keyboard::{Key, NamedKey},
};

use crate::input::{
    camera_handler::CameraHandler,
    debug_handler::DebugHandler,
    navigation_state::{NavigationState, PointerProfile},
    pan_handler::PanHandler,
    pinch_handler::PinchHandler,
    query_handler::QueryHandler,
    shift_handler::ShiftHandler,
    zoom_handler::ZoomHandler,
};

mod camera_handler;
mod debug_handler;
mod inertia;
pub mod navigation_state;
mod pan_handler;
mod pinch_handler;
pub mod projection;
mod query_handler;
mod shift_handler;
mod zoom_handler;

/// Accumulates window gestures and applies camera changes when the next frame updates.
pub struct InputController {
    pinch_handler: PinchHandler,
    pan_handler: PanHandler,
    zoom_handler: ZoomHandler,
    camera_handler: CameraHandler,
    shift_handler: ShiftHandler,
    query_handler: QueryHandler,
    debug_handler: DebugHandler,
    navigation_state: NavigationState,
    /// Touch ID currently interacting with the floating HUD capsule, if any.
    hud_touch_id: Option<u64>,
    /// Press position of the right mouse button in RTS mode before the drag threshold is met.
    rts_right_press_origin: Option<Vector2<f64>>,
    /// Whether the current right-mouse drag in RTS mode is actively panning the map.
    rts_right_panning: bool,
    /// Latest known viewport width in logical pixels.
    last_known_width: f64,
    /// Latest known viewport height in logical pixels.
    last_known_height: f64,
    /// Latest known continuous zoom level.
    last_known_zoom: f64,
    /// Latest known camera bearing.
    last_known_bearing: Rad<f64>,
    /// Exponentially smoothed camera pan velocity in world pixels per second for predictive tile prefetching.
    smoothed_pan_velocity: Vector2<f64>,
    /// Whether the last update moved the view or updated the HUD, so the next frame may move it further.
    moved_view: bool,
}

impl InputController {
    /// Sets movement speed, camera/keyboard sensitivity and scroll zoom sensitivity, a factor
    /// on GL JS's scroll zoom rates.
    pub fn new(speed: f64, sensitivity: f64, zoom_sensitivity: f64) -> Self {
        Self {
            pinch_handler: PinchHandler::default(),
            pan_handler: PanHandler::default(),
            zoom_handler: ZoomHandler::new(zoom_sensitivity),
            camera_handler: CameraHandler::new(sensitivity),
            shift_handler: ShiftHandler::new(speed, sensitivity),
            query_handler: QueryHandler::new(),
            debug_handler: DebugHandler::default(),
            navigation_state: NavigationState::default(),
            hud_touch_id: None,
            rts_right_press_origin: None,
            rts_right_panning: false,
            last_known_width: 800.0,
            last_known_height: 800.0,
            last_known_zoom: 0.0,
            last_known_bearing: Rad(0.0),
            smoothed_pan_velocity: Vector2::new(0.0, 0.0),
            moved_view: false,
        }
    }

    /// Whether the last frame's update moved the view: a drag's inertia, a held key or a zoom
    /// animation carries on into the next frame, which therefore has to be drawn.
    pub fn moved_view(&self) -> bool {
        self.moved_view
    }

    /// Shortest time until a pending input timer (such as a held wheel step, smooth animation,
    /// held movement key, or gesture-end release) needs a frame, even while the view itself is still.
    pub fn next_wait(&self, now: instant::Instant) -> Option<Duration> {
        if self.navigation_state.is_animating() || self.shift_handler.is_active() {
            return Some(Duration::ZERO);
        }
        self.zoom_handler.next_wait(now)
    }

    /// Leaves raw device events unconsumed; navigation is driven by window events.
    pub fn device_input(&mut self, _event: &DeviceEvent) -> bool {
        false
    }

    /// Process the given winit `[winit::event::WindowEvent]`.
    /// Returns true if the event has been processed and false otherwise.
    pub fn window_input(&mut self, event: &WindowEvent, scale_factor: f64) -> bool {
        match event {
            WindowEvent::ModifiersChanged(modifiers) => {
                let state = modifiers.state();
                self.navigation_state.shift_held = state.shift_key();
                self.navigation_state.alt_held = state.alt_key();
                self.navigation_state.ctrl_held = state.control_key() || state.super_key();
                false
            }
            WindowEvent::CursorMoved { position, .. } => {
                let position: (f64, f64) = position.to_owned().into();
                let position = Vector2::from(position) / scale_factor;

                let consumed_by_hud = self.navigation_state.process_cursor_moved(
                    position,
                    self.last_known_width,
                    self.last_known_height,
                );

                // In RTS mode, a right-button drag starts panning once it moves past 4px.
                if let Some(origin) = self.rts_right_press_origin {
                    if !self.rts_right_panning && (position - origin).magnitude() >= 4.0 {
                        self.rts_right_panning = true;
                        self.navigation_state.on_manual_pan_or_zoom();
                        self.pan_handler.begin_pan(Some(origin));
                    }
                }

                if !consumed_by_hud {
                    self.pan_handler.process_window_position(&position, false);
                    self.query_handler.process_window_position(&position, false);
                    self.zoom_handler.process_window_position(&position, false);
                    self.camera_handler
                        .process_window_position(&position, false);
                }
                true
            }
            WindowEvent::KeyboardInput {
                event: KeyEvent {
                    state, logical_key, ..
                },
                ..
            } => {
                if *state == ElementState::Pressed {
                    match logical_key.as_ref() {
                        Key::Named(NamedKey::Tab) => {
                            self.navigation_state.toggle_pointer_profile();
                            return true;
                        }
                        Key::Character("t") | Key::Character("T") => {
                            self.navigation_state.cycle_3d_mode(self.last_known_zoom);
                            return true;
                        }
                        Key::Character("n") | Key::Character("N") => {
                            self.navigation_state
                                .trigger_compass(self.last_known_bearing);
                            return true;
                        }
                        Key::Character("r") | Key::Character("R") | Key::Named(NamedKey::Home) => {
                            self.navigation_state.trigger_home();
                            return true;
                        }
                        Key::Character("h") | Key::Character("H") => {
                            self.navigation_state.show_hud = !self.navigation_state.show_hud;
                            return true;
                        }
                        _ => {}
                    }
                }
                self.shift_handler.process_key_press(logical_key, *state)
                    || self.debug_handler.process_key_press(logical_key, *state)
                    || self.zoom_handler.process_key_press(logical_key, *state)
            }
            WindowEvent::Touch(touch) => {
                let raw: (f64, f64) = touch.location.to_owned().into();
                let position = Vector2::from(raw) / scale_factor;
                match touch.phase {
                    TouchPhase::Started => {
                        if self.pinch_handler.touch_count() == 0
                            && self.navigation_state.process_hud_press(
                                position,
                                self.last_known_width,
                                self.last_known_height,
                            )
                        {
                            self.hud_touch_id = Some(touch.id);
                            return true;
                        }
                        if self.pinch_handler.touch_count() == 0
                            && self
                                .navigation_state
                                .check_double_click(Instant::now(), position)
                        {
                            if let Some(delta) = self.navigation_state.take_pending_smooth_zoom() {
                                self.zoom_handler.add_smooth_zoom(delta, Some(position));
                            }
                            return true;
                        }
                        self.navigation_state.on_manual_pan_or_zoom();
                        self.pinch_handler.on_touch_start(
                            touch.id,
                            position,
                            &mut self.pan_handler,
                        );
                        self.query_handler.process_touch_start();
                        true
                    }
                    TouchPhase::Moved => {
                        if self.hud_touch_id == Some(touch.id) {
                            self.navigation_state.process_cursor_moved(
                                position,
                                self.last_known_width,
                                self.last_known_height,
                            );
                            return true;
                        }
                        self.pinch_handler.on_touch_move(
                            touch.id,
                            position,
                            &mut self.pan_handler,
                            &mut self.zoom_handler,
                            &mut self.navigation_state,
                        );
                        self.query_handler.process_window_position(&position, true);
                        self.camera_handler.process_window_position(&position, true);
                        true
                    }
                    TouchPhase::Ended | TouchPhase::Cancelled => {
                        if self.hud_touch_id == Some(touch.id) {
                            self.hud_touch_id = None;
                            self.navigation_state
                                .process_hud_release(self.last_known_zoom, self.last_known_bearing);
                            if let Some(delta) = self.navigation_state.take_pending_smooth_zoom() {
                                let center = Vector2::new(
                                    self.last_known_width * 0.5,
                                    self.last_known_height * 0.5,
                                );
                                self.zoom_handler.add_smooth_zoom(delta, Some(center));
                            }
                            return true;
                        }
                        self.pinch_handler
                            .on_touch_end(touch.id, &mut self.pan_handler);
                        self.query_handler.process_touch_end();
                        true
                    }
                }
            }
            WindowEvent::PinchGesture { delta, .. } => {
                self.navigation_state.on_manual_pan_or_zoom();
                self.zoom_handler.process_pinch(*delta);
                true
            }
            WindowEvent::RotationGesture { delta, .. } => {
                self.navigation_state
                    .process_trackpad_rotate(f64::from(*delta));
                true
            }
            WindowEvent::DoubleTapGesture { .. } => {
                self.navigation_state.on_manual_pan_or_zoom();
                let delta = if self.navigation_state.shift_held || self.navigation_state.alt_held {
                    -1.0
                } else {
                    1.0
                };
                self.zoom_handler
                    .add_smooth_zoom(delta, self.navigation_state.cursor_position);
                true
            }
            WindowEvent::MouseWheel { delta, .. } => {
                self.navigation_state.on_manual_pan_or_zoom();
                if let winit::event::MouseScrollDelta::PixelDelta(pos) = delta {
                    let dx = pos.x / scale_factor;
                    let dy = pos.y / scale_factor;
                    let is_discrete_wheel = dx == 0.0 && dy != 0.0 && {
                        let steps = dy / 4.000244140625;
                        (steps - steps.round()).abs() <= 1e-6 * steps.abs().max(1.0)
                            || (dy.abs() >= 40.0 && (dy % 10.0).abs() <= 1e-6)
                    };
                    if !is_discrete_wheel && !self.navigation_state.ctrl_held {
                        if self.navigation_state.alt_held {
                            self.camera_handler
                                .process_trackpad_orbit_scroll(Vector2::new(dx, dy));
                        } else {
                            self.pan_handler
                                .process_trackpad_scroll_pan(Vector2::new(dx, dy));
                        }
                        return true;
                    }
                }
                self.shift_handler.process_scroll(delta);
                self.zoom_handler.process_scroll(delta, scale_factor);
                true
            }
            WindowEvent::MouseInput { button, state, .. } => {
                let cursor = self.navigation_state.cursor_position;
                match (*button, *state) {
                    (MouseButton::Left, ElementState::Pressed) => {
                        if let Some(pos) = cursor {
                            if self.navigation_state.process_hud_press(
                                pos,
                                self.last_known_width,
                                self.last_known_height,
                            ) {
                                return true;
                            }
                            if self
                                .navigation_state
                                .check_double_click(Instant::now(), pos)
                            {
                                if let Some(delta) =
                                    self.navigation_state.take_pending_smooth_zoom()
                                {
                                    self.zoom_handler.add_smooth_zoom(delta, Some(pos));
                                }
                                return true;
                            }
                        }
                        if self.navigation_state.alt_held || self.navigation_state.ctrl_held {
                            self.camera_handler.begin_orbit(false);
                        } else if self.navigation_state.pointer_profile == PointerProfile::Rts
                            || self.navigation_state.shift_held
                        {
                            if let Some(pos) = cursor {
                                self.navigation_state.selection_drag = Some((pos, pos));
                            }
                        } else {
                            self.navigation_state.on_manual_pan_or_zoom();
                            self.pan_handler.process_mouse_key_press(button, state);
                        }
                        self.query_handler.process_mouse_key_press(button, state);
                    }
                    (MouseButton::Left, ElementState::Released) => {
                        if self
                            .navigation_state
                            .process_hud_release(self.last_known_zoom, self.last_known_bearing)
                        {
                            if let Some(delta) = self.navigation_state.take_pending_smooth_zoom() {
                                let center = Vector2::new(
                                    self.last_known_width * 0.5,
                                    self.last_known_height * 0.5,
                                );
                                self.zoom_handler.add_smooth_zoom(delta, Some(center));
                            }
                            return true;
                        }
                        self.navigation_state.selection_drag = None;
                        if self.camera_handler.is_active() {
                            self.camera_handler.end_orbit();
                        }
                        if self.pan_handler.is_panning() {
                            self.pan_handler.end_pan();
                        }
                        self.query_handler.process_mouse_key_press(button, state);
                    }
                    (MouseButton::Right, ElementState::Pressed) => {
                        if self.navigation_state.pointer_profile == PointerProfile::Rts
                            && !self.navigation_state.alt_held
                        {
                            self.rts_right_press_origin = cursor;
                            self.rts_right_panning = false;
                        } else {
                            self.camera_handler.begin_orbit(false);
                        }
                    }
                    (MouseButton::Right, ElementState::Released) => {
                        self.rts_right_press_origin = None;
                        if self.rts_right_panning {
                            self.rts_right_panning = false;
                            self.pan_handler.end_pan();
                        }
                        if self.camera_handler.is_active() {
                            self.camera_handler.end_orbit();
                        }
                    }
                    (MouseButton::Middle, ElementState::Pressed) => {
                        if self.navigation_state.pointer_profile == PointerProfile::Rts {
                            // In RTS mode (C&C Generals style), middle-drag orbits both bearing and pitch.
                            self.camera_handler.begin_orbit(false);
                        } else {
                            self.camera_handler.process_mouse_key_press(button, state);
                        }
                    }
                    (MouseButton::Middle, ElementState::Released) => {
                        self.camera_handler.end_orbit();
                    }
                    _ => {
                        self.pan_handler.process_mouse_key_press(button, state);
                        self.query_handler.process_mouse_key_press(button, state);
                        self.camera_handler.process_mouse_key_press(button, state);
                    }
                }
                true
            }
            _ => false,
        }
    }
}

/// Applies accumulated input and time-dependent motion to the map frame.
pub trait UpdateState {
    /// Advances input effects by the elapsed time since the preceding frame.
    fn update_state(&mut self, state: &mut MapContext, dt: Duration);
}

impl UpdateState for InputController {
    fn update_state(&mut self, map_context: &mut MapContext, dt: Duration) {
        self.navigation_state.ensure_initialized(map_context);
        let initial_pitch = map_context.view_state.camera().get_pitch().0;
        let pos_before = map_context.view_state.camera().position();
        let zoom_before = map_context.view_state.zoom().value();
        let before = view_snapshot(map_context);
        if let Some(hud) = map_context
            .world
            .resources
            .get::<maplibre::hud::NavigationHudState>()
        {
            self.zoom_handler.set_sensitivity(hud.zoom_sensitivity);
        }
        let hud_before = map_context
            .world
            .resources
            .get::<maplibre::hud::NavigationHudState>()
            .cloned();

        self.pan_handler.update_state(map_context, dt);
        self.pinch_handler
            .update_state_with_nav(map_context, dt, &mut self.navigation_state);
        self.zoom_handler
            .update_state_with_nav(map_context, dt, Some(&mut self.navigation_state));
        self.camera_handler
            .update_state_with_nav(&mut map_context.view_state, &mut self.navigation_state);
        self.shift_handler
            .update_state_with_nav(map_context, dt, &mut self.navigation_state);
        self.navigation_state.update_frame(map_context, dt);
        self.query_handler.update_state(map_context, dt);
        self.debug_handler.update_state(map_context, dt);

        self.last_known_width = map_context.view_state.width();
        self.last_known_height = map_context.view_state.height();
        self.last_known_zoom = map_context.view_state.zoom().value();
        self.last_known_bearing = map_context.view_state.camera().get_bearing();

        let before_pitch = map_context.view_state.camera().get_pitch().0;
        let handlers_moved = view_snapshot(map_context) != before;

        // Only a view the handlers moved is lifted out of the terrain, as GL JS applies its
        // camera update after a handler changed the transform; elevation data arriving under a
        // resting camera leaves it where it is.
        if handlers_moved {
            let zoom_after_handlers = map_context.view_state.zoom().value();
            let pitch_after_handlers = before_pitch.to_degrees();
            let MapContext {
                style,
                view_state,
                world,
                ..
            } = map_context;
            // Ground `center_elevation` when Auto-Tilt is active or when the gesture started
            // before DEM tiles arrived (`center_elevation == 0`), so terrain clearance is
            // measured relative to the actual terrain under `center` rather than a stale or
            // sea-level reference.
            if style.terrain.is_some()
                && ((self.navigation_state.three_dimensional && self.navigation_state.auto_tilt)
                    || view_state.center_elevation().abs() <= f64::EPSILON
                    || !view_state.center_elevation_frozen())
            {
                let was_frozen = view_state.center_elevation_frozen();
                if was_frozen {
                    view_state.thaw_center_elevation();
                }
                maplibre::terrain::elevation::ground_center(view_state, world);
                if was_frozen {
                    view_state.freeze_center_elevation();
                }
            }
            let pitch_clamped = clamp_pitch_above_terrain_keeping_zoom(style, view_state, world);
            let lifted = keep_camera_above_terrain(style, view_state, world);
            if pitch_clamped || lifted {
                self.navigation_state.cancel_pitch_animation();
                if (view_state.camera().get_pitch().0 - initial_pitch).abs() < 2e-3 {
                    view_state.camera_mut().set_pitch(Rad(initial_pitch));
                }
            }
            let zoom_final = view_state.zoom().value();
            let pitch_final = view_state.camera().get_pitch().0.to_degrees();
            let center_elev = view_state.center_elevation();
            if view_snapshot(map_context) != before {
                tracing::debug!(
                    zoom_handlers = format!("{zoom_after_handlers:.2}"),
                    pitch_handlers = format!("{pitch_after_handlers:.1}"),
                    zoom_final = format!("{zoom_final:.2}"),
                    pitch_final = format!("{pitch_final:.1}"),
                    center_elev = format!("{center_elev:.1}"),
                    pitch_clamped,
                    lifted,
                    three_d = self.navigation_state.three_dimensional,
                    auto_tilt = self.navigation_state.auto_tilt,
                    "camera frame update"
                );
            }
        }
        let camera_moved = view_snapshot(map_context) != before;
        update_velocity_prefetch_view(
            &mut self.smoothed_pan_velocity,
            pos_before,
            zoom_before,
            dt,
            map_context,
        );
        let hud_changed = map_context
            .world
            .resources
            .get::<maplibre::hud::NavigationHudState>()
            != hud_before.as_ref();
        self.moved_view = camera_moved
            || hud_changed
            || self.navigation_state.is_animating()
            || self.shift_handler.is_active();
    }
}

/// Projects a velocity-predictive [`maplibre::render::xr::PrefetchView`] ahead of the camera
/// while panning or edge-scrolling so incoming tiles (especially at the high-zoom bottom edge
/// of a pitched view) are fetched and draped before entering the screen.
fn update_velocity_prefetch_view(
    smoothed_vel: &mut Vector2<f64>,
    pos_before: cgmath::Point2<f64>,
    zoom_before: f64,
    dt: Duration,
    map_context: &mut MapContext,
) {
    if map_context.view_state.has_external_view() {
        return;
    }
    let zoom_after = map_context.view_state.zoom().value();
    let pos_after = map_context.view_state.camera().position();
    let dt_sec = dt.as_secs_f64();
    if dt_sec > 1e-4 && (zoom_after - zoom_before).abs() < 0.05 {
        let world_size = maplibre::coords::TILE_SIZE * 2_f64.powf(zoom_after);
        let mut dx = pos_after.x - pos_before.x;
        if dx > world_size * 0.5 {
            dx -= world_size;
        } else if dx < -world_size * 0.5 {
            dx += world_size;
        }
        let dy = pos_after.y - pos_before.y;
        let step_sec = dt_sec.max(1.0 / 240.0);
        let inst_vel = Vector2::new(dx / step_sec, dy / step_sec);
        let alpha = (dt_sec * 14.0).clamp(0.0, 1.0);
        *smoothed_vel = *smoothed_vel * (1.0 - alpha) + inst_vel * alpha;
    } else {
        *smoothed_vel *= 0.5;
    }

    let speed = smoothed_vel.magnitude();
    if speed > 25.0 {
        let max_reach = map_context
            .view_state
            .width()
            .max(map_context.view_state.height())
            * 0.9;
        let mut lead = *smoothed_vel * 0.80;
        let lead_mag = lead.magnitude();
        if lead_mag > max_reach {
            lead *= max_reach / lead_mag;
        }
        let eye = map_context.view_state.eye_position();
        let to_nadir = Vector2::new(eye.x - pos_after.x, eye.y - pos_after.y);
        let nadir_dist = to_nadir.magnitude();
        if nadir_dist > 1.0 {
            let nadir_dir = to_nadir / nadir_dist;
            let vel_dir = *smoothed_vel / speed;
            let backward_alignment = vel_dir.dot(nadir_dir).max(0.0);
            if backward_alignment > 0.0 {
                lead += to_nadir * (0.55 * backward_alignment);
            }
        }
        let world_size = maplibre::coords::TILE_SIZE * 2_f64.powf(zoom_after);
        let mut ahead = map_context.view_state.clone();
        let new_x = (pos_after.x + lead.x).rem_euclid(world_size);
        let new_y = (pos_after.y + lead.y).clamp(0.0, world_size);
        ahead
            .camera_mut()
            .move_to(cgmath::Point2::new(new_x, new_y));
        map_context
            .world
            .resources
            .get_or_init_mut::<maplibre::render::xr::PrefetchView>()
            .view_state = Some(ahead);
    } else if let Some(prefetch) = map_context
        .world
        .resources
        .get_mut::<maplibre::render::xr::PrefetchView>()
    {
        if prefetch.placement.is_none() {
            prefetch.view_state = None;
        }
    }
}

/// The parts of the view the input handlers change.
fn view_snapshot(map_context: &MapContext) -> impl PartialEq {
    let camera = map_context.view_state.camera();
    let position = camera.position();
    (
        Vector2::new(position.x, position.y),
        camera.get_pitch().0,
        camera.get_bearing().0,
        map_context.view_state.zoom().value(),
        map_context.view_state.globe_pose(),
        *map_context.view_state.edge_insets(),
    )
}
