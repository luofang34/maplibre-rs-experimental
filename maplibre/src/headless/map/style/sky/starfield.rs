//! Stars sit at infinity behind the globe: the globe hides those beyond it, and the rest turn
//! with the camera.
#![allow(clippy::expect_used, clippy::panic)]

use cgmath::{InnerSpace, Vector3, Vector4};

use crate::{
    celestial::{EcefDirection, Star},
    headless::map::{readback::read_blocking, HeadlessMap},
    projection::globe::camera::GlobeCameraState,
    render::{projection::globe_camera_for_view, view_state::CameraOptions, RenderPlugin},
    style::Style,
};

const SIZE: u32 = 256;
/// Pixels around a predicted position searched for the star, which covers several.
const WINDOW: i32 = 3;
/// The globe's background, which the hidden star must not show through.
const GROUND: u8 = 0x30;

async fn globe() -> HeadlessMap {
    let style: Style = serde_json::from_str(r##"{"version":8,"center":[0,0],"zoom":0,"sources":{},"layers":[{"id":"background","type":"background","paint":{"background-color":"#303030"}}],"projection":{"type":"globe"}}"##).expect("style");
    let (kernel, renderer) = crate::headless::create_headless_renderer(SIZE, SIZE, None)
        .await
        .expect("renderer");
    HeadlessMap::new(
        style,
        renderer,
        kernel,
        vec![
            Box::new(RenderPlugin),
            Box::new(crate::background::BackgroundPlugin),
            Box::new(crate::starfield::StarfieldPlugin),
        ],
    )
    .expect("map")
}

fn frame(map: &mut HeadlessMap) -> Vec<u8> {
    map.run_frame().expect("frame");
    read_blocking(
        map,
        map.head_texture().expect("head texture"),
        wgpu::TextureAspect::All,
    )
    .expect("readback")
}

fn camera(map: &HeadlessMap) -> GlobeCameraState {
    globe_camera_for_view(map.view_state()).expect("camera")
}

/// The ECEF direction of a globe world direction, undoing `ecef_to_globe_world`.
fn ecef(globe: Vector3<f64>) -> EcefDirection {
    EcefDirection::new(globe.z, globe.x, globe.y).expect("direction")
}

/// The globe world direction from the eye through pixel `(x, y)`.
fn direction_through(camera: &GlobeCameraState, x: f64, y: f64) -> Vector3<f64> {
    let ndc = (
        x / f64::from(SIZE) * 2.0 - 1.0,
        1.0 - y / f64::from(SIZE) * 2.0,
    );
    let far = camera.inverse_view_projection() * Vector4::new(ndc.0, ndc.1, 0.5, 1.0);
    (far.truncate() / far.w - camera.camera_position()).normalize()
}

/// The pixel a direction at infinity projects to, as the starfield's shader projects it.
fn pixel_of(camera: &GlobeCameraState, direction: Vector3<f64>) -> (i32, i32) {
    let clip = camera.view_projection() * direction.extend(0.0);
    assert!(clip.w > 0.0, "the direction lies ahead of the eye");
    let (x, y) = (clip.x / clip.w, clip.y / clip.w);
    (
        ((x + 1.0) / 2.0 * f64::from(SIZE)) as i32,
        ((1.0 - y) / 2.0 * f64::from(SIZE)) as i32,
    )
}

/// The brightest colour channel within the window around `(x, y)`.
fn brightest_near(pixels: &[u8], (x, y): (i32, i32)) -> u8 {
    let mut brightest = 0;
    for dy in -WINDOW..=WINDOW {
        for dx in -WINDOW..=WINDOW {
            let (px, py) = (x + dx, y + dy);
            if (0..SIZE as i32).contains(&px) && (0..SIZE as i32).contains(&py) {
                let at = ((py as u32 * SIZE + px as u32) * 4) as usize;
                brightest = brightest.max(pixels[at..at + 3].iter().copied().max().unwrap_or(0));
            }
        }
    }
    brightest
}

#[tokio::test]
async fn stars_hide_behind_the_globe_and_turn_with_the_camera() {
    let mut map = globe().await;
    let plain = frame(&mut map);
    let before = camera(&map);
    // A star seen in space to the right of the globe, whose disc spans about 81 pixels.
    let in_space = direction_through(&before, 224.0, 128.0);
    // A star straight beyond the globe's centre, which the globe must hide.
    let beyond = (-before.camera_position()).normalize();
    // A star behind the eye, which nothing must draw.
    let behind = before.camera_position().normalize();
    let stars = [in_space, beyond, behind]
        .map(|direction| Star::new(ecef(direction), -1.0, None).expect("star"));
    map.set_stars(stars.to_vec());

    let lit = frame(&mut map);
    let star_at = pixel_of(&before, in_space);
    assert!(
        brightest_near(&plain, star_at) < 10,
        "no star before any is set"
    );
    assert!(
        brightest_near(&lit, star_at) > 200,
        "the star in space shows at {star_at:?}"
    );
    let centre = pixel_of(&before, beyond);
    assert_eq!(
        brightest_near(&lit, centre),
        GROUND,
        "the globe hides the star beyond it"
    );

    // Turning the view a quarter carries the star with the sky, a quarter around the globe.
    map.jump_to(CameraOptions {
        bearing: Some(cgmath::Deg(90.0)),
        ..Default::default()
    })
    .expect("jump");
    let turned = frame(&mut map);
    let moved_to = pixel_of(&camera(&map), in_space);
    assert!(
        (moved_to.0 - star_at.0).abs() + (moved_to.1 - star_at.1).abs() > 60,
        "a quarter turn moves the star from {star_at:?}, not to {moved_to:?}"
    );
    assert!(
        brightest_near(&turned, moved_to) > 200,
        "the star follows to {moved_to:?}"
    );
    assert!(
        brightest_near(&turned, star_at) < 10,
        "and leaves {star_at:?}"
    );

    map.set_stars(Vec::new());
    assert!(brightest_near(&frame(&mut map), moved_to) < 10);
}
