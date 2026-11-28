use std::time::Duration;

use cgmath::Vector2;
use instant::Instant;

use super::*;

#[test]
fn two_finger_parallel_drag_tilts_and_disengages_auto_tilt() {
    let mut pinch = PinchHandler::default();
    let mut pan = PanHandler::default();
    let mut zoom = ZoomHandler::new(1.0);
    let mut nav = NavigationState::default();
    nav.three_dimensional = true;
    nav.auto_tilt = true;
    let t0 = Instant::now();

    pinch.on_touch_start(1, Vector2::new(200.0, 300.0), &mut pan);
    pinch.on_touch_start(2, Vector2::new(400.0, 300.0), &mut pan);
    assert_eq!(pinch.touch_count(), 2);

    // Both fingers drag upward by 40px -> tilts up and disengages auto_tilt
    pinch.on_touch_move_at(
        1,
        Vector2::new(200.0, 260.0),
        &mut pan,
        &mut zoom,
        &mut nav,
        t0 + Duration::from_millis(16),
    );
    pinch.on_touch_move_at(
        2,
        Vector2::new(400.0, 260.0),
        &mut pan,
        &mut zoom,
        &mut nav,
        t0 + Duration::from_millis(16),
    );
    assert!(!nav.auto_tilt, "manual 2-finger tilt disengages auto-tilt");
    assert!(pinch.pending_pitch_deg > 0.0);

    // Lifting second finger cleanly rebases single finger pan
    pinch.on_touch_end_at(2, &mut pan, t0 + Duration::from_millis(32));
    assert_eq!(pinch.touch_count(), 1);
    assert!(pan.is_panning());
}

#[test]
fn two_finger_twist_rotates_bearing_and_coasts_with_angular_inertia() {
    let mut pinch = PinchHandler::default();
    let mut pan = PanHandler::default();
    let mut zoom = ZoomHandler::new(1.0);
    let mut nav = NavigationState::default();
    nav.three_dimensional = true;
    nav.auto_tilt = true;
    let t0 = Instant::now();

    pinch.on_touch_start(1, Vector2::new(200.0, 300.0), &mut pan);
    pinch.on_touch_start(2, Vector2::new(400.0, 300.0), &mut pan);

    // Finger 1 moves up while Finger 2 moves down (clockwise twist):
    pinch.on_touch_move_at(
        1,
        Vector2::new(200.0, 280.0),
        &mut pan,
        &mut zoom,
        &mut nav,
        t0 + Duration::from_millis(16),
    );
    pinch.on_touch_move_at(
        2,
        Vector2::new(400.0, 320.0),
        &mut pan,
        &mut zoom,
        &mut nav,
        t0 + Duration::from_millis(16),
    );

    assert!(
        pinch.twist_unlocked,
        "two-finger twist should unlock rotation"
    );
    assert!(
        pinch.pending_bearing_rad < -0.1,
        "clockwise twist should accumulate negative bearing delta, got {}",
        pinch.pending_bearing_rad
    );

    // Releasing right after twisting launches angular rotation inertia!
    pinch.on_touch_end_at(2, &mut pan, t0 + Duration::from_millis(24));
    assert!(
        pinch.is_coasting() && pinch.rotate_inertia_velocity < 0.0,
        "rapid twist release starts kinetic angular inertia"
    );
}
