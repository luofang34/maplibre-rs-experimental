#![allow(clippy::expect_used, clippy::panic)]

use super::*;
use crate::{
    coords::{WorldCoords, Zoom},
    window::PhysicalSize,
};
use cgmath::Deg;

fn level_transition(pitch: f64, bearing: f64, near: f64, height: f64) -> ViewState {
    let mut state = ViewState::new(
        PhysicalSize::new(1888, 1792).expect("viewport"),
        WorldCoords::from((256.0, 256.0)),
        Zoom::new(14.0),
        Deg(0.0),
        Deg(36.87),
    );
    let eye = Matrix4::from_translation(Vector3::new(1000.0, -2000.0, height))
        * Matrix4::from_angle_z(Deg(bearing))
        * Matrix4::from_angle_x(Deg(pitch))
        * Matrix4::from_angle_z(Deg(3.7));
    // Compositor poses cross the FFI as floats even though covering uses doubles.
    let view = eye
        .invert()
        .expect("eye")
        .cast::<f32>()
        .expect("float pose")
        .cast::<f64>()
        .expect("double pose");
    state
        .set_external_view(
            ExternalView {
                anchor: ExternalAnchor {
                    position: LatLon::new(47.26, 11.39),
                    altitude_meters: 600.0,
                },
                view,
                frustum: EyeFrustum {
                    left: 1.1,
                    right: 0.9,
                    top: 1.0,
                    bottom: 0.8,
                    near,
                    far: near * 1.0e10,
                },
            },
            &ProjectionType::Mercator,
        )
        .expect("valid external view");
    state
}

fn assert_corners(state: &ViewState) {
    let frustum = state.external_frustum().expect("frustum");
    let slopes = [
        (-frustum.left, frustum.top),
        (frustum.right, frustum.top),
        (frustum.right, -frustum.bottom),
        (-frustum.left, -frustum.bottom),
    ];
    for (index, corner) in state
        .frustum_corners()
        .expect("valid frustum")
        .iter()
        .enumerate()
    {
        let eye = FLIP_Y * state.camera_matrix() * corner.extend(1.0);
        let depth = if index < 4 { frustum.far } else { frustum.near };
        let (x, y) = slopes[index % 4];
        let expected = Vector3::new(x, y, -1.0);
        let actual = eye.truncate() / eye.w / depth;
        // Near points are separated by less than one world-coordinate ULP at extreme
        // clip ratios; the error bound accounts for the world-to-eye subtraction.
        let world_size = TILE_SIZE * 2_f64.powf(state.zoom().value());
        let tolerance = 1e-5 + 32.0 * f64::EPSILON * world_size / depth;
        assert!(
            (actual - expected).magnitude() < tolerance,
            "corner={index}, frustum={frustum:?}: {actual:?} vs {expected:?}"
        );
    }
}

#[test]
fn leveling_with_head_rotation_preserves_frustum_corners() {
    for height in [150.0, 4000.0, 60000.0] {
        for near in [1.0e-7, 1.0e-5, 0.001, 0.01, 0.05] {
            for bearing in [0.0, 13.7, 37.3, 90.0, 167.4, 271.8] {
                for step in 0..=100 {
                    let state =
                        level_transition(45.0 + f64::from(step) * 0.5, bearing, near, height);
                    assert_corners(&state);
                    assert_corners(&state.overscanned(1.5).expect("overscan"));
                    assert_corners(
                        &state
                            .surround(2.0, &ProjectionType::Mercator)
                            .expect("surround"),
                    );
                }
            }
        }
    }
}

#[test]
fn invalid_world_scale_returns_a_covering_error() {
    use crate::projection::globe::{
        covering::TileElevationRange,
        covering_tiles::{SourceZoomRange, ZoomRounding},
    };
    use crate::projection::mercator::{
        covering_tiles, MercatorCoveringError, MercatorCoveringOptions,
    };
    let mut state = level_transition(90.0, 37.3, 0.05, 4000.0);
    state.update_zoom(Zoom::new(-2048.0));
    let result = covering_tiles(
        &state,
        MercatorCoveringOptions {
            zoom: crate::coords::ZoomLevel::new(12),
            requested_zoom: 12.0,
            variable_zoom: true,
            rounding: ZoomRounding::Floor,
            zoom_range: SourceZoomRange::default(),
            padding: 0,
            max_tiles: 12,
        },
        &TileElevationRange::default(),
    );
    assert!(matches!(
        result,
        Err(MercatorCoveringError::ViewProjection { .. })
    ));
}
