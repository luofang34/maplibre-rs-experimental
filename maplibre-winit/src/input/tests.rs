use std::time::Duration;

use cgmath::Deg;
use instant::Instant;
use maplibre::{
    coords::{LatLon, WorldCoords, Zoom},
    render::view_state::ViewState,
    style::Style,
    window::PhysicalSize,
};
use winit::{
    dpi::PhysicalPosition,
    event::{
        DeviceId, ElementState, MouseButton, MouseScrollDelta, Touch, TouchPhase, WindowEvent,
    },
};

use super::InputController;

fn test_view() -> ViewState {
    let zoom = Zoom::new(5.0);
    let mut view = ViewState::new(
        PhysicalSize::new(800, 600).expect("viewport"),
        WorldCoords::from_lat_lon(LatLon::new(47.3769, 8.5417), zoom),
        zoom,
        Deg(60.0),
        Deg(36.87),
    );
    view.set_max_pitch(Deg(85.0));
    view
}

#[test]
fn a_pinch_gesture_of_the_window_reaches_the_zoom_and_coasts_on_end() {
    let style = Style::default();
    let mut view = test_view();
    let mut input = InputController::new(0.2, 100.0, 1.0);
    let pinch = WindowEvent::PinchGesture {
        device_id: DeviceId::dummy(),
        delta: 0.5,
        phase: TouchPhase::Moved,
    };
    assert!(input.window_input(&pinch, 2.0), "the pinch is consumed");
    input
        .inner_mut()
        .update_view(&style, &mut view, Duration::from_millis(16));
    let expected = 5.0 + (1.0 + 0.5 * 1.65_f64).log2();
    assert!(
        (view.zoom().value() - expected).abs() < 1e-6,
        "got {}, expected {expected}",
        view.zoom().value()
    );

    let end = WindowEvent::PinchGesture {
        device_id: DeviceId::dummy(),
        delta: 0.0,
        phase: TouchPhase::Ended,
    };
    assert!(input.window_input(&end, 2.0), "the pinch end is consumed");
    assert_eq!(
        input.next_wait(Instant::now()),
        Some(Duration::ZERO),
        "active pinch inertia requests immediate frame updates"
    );
}

#[test]
fn web_ctrl_wheel_pinch_scales_by_display_scale_factor() {
    let style = Style::default();
    let mut view = test_view();
    let mut input = InputController::new(0.2, 100.0, 1.0);
    input.inner_mut().navigation_state_mut().ctrl_held = true;

    // 10 physical pixels at scale_factor 2.0 = 5 logical pixels = +0.05 magnification
    let wheel = WindowEvent::MouseWheel {
        device_id: DeviceId::dummy(),
        delta: MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, 10.0)),
        phase: TouchPhase::Moved,
    };
    assert!(input.window_input(&wheel, 2.0));
    input
        .inner_mut()
        .update_view(&style, &mut view, Duration::from_millis(16));
    let expected = 5.0 + (1.0 + 0.05 * 1.65_f64).log2();
    assert!(
        (view.zoom().value() - expected).abs() < 1e-6,
        "got {}, expected {expected}",
        view.zoom().value()
    );
}

#[test]
fn cursor_drag_and_touch_translate_physical_pixels_by_scale_factor() {
    let style = Style::default();
    let mut view = test_view();
    let lon_before = view.center_lat_lon().longitude;
    let mut input = InputController::new(0.2, 100.0, 1.0);
    input.inner_mut().navigation_state_mut().show_hud = false;

    // Physical (800, 600) at scale_factor 2.0 -> logical (400, 300)
    assert!(input.window_input(
        &WindowEvent::CursorMoved {
            device_id: DeviceId::dummy(),
            position: PhysicalPosition::new(800.0, 600.0),
        },
        2.0
    ));
    assert!(input.window_input(
        &WindowEvent::MouseInput {
            device_id: DeviceId::dummy(),
            state: ElementState::Pressed,
            button: MouseButton::Left,
        },
        2.0
    ));
    assert!(input.window_input(
        &WindowEvent::CursorMoved {
            device_id: DeviceId::dummy(),
            position: PhysicalPosition::new(1000.0, 600.0),
        },
        2.0
    ));
    input
        .inner_mut()
        .update_view(&style, &mut view, Duration::from_millis(16));
    assert!(input.window_input(
        &WindowEvent::MouseInput {
            device_id: DeviceId::dummy(),
            state: ElementState::Released,
            button: MouseButton::Left,
        },
        2.0
    ));
    assert!(view.center_lat_lon().longitude < lon_before - 0.5);

    // Touch events also forward through winit translation
    assert!(input.window_input(
        &WindowEvent::Touch(Touch {
            device_id: DeviceId::dummy(),
            phase: TouchPhase::Started,
            location: PhysicalPosition::new(400.0, 400.0),
            force: None,
            id: 7,
        }),
        2.0
    ));
    assert!(input.window_input(
        &WindowEvent::Touch(Touch {
            device_id: DeviceId::dummy(),
            phase: TouchPhase::Ended,
            location: PhysicalPosition::new(400.0, 400.0),
            force: None,
            id: 7,
        }),
        2.0
    ));
}
