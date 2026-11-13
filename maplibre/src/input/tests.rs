#![allow(clippy::expect_used, clippy::panic)]

use std::time::Duration;

use cgmath::Deg;
use instant::Instant;

use super::{
    ButtonState, InputController, InputEvent, ModifiersState, MouseScrollDelta, NavKey,
    PointerButton, TouchPhase,
};
use crate::{
    coords::{LatLon, WorldCoords, Zoom},
    render::view_state::{LatLonBounds, ViewState},
    style::Style,
    window::PhysicalSize,
};

fn test_view(lat: f64, lon: f64, zoom: f64) -> ViewState {
    let z = Zoom::new(zoom);
    let mut view = ViewState::new(
        PhysicalSize::new(800, 600).expect("viewport"),
        WorldCoords::from_lat_lon(LatLon::new(lat, lon), z),
        z,
        Deg(60.0),
        Deg(36.87),
    );
    view.set_max_pitch(Deg(85.0));
    view
}

#[test]
fn pointer_drag_pans_view_and_coasts_with_kinetic_inertia() {
    let style = Style::default();
    let mut view = test_view(0.0, 0.0, 5.0);
    let mut ctrl = InputController::new(0.2, 100.0, 1.0);
    // Disable HUD hit-test interference for arbitrary coordinates
    ctrl.navigation_state_mut().show_hud = false;

    let t0 = Instant::now();
    let dt = Duration::from_millis(16);

    ctrl.handle_event_at(
        InputEvent::PointerMoved {
            position: [400.0, 300.0],
        },
        t0,
    );
    ctrl.handle_event_at(
        InputEvent::PointerButton {
            button: PointerButton::Primary,
            state: ButtonState::Pressed,
        },
        t0,
    );
    ctrl.update_view_at(&style, &mut view, dt, t0);

    // Drag right by 120px over two frames (32ms) -> map center moves west (longitude < 0).
    let t1 = t0 + dt;
    ctrl.handle_event_at(
        InputEvent::PointerMoved {
            position: [460.0, 300.0],
        },
        t1,
    );
    ctrl.update_view_at(&style, &mut view, dt, t1);

    let t2 = t1 + dt;
    ctrl.handle_event_at(
        InputEvent::PointerMoved {
            position: [520.0, 300.0],
        },
        t2,
    );
    ctrl.update_view_at(&style, &mut view, dt, t2);
    let lon_at_release = view.center_lat_lon().longitude;
    assert!(
        lon_at_release < -0.5,
        "dragging pointer right must pan center west, got {lon_at_release}"
    );

    // Release primary button -> kinetic pan inertia continues coasting west!
    ctrl.handle_event_at(
        InputEvent::PointerButton {
            button: PointerButton::Primary,
            state: ButtonState::Released,
        },
        t2,
    );
    assert_eq!(ctrl.next_wait(t2), Some(Duration::ZERO));

    for step in 1..=25 {
        let now = t2 + Duration::from_millis(step * 16);
        ctrl.update_view_at(&style, &mut view, dt, now);
    }
    let lon_after_inertia = view.center_lat_lon().longitude;
    assert!(
        lon_after_inertia < lon_at_release - 0.1,
        "kinetic pan inertia must carry the map further west ({lon_at_release} -> {lon_after_inertia})"
    );
}

