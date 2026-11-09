use std::time::Duration;

use instant::Instant;
use winit::event::TouchPhase;

use super::{
    apple_pinch_scale, ZoomHandler, PINCH_INERTIA_GAIN, PINCH_ZOOM_IN_GAIN, PINCH_ZOOM_OUT_EXP,
    PINCH_ZOOM_OUT_GAIN, PINCH_ZOOM_OUT_K,
};

fn assert_zoom(actual: f64, expected: f64, what: &str) {
    assert!(
        (actual - expected).abs() < 1e-6,
        "{what}: zoom {actual}, expected {expected}"
    );
}

#[test]
fn a_pinch_scales_the_map_by_its_magnification() {
    let mut handler = ZoomHandler::new(1.0);
    handler.process_pinch(1.0);
    let expected_in = (1.0 + 1.0 * PINCH_ZOOM_IN_GAIN).log2();
    assert_zoom(
        handler.zoom_delta.take().map_or(0.0, |zoom| zoom.value()),
        expected_in,
        "spreading the fingers scales by Apple Maps zoom-in curve",
    );

    handler.process_pinch(-0.2);
    handler.process_pinch(-0.375);
    let cum_out = (-0.2 - 0.375) * PINCH_ZOOM_OUT_GAIN;
    let expected_out =
        (1.0 / (1.0 + PINCH_ZOOM_OUT_K * (-cum_out).powf(PINCH_ZOOM_OUT_EXP))).log2();
    assert_zoom(
        handler.zoom_delta.take().map_or(0.0, |zoom| zoom.value()),
        expected_out,
        "two pinches accumulate on Apple Maps UITransformEvent zoom-out curve",
    );

    handler.process_pinch(-1.0);
    assert!(
        handler.zoom_delta.is_none(),
        "a magnification that leaves no scale is ignored"
    );
}

#[test]
fn a_pinch_gesture_of_the_window_reaches_the_zoom_and_coasts_on_end() {
    let mut input = super::super::InputController::new(0.2, 100.0, 1.0);
    let pinch = winit::event::WindowEvent::PinchGesture {
        device_id: winit::event::DeviceId::dummy(),
        delta: 0.5,
        phase: TouchPhase::Moved,
    };
    assert!(input.window_input(&pinch, 2.0), "the pinch is consumed");
    let expected = apple_pinch_scale(0.5 * PINCH_ZOOM_IN_GAIN).log2();
    assert_zoom(
        input
            .zoom_handler
            .zoom_delta
            .map_or(0.0, |zoom| zoom.value()),
        expected,
        "the pinch's zoom",
    );

    let end = winit::event::WindowEvent::PinchGesture {
        device_id: winit::event::DeviceId::dummy(),
        delta: 0.0,
        phase: TouchPhase::Ended,
    };
    assert!(input.window_input(&end, 2.0), "the pinch end is consumed");
    assert!(
        input.zoom_handler.pinch_inertia_velocity > 0.0,
        "releasing a moving pinch starts Apple Maps momentum deceleration"
    );
    assert_eq!(
        input.zoom_handler.next_wait(Instant::now()),
        Some(Duration::ZERO),
        "active pinch inertia requests immediate frame updates"
    );
}

#[test]
fn pinch_pause_before_release_suppresses_inertia() {
    let mut handler = ZoomHandler::new(1.0);
    let t0 = Instant::now();
    handler.process_pinch_gesture_at(0.0, TouchPhase::Started, t0);
    handler.process_pinch_gesture_at(0.1, TouchPhase::Moved, t0 + Duration::from_millis(16));
    let _ = handler.zoom_delta.take();
    // Pause > 150ms before lifting fingers
    handler.process_pinch_gesture_at(0.0, TouchPhase::Ended, t0 + Duration::from_millis(200));
    assert_zoom(
        handler.pinch_inertia_velocity,
        0.0,
        "pausing >150ms before finger release suppresses pinch inertia",
    );
}

#[test]
fn web_ctrl_wheel_pinch_uses_apple_pinch_curve() {
    let mut input = super::super::InputController::new(0.2, 100.0, 1.0);
    input.navigation_state.ctrl_held = true;
    // 10 physical pixels at scale_factor 2.0 = 5 logical pixels = +0.05 magnification
    let wheel = winit::event::WindowEvent::MouseWheel {
        device_id: winit::event::DeviceId::dummy(),
        delta: winit::event::MouseScrollDelta::PixelDelta(winit::dpi::PhysicalPosition::new(
            0.0, 10.0,
        )),
        phase: TouchPhase::Moved,
    };
    assert!(input.window_input(&wheel, 2.0));
    let expected = apple_pinch_scale(0.05 * PINCH_ZOOM_IN_GAIN).log2();
    assert_zoom(
        input
            .zoom_handler
            .zoom_delta
            .map_or(0.0, |zoom| zoom.value()),
        expected,
        "browser ctrl+wheel trackpad pinch uses Apple Maps pinch curve without 40ms delay",
    );
    let _ = PINCH_INERTIA_GAIN;
}

#[test]
fn zoom_handler_reports_wait_for_open_gesture_and_held_pixel_scroll() {
    use std::time::Duration;

    use instant::Instant;
    use winit::{dpi::PhysicalPosition, event::MouseScrollDelta};

    let mut handler = ZoomHandler::new(1.0);
    let start = Instant::now();
    assert_eq!(handler.next_wait(start), None);

    handler.scroll.add(
        &MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, 200.0)),
        2.0,
        start,
    );
    assert_eq!(
        handler.next_wait(start + Duration::from_millis(10)),
        Some(Duration::from_millis(30))
    );

    handler.last_input = Some(start);
    assert_eq!(
        handler.next_wait(start + Duration::from_millis(10)),
        Some(Duration::from_millis(30)),
        "chooses the earlier scroll deadline"
    );
    let _ = handler.scroll.take_zoom(start + Duration::from_millis(45));
    let _ = handler.zoom_delta.take();
    assert_eq!(
        handler.next_wait(start + Duration::from_millis(50)),
        Some(Duration::from_millis(150)),
        "keeps waiting for GESTURE_END_DELAY while the gesture is open"
    );
}
