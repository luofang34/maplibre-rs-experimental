//! Picking returns the first drawn ground along a pixel's ray, and says when there is none or
//! when ground no DEM describes comes first.

use super::*;

#[test]
fn a_picked_point_projects_back_to_its_pixel_and_is_visible() {
    let antimeridian = LatLon::new(-16.5, 179.995);
    for center in [SCENE, antimeridian] {
        let ground = Ground::around(center, hills);
        let terrain = ground.terrain();
        let elevation = terrain.ground_at(center).expect("loaded ground");
        for (pitch, bearing) in [(0.0, 0.0), (70.0, 324.037_142_967_012_36), (85.0, 90.0)] {
            let camera = camera(center, pitch, bearing, elevation);
            let mut picked = 0;
            for y in (0..1800).step_by(97) {
                for x in (0..2330).step_by(97) {
                    let pixel = Point2::new(f64::from(x) + 0.5, f64::from(y) + 0.5);
                    let TerrainPick::Ground(hit) = pick_globe_terrain(&camera, terrain, pixel)
                    else {
                        continue;
                    };
                    picked += 1;
                    let view = target_view(
                        &camera,
                        Some(terrain),
                        location(hit.mercator),
                        TargetAltitude::Drawn {
                            meters: hit.elevation,
                        },
                    );
                    let back = view.pixel.expect("in front of the camera");
                    assert!(
                        (back - pixel).magnitude() < 1e-3,
                        "{center:?} pitch {pitch}: {pixel:?} picks ground that projects to {back:?}"
                    );
                    assert_eq!(
                        view.visibility,
                        Visibility::Visible,
                        "{center:?} pitch {pitch}: the ground picked at {pixel:?} is hidden"
                    );
                }
            }
            assert!(
                picked > 100,
                "{center:?} pitch {pitch}: {picked} pixels pick ground"
            );
        }
    }
}

#[test]
fn a_grazing_ray_meets_a_narrow_ridge_rather_than_passing_through_it() {
    let ground = Ground::around(SCENE, ridge(12_000.0, 3000.0));
    let terrain = ground.terrain();
    let camera = camera(SCENE, 85.0, 90.0, 0.0);
    // Points just under the crest, seen at a few degrees from the eye kilometres above.
    for (north, below) in [(0.0, 30.0), (-4000.0, 200.0), (3000.0, 5.0)] {
        let under_crest = offset(SCENE, 12_000.0, north);
        let aim = target_view(
            &camera,
            None,
            under_crest,
            TargetAltitude::Drawn {
                meters: 3000.0 - below,
            },
        );
        let pixel = aim.pixel.expect("the ridge is in front of the camera");
        let TerrainPick::Ground(hit) = pick_globe_terrain(&camera, terrain, pixel) else {
            panic!("the ray through {pixel:?} passes the ridge {below} m under its crest");
        };
        let east = (location(hit.mercator).longitude - SCENE.longitude).to_radians()
            * Body::EARTH.radius_meters
            * SCENE.latitude.to_radians().cos();
        assert!(
            (east - 12_000.0).abs() < 600.0 && hit.elevation > 0.0,
            "the ray meets ground {east} m east at {} m, not the ridge",
            hit.elevation
        );
    }
}

#[test]
fn ground_drawn_from_a_parent_dem_is_picked_and_moves_when_the_tile_s_own_dem_loads() {
    let rendered = block(SCENE, ZOOM, 4);
    let parents: Vec<WorldTileCoords> = block(SCENE, DEM_ZOOM - 2, 1);
    let parent_only = Ground::new(&rendered, &parents, |_| 1000.0);
    let camera = camera(SCENE, 70.0, 324.037_142_967_012_36, 1000.0);
    let center = Point2::new(1165.0, 900.0);
    let TerrainPick::Ground(first) = pick_globe_terrain(&camera, parent_only.terrain(), center)
    else {
        panic!("the parent's DEM draws the ground");
    };
    assert!((first.elevation - 1000.0).abs() < 0.05);
    let mut loaded = parents.clone();
    loaded.extend(block(SCENE, DEM_ZOOM, 3));
    let replaced = Ground::new(&rendered, &loaded, |_| 1500.0);
    let TerrainPick::Ground(second) = pick_globe_terrain(&camera, replaced.terrain(), center)
    else {
        panic!("the tile's own DEM draws the ground");
    };
    assert!((second.elevation - 1500.0).abs() < 0.05);
    assert!(
        (second.mercator - first.mercator).magnitude() > 1e-6,
        "the higher ground meets the ray nearer the eye"
    );
}

#[test]
fn ground_without_any_dem_is_unknown_not_sea_level() {
    let rendered = block(SCENE, ZOOM, 4);
    // The DEM has loaded up to the scene's column and not east of it.
    let scale = 2_f64.powi(i32::from(DEM_ZOOM));
    let scene = lat_lon_to_mercator(SCENE);
    let column = (scene.x * scale).floor() as i32;
    let loaded: Vec<WorldTileCoords> = block(SCENE, DEM_ZOOM, 3)
        .into_iter()
        .filter(|tile| tile.x <= column)
        .collect();
    let ground = Ground::new(&rendered, &loaded, |_| 200.0);
    let terrain = ground.terrain();
    let unloaded = location(Point2::new((f64::from(column) + 1.5) / scale, scene.y));
    assert_eq!(terrain.ground_at(unloaded), None);
    // Looking east past the scene, the ray runs low over the unloaded column before it meets
    // any ground.
    let camera = camera(SCENE, 80.0, 90.0, 200.0);
    let pick = pick_globe_terrain(&camera, terrain, Point2::new(1165.0, 600.0));
    assert_eq!(pick, TerrainPick::Unknown);
    // Looking down on loaded ground west of it, the ray meets that ground.
    let west = offset(SCENE, -40_000.0, 0.0);
    let camera = super::camera(west, 20.0, 270.0, 200.0);
    let pick = pick_globe_terrain(&camera, terrain, Point2::new(1165.0, 900.0));
    assert!(matches!(pick, TerrainPick::Ground(_)), "{pick:?}");
}

#[test]
fn rays_into_the_sky_and_onto_a_polar_cap_say_so() {
    let ground = Ground::around(SCENE, hills);
    let camera = camera(SCENE, 85.0, 0.0, 1500.0);
    assert_eq!(
        pick_globe_terrain(&camera, ground.terrain(), Point2::new(1165.0, 2.0)),
        TerrainPick::Sky
    );
    // Close enough to the last row of tiles that the rendered block includes it.
    let north = LatLon::new(85.03, 20.0);
    let polar = Ground::around(north, |_| 300.0);
    let camera = super::camera(north, 60.0, 0.0, 300.0);
    let beyond = target_view(
        &camera,
        None,
        LatLon::new(85.3, 20.0),
        TargetAltitude::Drawn { meters: 0.0 },
    );
    let pick = pick_globe_terrain(
        &camera,
        polar.terrain(),
        beyond.pixel.expect("the cap is in front of the camera"),
    );
    let TerrainPick::PolarCap(hit) = pick else {
        panic!("the ray beyond the last row of tiles meets {pick:?}");
    };
    // The fan's flat triangles span the 550 km from the last row to the pole, chords that sag
    // up to 6 km under the sphere.
    assert!(
        hit.elevation < 300.0 && hit.elevation > -6000.0,
        "the cap runs from the last row at 300 m to the pole under the sphere: {} m",
        hit.elevation
    );
}