#[test]
fn multi_touch_pinch_zoom_rotate_inertia_and_two_finger_pitch() {
    let style = Style::default();
    let mut view = test_view(47.3769, 8.5417, 6.0);
    let mut ctrl = InputController::new(0.2, 100.0, 1.0);
    ctrl.navigation_state_mut().show_hud = false;

    let t0 = Instant::now();
    let dt = Duration::from_millis(16);

    // 1) Two-finger spread apart -> zooms in!
    ctrl.handle_event_at(
        InputEvent::Touch {
            id: 1,
            phase: TouchPhase::Started,
            position: [350.0, 300.0],
        },
        t0,
    );
    ctrl.handle_event_at(
        InputEvent::Touch {
            id: 2,
            phase: TouchPhase::Started,
            position: [450.0, 300.0],
        },
        t0,
    );
    let t1 = t0 + dt;
    ctrl.handle_event_at(
        InputEvent::Touch {
            id: 1,
            phase: TouchPhase::Moved,
            position: [300.0, 300.0],
        },
        t1,
    );
    ctrl.handle_event_at(
        InputEvent::Touch {
            id: 2,
            phase: TouchPhase::Moved,
            position: [500.0, 300.0],
        },
        t1,
    );
    ctrl.update_view_at(&style, &mut view, dt, t1);
    assert!(
        view.zoom().value() > 6.8,
        "doubling two-finger span must zoom in ~1 level, got {}",
        view.zoom().value()
    );

    ctrl.handle_event_at(
        InputEvent::Touch {
            id: 1,
            phase: TouchPhase::Ended,
            position: [300.0, 300.0],
        },
        t1 + dt,
    );
    ctrl.handle_event_at(
        InputEvent::Touch {
            id: 2,
            phase: TouchPhase::Ended,
            position: [500.0, 300.0],
        },
        t1 + dt,
    );

    // 2) Two-finger parallel upward drag -> tilts pitch up!
    let t2 = t1 + Duration::from_millis(100);
    ctrl.handle_event_at(
        InputEvent::Touch {
            id: 10,
            phase: TouchPhase::Started,
            position: [300.0, 350.0],
        },
        t2,
    );
    ctrl.handle_event_at(
        InputEvent::Touch {
            id: 11,
            phase: TouchPhase::Started,
            position: [500.0, 350.0],
        },
        t2,
    );
    let t3 = t2 + dt;
    ctrl.handle_event_at(
        InputEvent::Touch {
            id: 10,
            phase: TouchPhase::Moved,
            position: [300.0, 270.0],
        },
        t3,
    );
    ctrl.handle_event_at(
        InputEvent::Touch {
            id: 11,
            phase: TouchPhase::Moved,
            position: [500.0, 270.0],
        },
        t3,
    );
    ctrl.update_view_at(&style, &mut view, dt, t3);
    let pitch_deg: Deg<f64> = view.camera().get_pitch().into();
    assert!(
        pitch_deg.0 > 20.0,
        "parallel upward 2-finger drag must tilt pitch up, got {}",
        pitch_deg.0
    );
    ctrl.handle_event_at(
        InputEvent::Touch {
            id: 10,
            phase: TouchPhase::Ended,
            position: [300.0, 270.0],
        },
        t3 + dt,
    );
    ctrl.handle_event_at(
        InputEvent::Touch {
            id: 11,
            phase: TouchPhase::Ended,
            position: [500.0, 270.0],
        },
        t3 + dt,
    );

    // 3) Two-finger twist -> rotates bearing and coasts further on release with kinetic rotate inertia!
    let t4 = t3 + Duration::from_millis(100);
    ctrl.handle_event_at(
        InputEvent::Touch {
            id: 20,
            phase: TouchPhase::Started,
            position: [300.0, 300.0],
        },
        t4,
    );
    ctrl.handle_event_at(
        InputEvent::Touch {
            id: 21,
            phase: TouchPhase::Started,
            position: [500.0, 300.0],
        },
        t4,
    );
    let t5 = t4 + dt;
    ctrl.handle_event_at(
        InputEvent::Touch {
            id: 20,
            phase: TouchPhase::Moved,
            position: [300.0, 270.0],
        },
        t5,
    );
    ctrl.handle_event_at(
        InputEvent::Touch {
            id: 21,
            phase: TouchPhase::Moved,
            position: [500.0, 330.0],
        },
        t5,
    );
    ctrl.update_view_at(&style, &mut view, dt, t5);
    let bearing_before_release = view.camera().get_bearing().0;
    assert!(
        bearing_before_release.abs() > 0.1,
        "2-finger twist must rotate bearing, got {bearing_before_release}"
    );

    let t6 = t5 + Duration::from_millis(8);
    ctrl.handle_event_at(
        InputEvent::Touch {
            id: 20,
            phase: TouchPhase::Ended,
            position: [300.0, 270.0],
        },
        t6,
    );
    ctrl.handle_event_at(
        InputEvent::Touch {
            id: 21,
            phase: TouchPhase::Ended,
            position: [500.0, 330.0],
        },
        t6,
    );
    assert_eq!(
        ctrl.next_wait(t6),
        Some(Duration::ZERO),
        "active rotation inertia must request frame updates"
    );
    for step in 1..=15 {
        ctrl.update_view_at(&style, &mut view, dt, t6 + Duration::from_millis(step * 16));
    }
    let bearing_after_inertia = view.camera().get_bearing().0;
    assert!(
        bearing_after_inertia.abs() > bearing_before_release.abs() + 0.03,
        "kinetic rotation inertia must continue rotating bearing ({bearing_before_release} -> {bearing_after_inertia})"
    );
}

