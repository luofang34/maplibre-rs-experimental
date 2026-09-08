use cgmath::{Deg, Matrix4, Vector2, Vector4};

use crate::{
    coords::{WorldCoords, Zoom},
    render::view_state::ViewState,
    window::PhysicalSize,
};

#[test]
fn conform_transformation() {
    let fov = Deg(60.0);
    let mut state = ViewState::new(
        PhysicalSize::new(800, 600).unwrap(),
        WorldCoords::at_ground(0.0, 0.0),
        Zoom::new(10.0),
        Deg(0.0),
        fov,
    );

    //state.furthest_distance(state.camera_to_center_distance(), Point2::new(0.0, 0.0));

    let projection = state.view_projection().invert();

    let bottom_left = state
        .window_to_world_at_ground(&Vector2::new(0.0, 0.0), &projection, true)
        .unwrap();
    println!("bottom left on ground {:?}", bottom_left);
    let top_right = state
        .window_to_world_at_ground(&Vector2::new(state.width, state.height), &projection, true)
        .unwrap();
    println!("top right on ground {:?}", top_right);

    let mut rotated =
        Matrix4::from_angle_x(Deg(-30.0)) * Vector4::new(bottom_left.x, bottom_left.y, 0.0, 0.0);

    println!("bottom left rotated around x axis {:?}", rotated);

    rotated = Matrix4::from_angle_y(Deg(-30.0)) * rotated;

    println!("bottom left rotated around x and y axis {:?}", rotated);

    state.camera.set_pitch(Deg(30.0));
    //state.camera.set_yaw(Deg(-30.0));

    // TODO: verify far distance plane calculation
}

fn state_at(zoom: f64, pitch: Deg<f64>) -> ViewState {
    ViewState::new(
        PhysicalSize::new(800, 600).unwrap(),
        WorldCoords::at_ground(256.0 * 2.0_f64.powf(zoom), 256.0 * 2.0_f64.powf(zoom)),
        Zoom::new(zoom),
        pitch,
        Deg(36.87),
    )
}

#[test]
fn pixels_per_meter_follows_world_size_at_the_equator() {
    let state = state_at(0.0, Deg(0.0));
    let expected = 512.0 / (2.0 * std::f64::consts::PI * 6_371_008.8);

    assert!((state.pixels_per_meter() - expected).abs() < 1e-12);
    assert!((state_at(3.0, Deg(0.0)).pixels_per_meter() - expected * 8.0).abs() < 1e-12);
}

#[test]
fn center_elevation_keeps_the_elevated_center_on_screen_center() {
    let mut state = state_at(12.0, Deg(45.0));
    state.set_center_elevation(570.0);
    let center = state.camera.position();

    let clip = state
        .view_projection()
        .project(Vector4::new(center.x, center.y, 570.0, 1.0));
    let window = state.clip_to_window(&clip);

    assert!((window.x - 400.0).abs() < 1e-6);
    assert!((window.y - 300.0).abs() < 1e-6);
}

#[test]
fn a_bearing_of_ninety_degrees_puts_east_at_the_top_of_the_screen() {
    let mut state = state_at(2.0, Deg(0.0));
    state.camera_mut().set_bearing(Deg(90.0));
    let center = state.camera().position();

    let clip = state
        .view_projection()
        .project(Vector4::new(center.x + 100.0, center.y, 0.0, 1.0));
    let window = state.clip_to_window(&clip);

    assert!(
        (window.x - 400.0).abs() < 1e-6,
        "east stays centred: {window:?}"
    );
    assert!(window.y < 300.0, "east is above the centre: {window:?}");
}

#[test]
fn far_plane_stays_finite_at_high_pitch() {
    let mut state = state_at(12.0, Deg(0.0));
    state.set_max_pitch(Deg(85.0));
    state.camera_mut().set_pitch(Deg(85.0));
    let (near, far) = state.depth_range(cgmath::Point2::new(0.0, 0.0));

    assert!(near > 0.0);
    assert!(far.is_finite());
    assert!(far > state.camera_to_center_distance());
    assert!((state.camera.get_pitch().0 - 85.0_f64.to_radians()).abs() < 1e-9);
}

#[test]
fn far_plane_matches_the_camera_distance_when_flat() {
    let state = state_at(5.0, Deg(0.0));
    let (_, far) = state.depth_range(cgmath::Point2::new(0.0, 0.0));

    assert!((far - state.camera_to_center_distance() * 1.01).abs() < 1e-6);
}

#[test]
fn max_pitch_clamps_pitch_changes() {
    let mut state = state_at(5.0, Deg(70.0));
    assert!((state.camera.get_pitch().0 - 60.0_f64.to_radians()).abs() < 1e-9);

    state.set_max_pitch(Deg(85.0));
    state.camera_mut().set_pitch(Deg(90.0));
    assert!((state.camera.get_pitch().0 - 85.0_f64.to_radians()).abs() < 1e-9);
}

#[test]
fn gpu_view_projection_reverses_depth_only() {
    let state = ViewState::new(
        PhysicalSize::new(800, 600).unwrap(),
        WorldCoords::at_ground(1024.0, 2048.0),
        Zoom::new(10.0),
        Deg(25.0),
        Deg(60.0),
    );
    let point = Vector4::new(1000.0, 2100.0, 0.0, 1.0);

    let cpu = state.view_projection().project(point);
    let gpu = state.gpu_view_projection().project(point);

    assert!((cpu.x - gpu.x).abs() < 1e-9);
    assert!((cpu.y - gpu.y).abs() < 1e-9);
    assert!((cpu.w - gpu.w).abs() < 1e-9);
    assert!((gpu.z / gpu.w - (1.0 - cpu.z / cpu.w)).abs() < 1e-9);
}
