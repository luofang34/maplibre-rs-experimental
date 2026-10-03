//! Zooming the Himalayan view across the levels where the terrain changes detail keeps the
//! camera and the drawn ground in agreement, from the whole range down to street level.

use super::*;
use crate::coords::Zoom;

/// The target stands on the ground the DEM of the integer zoom describes, as GL JS lifts it;
/// the tile drawn under the center may sample a finer DEM, a few metres apart.
fn check_target_on_ground(
    level: &LevelMap,
    terrain: crate::terrain::sightline::DrawnTerrain<'_>,
    center: crate::coords::LatLon,
    scene: Scene,
) {
    let target = level.map.view_state().center_elevation();
    let drawn = terrain.ground_at(center).expect("ground at the center");
    assert!(
        (drawn - target).abs() < 50.0,
        "{scene:?}: the target stands at {target} m over ground drawn at {drawn} m"
    );
}

/// The settled frame's ground: the center lies on it, and every sampled pixel shows ground
/// exactly where a pick finds it, the frame leaving none empty that a pick meets.
fn check_settled(level: &LevelMap, depth: &[f32], scene: Scene) {
    use crate::terrain::{sightline::DrawnTerrain, TerrainCoverageIndex};
    let camera = globe_camera_for_view(level.map.view_state()).expect("camera");
    let (near, far) = camera.depth_range();
    let world = level.map.world();
    let terrain = DrawnTerrain {
        index: world
            .resources
            .get::<TerrainCoverageIndex>()
            .expect("coverage index"),
        tiles: &world.tiles,
        body: camera.body(),
    };
    let stored = |x: u32, y: u32| {
        let value = depth[(y * WIDTH + x) as usize];
        (value > 0.0).then(|| near * far / (f64::from(value) * (far - near) + near))
    };
    check_target_on_ground(level, terrain, camera.center(), scene);
    let samples = [
        (0.375, 0.125),
        (0.875, 0.375),
        (0.125, 0.625),
        (0.625, 0.875),
    ];
    // Whether pixel (x, y) shows ground where its samples pick it, checked.
    let matches = |x: u32, y: u32| {
        let picks: Vec<Option<f64>> = samples
            .iter()
            .map(|(dx, dy)| {
                let pixel = Point2::new(f64::from(x) + dx, f64::from(y) + dy);
                picked_distance(&camera, terrain, pixel).expect("settled DEM")
            })
            .collect();
        let nearest = picks.iter().flatten().copied().reduce(f64::min);
        let farthest = picks.iter().flatten().copied().reduce(f64::max);
        match (nearest.zip(farthest), stored(x, y)) {
            // Where a crest's silhouette crosses the pixel, which of its samples the GPU
            // covers turns on the last bit of a vertex; the stored depth then lies between
            // the ground the samples pick.
            (Some((nearest, farthest)), Some(drawn)) if farthest > nearest * 1.01 => {
                assert!(
                    drawn > nearest * (1.0 - 2e-3) && drawn < farthest * (1.0 + 2e-3),
                    "{scene:?}: ({x},{y}) drawn {drawn} px, picks {nearest}..{farthest} px"
                );
                true
            }
            (Some((nearest, _)), Some(drawn)) => {
                assert!(
                    (drawn / nearest - 1.0).abs() < 2e-3,
                    "{scene:?}: ({x},{y}) picks ground {nearest} px away, drawn {drawn} px"
                );
                true
            }
            (Some(_), None) => panic!("{scene:?}: ({x},{y}) picks ground the frame left empty"),
            (None, _) => false,
        }
    };
    // The viewport's center is the corner of these four pixels.
    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
        assert!(
            matches(WIDTH / 2 - dx, HEIGHT / 2 - dy),
            "{scene:?}: no ground at the center"
        );
    }
    let mut picked = 0;
    for y in (0..HEIGHT).step_by(180) {
        for x in (0..WIDTH).step_by(180) {
            picked += usize::from(matches(x, y));
        }
    }
    assert!(picked > 20, "{scene:?}: {picked} pixels picked ground");
}

#[tokio::test]
async fn zooming_through_the_terrain_s_levels_of_detail_keeps_camera_and_ground_together() {
    for pitch in [70.0, 85.0] {
        let mut scene = Scene {
            meters: 1500.0,
            relief: 600.0,
            exaggeration: 1.0,
            pitch,
            zoom: 10.8,
        };
        let mut level = LevelMap::new(scene).await;
        let settled_at = [10.8, 11.2, 11.5, 11.67, 11.9, 12.2];
        let mut zoom: f64 = 10.8;
        while zoom <= 12.2 + 1e-9 {
            // Every frame of the continuous zoom has a finite camera and projection.
            level.map.map_context.view_state.zoom_to(Zoom::new(zoom));
            level.map.run_frame().expect("frame");
            let state = level.map.view_state();
            let camera = globe_camera_for_view(state).expect("camera");
            let matrix = camera.wgpu_view_projection();
            let columns: [[f64; 4]; 4] = matrix.into();
            assert!(
                columns.iter().flatten().all(|value| value.is_finite()),
                "pitch {pitch}, zoom {zoom}: a non-finite view-projection"
            );
            let data = crate::render::projection::projection_data_for_view(
                &level.map.map_context.style,
                state,
            )
            .expect("projection data");
            assert!(data.center_clip_w.is_finite() && data.center_clip_w > 0.0);
            if settled_at.iter().any(|at| (at - zoom).abs() < 1e-6) {
                scene.zoom = zoom;
                let depth = level.settle(None).await;
                check_settled(&level, &depth, scene);
            }
            zoom = (zoom * 100.0 + 2.0).round() / 100.0;
            if (zoom - 11.68).abs() < 1e-9 {
                zoom = 11.67;
            } else if (zoom - 11.69).abs() < 1e-9 {
                zoom = 11.7;
            }
        }
    }
}

#[tokio::test]
async fn street_level_ground_meets_the_camera_on_the_tile_relative_path() {
    for zoom in [17.0, 18.5] {
        let scene = Scene {
            meters: 1500.0,
            relief: 600.0,
            exaggeration: 1.0,
            pitch: 70.0,
            zoom,
        };
        let mut level = LevelMap::new(scene).await;
        let depth = level.settle(None).await;
        check_settled(&level, &depth, scene);
    }
}
