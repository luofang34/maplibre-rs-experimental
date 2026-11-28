//! Unit tests for the navigation state machine.

use cgmath::{Deg, Rad};

use super::*;
use crate::{coords::Zoom, render::view_state::ViewState};

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
    use crate::{
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

fn innsbruck_view(pitch: f64) -> ViewState {
    use crate::{
        coords::{LatLon, WorldCoords},
        window::PhysicalSize,
    };
    let zoom = Zoom::new(12.0);
    let mut view = ViewState::new(
        PhysicalSize::new(800, 600).expect("valid viewport"),
        WorldCoords::from_lat_lon(LatLon::new(47.27574, 11.39085), zoom),
        zoom,
        Deg(0.0),
        Deg(pitch),
    );
    view.camera_mut().set_max_pitch(Deg(85.0));
    view.camera_mut().set_pitch(Deg(pitch));
    view
}

#[test]
fn auto_tilt_off_the_curve_wants_frames_until_the_pitch_reaches_it() {
    let mut nav = NavigationState::default();
    let view = innsbruck_view(10.0);
    assert!(!nav.wants_frame(&view), "manual tilt holds still");
    nav.set_auto_tilt(true, 12.0);
    assert!(nav.wants_frame(&view));
    nav.stop_animations();
    assert!(!nav.is_animating());
    assert!(
        nav.wants_frame(&view),
        "recovery onto the curve still moves the camera"
    );
    let on_curve = innsbruck_view(nav.pitch_for_zoom(12.0));
    assert!(!nav.wants_frame(&on_curve));
}

#[test]
fn a_manual_pitch_leaves_auto_tilt_for_manual_3d() {
    let mut nav = NavigationState::default();
    nav.set_auto_tilt(true, 12.0);
    nav.set_manual_pitch(30.0);
    assert!(nav.three_dimensional && !nav.auto_tilt);
    assert_eq!(nav.target_pitch_deg, Some(30.0));
    nav.set_auto_tilt(false, 12.0);
    assert!(!nav.auto_tilt && nav.target_pitch_deg.is_none());
}

#[test]
fn a_compass_drag_turns_and_tilts_unless_north_is_locked() {
    let mut nav = NavigationState::default();
    let mut view = innsbruck_view(40.0);
    nav.drag_compass(&mut view, cgmath::Vector2::new(20.0, 10.0));
    assert!(view.camera().get_bearing().0.abs() > 0.1);
    assert!((view.camera().get_pitch().0.to_degrees() - 35.0).abs() < 1e-6);
    assert!(!nav.auto_tilt);

    let mut locked = NavigationState {
        north_locked: true,
        ..Default::default()
    };
    let mut view = innsbruck_view(40.0);
    locked.drag_compass(&mut view, cgmath::Vector2::new(20.0, 0.0));
    assert_eq!(view.camera().get_bearing().0, 0.0);
}

#[test]
fn bearing_turns_a_free_globe_by_its_pose() {
    use crate::{projection::ProjectionType, render::view_state::NavigationMode};
    let mut view = innsbruck_view(30.0);
    view.set_navigation_mode(
        NavigationMode::FreeGlobe,
        &ProjectionType::VerticalPerspective,
    )
    .expect("free navigation");
    set_bearing_keeping_center(&mut view, Deg(30.0));
    assert!((view.camera().get_bearing().0.to_degrees() - 30.0).abs() < 1e-6);
    assert!(view.globe_pose().is_some());
}

#[test]
fn advancing_eases_auto_tilt_onto_its_curve_and_then_stops_wanting_frames() {
    let mut nav = NavigationState::default();
    let mut view = innsbruck_view(10.0);
    let style = crate::style::Style::default();
    nav.set_auto_tilt(true, view.zoom().value());
    let mut last = view.camera().get_pitch().0;
    let mut frames = 0;
    while nav.wants_frame(&view) {
        nav.advance(&style, &mut view, std::time::Duration::from_millis(16));
        let pitch = view.camera().get_pitch().0;
        assert!(pitch >= last, "the tilt rises steadily");
        last = pitch;
        frames += 1;
        assert!(frames < 200, "the ease settles");
    }
    let expected = nav.pitch_for_zoom(view.zoom().value());
    assert!((view.camera().get_pitch().0.to_degrees() - expected).abs() < 0.12);
}
