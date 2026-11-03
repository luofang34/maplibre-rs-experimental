use super::ZoomHandler;

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
    assert_zoom(
        handler.zoom_delta.take().map_or(0.0, |zoom| zoom.value()),
        1.0,
        "spreading the fingers to twice their distance",
    );
    handler.process_pinch(-0.2);
    handler.process_pinch(-0.375);
    assert_zoom(
        handler.zoom_delta.take().map_or(0.0, |zoom| zoom.value()),
        -1.0,
        "two pinches to half the distance",
    );
    handler.process_pinch(-1.0);
    assert!(
        handler.zoom_delta.is_none(),
        "a magnification that leaves no scale is ignored"
    );
}

#[test]
fn a_pinch_gesture_of_the_window_reaches_the_zoom() {
    let mut input = super::super::InputController::new(0.2, 100.0, 1.0);
    let pinch = winit::event::WindowEvent::PinchGesture {
        device_id: winit::event::DeviceId::dummy(),
        delta: 1.0,
        phase: winit::event::TouchPhase::Moved,
    };
    assert!(input.window_input(&pinch, 2.0), "the pinch is consumed");
    assert_zoom(
        input
            .zoom_handler
            .zoom_delta
            .map_or(0.0, |zoom| zoom.value()),
        1.0,
        "the pinch's zoom",
    );
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