#[test]
fn pinch_zoom_inertia_double_tap_keyboard_and_camera_bounds_clamping() {
    let style = Style::default();
    let mut view = test_view(10.0, 20.0, 5.0);
    view.set_min_zoom(Some(4.0));
    view.set_max_zoom(Some(7.5));
    view.set_max_bounds(Some(LatLonBounds::new(18.0, 8.0, 22.0, 12.0)));

    let mut ctrl = InputController::new(0.2, 100.0, 1.0);
    ctrl.navigation_state_mut().show_hud = false;

    let t0 = Instant::now();
    let dt = Duration::from_millis(16);

    // Trackpad pinch gesture + release momentum inertia
    ctrl.handle_event_at(
        InputEvent::PinchGesture {
            delta: 0.0,
            phase: TouchPhase::Started,
        },
        t0,
    );
    ctrl.handle_event_at(
        InputEvent::PinchGesture {
            delta: 0.35,
            phase: TouchPhase::Moved,
        },
        t0 + dt,
    );
    ctrl.update_view_at(&style, &mut view, dt, t0 + dt);
    let zoom_mid = view.zoom().value();
    assert!(zoom_mid > 5.4);

    ctrl.handle_event_at(
        InputEvent::PinchGesture {
            delta: 0.0,
            phase: TouchPhase::Ended,
        },
        t0 + dt + Duration::from_millis(8),
    );
    for step in 1..=12 {
        ctrl.update_view_at(
            &style,
            &mut view,
            dt,
            t0 + dt + Duration::from_millis(8 + step * 16),
        );
    }
    let zoom_after_coasting = view.zoom().value();
    assert!(
        zoom_after_coasting > zoom_mid + 0.05,
        "post-pinch zoom inertia must coast further in ({zoom_mid} -> {zoom_after_coasting})"
    );

    // Large double-tap + wheel zoom-in must clamp cleanly at max_zoom = 7.5!
    let t_zoom = t0 + Duration::from_secs(1);
    for step in 0..8 {
        ctrl.handle_event_at(InputEvent::DoubleTapGesture, t_zoom + step * dt);
        ctrl.handle_event_at(
            InputEvent::MouseWheel {
                delta: MouseScrollDelta::LineDelta { x: 0.0, y: 8.0 },
            },
            t_zoom + step * dt,
        );
        ctrl.update_view_at(&style, &mut view, dt, t_zoom + step * dt);
    }
    assert!(
        (view.zoom().value() - 7.5).abs() < 1e-6,
        "gestures must clamp at ViewState max_zoom (7.5), got {}",
        view.zoom().value()
    );

    // Keyboard navigation (`Right` held) pans east while clamping at max_bounds.east = 22.0!
    let t_key = t_zoom + Duration::from_secs(1);
    ctrl.handle_event_at(
        InputEvent::ModifiersChanged(ModifiersState {
            shift: true,
            alt: false,
            ctrl: false,
        }),
        t_key,
    );
    ctrl.handle_event_at(
        InputEvent::Key {
            key: NavKey::Right,
            state: ButtonState::Pressed,
        },
        t_key,
    );
    for step in 1..=60 {
        ctrl.update_view_at(
            &style,
            &mut view,
            Duration::from_millis(50),
            t_key + step * dt,
        );
    }
    ctrl.handle_event_at(
        InputEvent::Key {
            key: NavKey::Right,
            state: ButtonState::Released,
        },
        t_key + Duration::from_secs(2),
    );
    assert!(
        (view.center_lat_lon().longitude - 22.0).abs() < 1e-5,
        "keyboard pan must stop at max_bounds.east (22.0), got {}",
        view.center_lat_lon().longitude
    );
}

#[cfg(feature = "headless")]
mod map_context_tests {
    use std::time::Duration;

    use instant::Instant;

    use super::test_view;
    use crate::{
        context::MapContext,
        headless::create_headless_renderer,
        input::{ButtonState, InputController, InputEvent, PointerButton, UpdateState},
        style::Style,
        tcs::world::World,
    };

    #[tokio::test]
    async fn input_controller_drives_headless_map_context_without_winit() {
        let Ok((_kernel, renderer)) = create_headless_renderer(256, 256, None).await else {
            return;
        };
        let mut ctx = MapContext {
            style: Style::default(),
            world: World::default(),
            view_state: test_view(0.0, 0.0, 4.0),
            renderer,
        };

        let mut input = InputController::new(0.2, 100.0, 1.0);
        input.navigation_state_mut().show_hud = false;

        let t0 = Instant::now();
        let dt = Duration::from_millis(16);
        input.handle_event_at(
            InputEvent::PointerMoved {
                position: [400.0, 300.0],
            },
            t0,
        );
        input.handle_event_at(
            InputEvent::PointerButton {
                button: PointerButton::Secondary,
                state: ButtonState::Pressed,
            },
            t0,
        );
        input.handle_event_at(
            InputEvent::PointerMoved {
                position: [340.0, 260.0],
            },
            t0 + dt,
        );
        input.update_state(&mut ctx, dt);
        input.handle_event_at(
            InputEvent::PointerButton {
                button: PointerButton::Secondary,
                state: ButtonState::Released,
            },
            t0 + dt + dt,
        );

        assert!(input.moved_view());
        assert!(ctx.view_state.camera().get_bearing().0.abs() > 0.05);
        assert!(ctx.view_state.camera().get_pitch().0 > 0.05);
    }
}
