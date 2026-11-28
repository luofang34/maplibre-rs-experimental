//! The eye kept out of the terrain keeps the near plane out of it too, so a slope rising
//! just in front of a low eye is not cut open.

use cgmath::{Vector3, Vector4};

use super::*;
use crate::{projection::globe::unit_sphere_to_lat_lon, terrain::sightline::DrawnTerrain};

/// How far below the drawn ground the lowest corner of the view's near plane stands, in
/// metres; negative where every corner stands above it.
fn near_plane_depth_in_ground(state: &ViewState, world: &World) -> f64 {
    let camera = globe_camera_for_view(state).expect("globe camera");
    let radius = state.body().radius_meters;
    let terrain = DrawnTerrain {
        index: world.resources.get().expect("index"),
        tiles: &world.tiles,
        body: state.body(),
    };
    [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)]
        .into_iter()
        .map(|(x, y)| {
            let corner = camera.inverse_view_projection() * Vector4::new(x, y, -1.0, 1.0);
            let corner: Vector3<f64> = corner.truncate() / corner.w;
            let ground = terrain
                .ground_at(unit_sphere_to_lat_lon(corner.normalize()))
                .expect("known")
                .expect("drawn");
            ground - (corner.magnitude() - 1.0) * radius
        })
        .fold(f64::NEG_INFINITY, f64::max)
}

#[test]
fn an_eye_low_over_a_slope_keeps_the_near_plane_out_of_the_slope_ahead() {
    for bearing in [30.0, 135.0, 210.0, 324.037_142_967_012_36] {
        let mut state = view_at(SCENE, 14.0, 85.0, bearing, 0.0);
        let (eye, target) = globe_pose(&state).expect("orbiting camera");
        let radius = state.body().radius_meters;
        let height = (eye.magnitude() - 1.0) * radius;
        let foot = lat_lon_to_mercator(unit_sphere_to_lat_lon(eye.normalize()));
        let meters_per_unit =
            2.0 * std::f64::consts::PI * radius * SCENE.latitude.to_radians().cos();
        // Ahead along the ground, from the eye toward its target.
        let toward = lat_lon_to_mercator(unit_sphere_to_lat_lon(target.normalize())) - foot;
        let (sin, cos) = (toward.x / toward.magnitude(), toward.y / toward.magnitude());
        let under = unit_sphere_to_lat_lon(eye.normalize());
        // A plane rising at 45° away from the eye: a plane is drawn as a plane between the
        // DEM's samples, offset by where the synthetic tiles place their samples.
        let slope = move |base: f64| {
            Ground::around(SCENE, move |mercator| {
                let ahead =
                    ((mercator.x - foot.x) * sin + (mercator.y - foot.y) * cos) * meters_per_unit;
                base + ahead
            })
        };
        let drawn_under = |ground: &Ground| {
            ground
                .terrain()
                .ground_at(under)
                .expect("known")
                .expect("drawn")
        };
        // Drawn two and a half metres under the eye, so the eye clears the ground under it.
        let world = world(slope(height - 2.5 - drawn_under(&slope(0.0))));
        let before = near_plane_depth_in_ground(&state, &world);
        assert!(
            before > 0.0,
            "bearing {bearing}: the near plane of an eye low over the slope cuts it"
        );
        keep_globe_camera_above_terrain(&mut state, &world);
        let depth = near_plane_depth_in_ground(&state, &world);
        assert!(
            depth < 0.0,
            "bearing {bearing}: the kept eye's near plane is {depth} m inside the slope"
        );
    }
}
