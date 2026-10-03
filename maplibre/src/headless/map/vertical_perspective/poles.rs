//! A free-globe camera dragged across a pole keeps the drawn ground, the polar cap included,
//! where a pick finds it, frame after frame.

use super::*;
use crate::{
    coords::{LatLon, WorldCoords},
    projection::globe::scale::MERCATOR_LATITUDE_LIMIT,
    render::view_state::NavigationMode,
    terrain::{
        sightline::{pick_globe_terrain, DrawnTerrain, TerrainPick},
        TerrainCoverageIndex,
    },
};

/// Distance along the view axis to the ground or cap picked through `pixel`, `None` for the
/// sky; unknown ground fails the check, the frame having settled.
fn ground_distance(
    camera: &crate::projection::globe::camera::GlobeCameraState,
    terrain: DrawnTerrain<'_>,
    pixel: Point2<f64>,
) -> Option<f64> {
    let hit = match pick_globe_terrain(camera, terrain, pixel) {
        TerrainPick::Ground(hit) | TerrainPick::PolarCap(hit) => hit,
        TerrainPick::Sky => return None,
        TerrainPick::Unknown => panic!("{pixel:?} looks at ground without DEM"),
    };
    let point = crate::projection::globe::lat_lon_to_unit_sphere(hit.location)
        * camera.body().unit_radius_at(hit.elevation);
    let view = camera.view() * point.extend(1.0);
    Some(-view.z / view.w)
}

/// Every sampled pixel stores the depth of the nearest ground its samples pick; returns how
/// many pixels show tiles and how many show a cap. A pixel on the ground's silhouette against
/// the sky, where some samples pick ground and others the sky, is covered or not as the last
/// bit of a vertex decides, and is only counted.
fn check_frame(level: &LevelMap, depth: &[f32], case: &str) -> (usize, usize) {
    let camera = globe_camera_for_view(level.map.view_state()).expect("camera");
    let (near, far) = camera.depth_range();
    let world = level.map.world();
    let index = world
        .resources
        .get::<TerrainCoverageIndex>()
        .expect("coverage index");
    let terrain = DrawnTerrain {
        index,
        tiles: &world.tiles,
        body: camera.body(),
    };
    let samples = [
        (0.375, 0.125),
        (0.875, 0.375),
        (0.125, 0.625),
        (0.625, 0.875),
    ];
    let (mut tiles, mut caps, mut silhouettes, mut sampled) = (0, 0, 0, 0);
    for y in (0..HEIGHT).step_by(150) {
        for x in (0..WIDTH).step_by(150) {
            let picks: Vec<f64> = samples
                .iter()
                .filter_map(|(dx, dy)| {
                    ground_distance(
                        &camera,
                        terrain,
                        Point2::new(f64::from(x) + dx, f64::from(y) + dy),
                    )
                })
                .collect();
            let stored = depth[(y * WIDTH + x) as usize];
            let Some(nearest) = picks.iter().copied().reduce(f64::min) else {
                continue;
            };
            sampled += 1;
            if picks.len() < samples.len() {
                silhouettes += 1;
                continue;
            }
            let farthest = picks.iter().copied().fold(nearest, f64::max);
            assert!(
                stored > 0.0,
                "{case}: ({x},{y}) picks ground the frame left empty"
            );
            let drawn = near * far / (f64::from(stored) * (far - near) + near);
            let tolerance = if farthest > nearest * 1.01 {
                farthest / nearest
            } else {
                1.0
            };
            assert!(
                drawn > nearest * (1.0 - 2e-3) && drawn < nearest * tolerance * (1.0 + 2e-3),
                "{case}: ({x},{y}) drawn {drawn} px, picked {nearest}..{farthest} px"
            );
            let center = camera
                .screen_point_to_location_at(Point2::new(f64::from(x), f64::from(y)), 0.0)
                .map_or(0.0, |location| location.latitude.abs());
            if center > MERCATOR_LATITUDE_LIMIT {
                caps += 1;
            } else {
                tiles += 1;
            }
        }
    }
    assert!(
        silhouettes * 10 < sampled,
        "{case}: {silhouettes} of {sampled} pixels lie on a silhouette"
    );
    (tiles, caps)
}

#[tokio::test]
async fn a_free_camera_dragged_over_the_north_pole_keeps_ground_and_cap_where_picks_find_them() {
    let scene = Scene {
        meters: 500.0,
        relief: 0.0,
        exaggeration: 1.0,
        pitch: 50.0,
        zoom: 4.0,
    };
    let mut level = LevelMap::new(scene).await;
    let state = &mut level.map.map_context.view_state;
    let start = WorldCoords::from_lat_lon(LatLon::new(84.0, 10.0), state.zoom());
    state.camera_mut().move_to(Point2::new(start.x, start.y));
    state.camera_mut().set_bearing(cgmath::Deg(0.0));
    level
        .map
        .set_navigation_mode(NavigationMode::FreeGlobe)
        .expect("free navigation");
    let center = Point2::new(f64::from(WIDTH) / 2.0, f64::from(HEIGHT) / 2.0);
    let (mut seen_tiles, mut seen_caps, mut checked) = (0, 0, 0);
    let mut highest = 0.0_f64;
    for step in 0..400 {
        let state = &mut level.map.map_context.view_state;
        assert!(state.drag_free_globe(center - cgmath::Vector2::new(0.0, 40.0), center));
        level.map.run_frame().expect("frame");
        let state = level.map.view_state();
        let data = crate::render::projection::projection_data_for_view(
            &level.map.map_context.style,
            state,
        )
        .expect("projection data");
        let matrix: [[f32; 4]; 4] = data.main_matrix;
        assert!(
            matrix.iter().flatten().all(|value| value.is_finite()),
            "step {step}: a non-finite projection"
        );
        let latitude = state.pose_view().expect("free camera").center.latitude;
        highest = highest.max(latitude);
        if latitude > MERCATOR_LATITUDE_LIMIT {
            assert_eq!(
                state.center_elevation(),
                0.0,
                "the cap holds the center at sea level"
            );
        }
        let checkpoint = step % 25 == 0 || (latitude > 89.5 && checked < 4);
        if checkpoint {
            // The cap holds the center at sea level; the tiles at their level ground.
            let ground = if latitude > MERCATOR_LATITUDE_LIMIT {
                0.0
            } else {
                scene.meters
            };
            let depth = level.settle(Some(ground)).await;
            let (tiles, caps) = check_frame(&level, &depth, &format!("step {step} at {latitude}"));
            seen_tiles += tiles;
            seen_caps += caps;
            checked += 1;
        }
        if highest > 89.0 && latitude < 83.0 {
            break;
        }
    }
    assert!(highest > 89.0, "the drag never reached the pole: {highest}");
    assert!(
        seen_tiles > 50 && seen_caps > 50,
        "{seen_tiles} tile and {seen_caps} cap pixels"
    );
}
