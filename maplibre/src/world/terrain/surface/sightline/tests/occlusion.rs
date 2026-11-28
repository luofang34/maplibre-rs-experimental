//! A target's visibility follows the line from the eye to the target itself, through the body
//! and over the drawn ground.

use super::*;

#[test]
fn a_ridge_hides_the_ground_behind_it_but_not_a_point_high_above() {
    let ground = Ground::around(SCENE, ridge(12_000.0, 3000.0));
    let terrain = Some(ground.terrain());
    let camera = camera(SCENE, 85.0, 90.0, 0.0);
    let behind = offset(SCENE, 16_000.0, 0.0);
    let on_ground = target_view(&camera, terrain, behind, TargetAltitude::OnGround);
    assert_eq!(on_ground.visibility, Visibility::BehindTerrain);
    assert!(
        on_ground.pixel.is_some(),
        "a hidden target still has a pixel"
    );
    let above = target_view(
        &camera,
        terrain,
        behind,
        TargetAltitude::AboveGround { meters: 12_000.0 },
    );
    assert_eq!(above.visibility, Visibility::Visible);
    // The drawn triangles are chords a fraction of a millimetre under the sphere.
    assert!((above.elevation.expect("measured from loaded ground") - 12_000.0).abs() < 1e-2);
    // In front of the ridge the ground is in plain sight, its own ground no obstacle.
    let before = offset(SCENE, 8_000.0, 0.0);
    let near = target_view(&camera, terrain, before, TargetAltitude::OnGround);
    assert_eq!(near.visibility, Visibility::Visible);
}

#[test]
fn ground_beyond_the_horizon_is_behind_the_globe_but_a_point_high_over_it_is_not() {
    let ground = Ground::around(SCENE, |_| 0.0);
    let terrain = Some(ground.terrain());
    let camera = camera(SCENE, 85.0, 90.0, 0.0);
    let far = offset(SCENE, 800_000.0, 0.0);
    let low = target_view(&camera, terrain, far, TargetAltitude::Drawn { meters: 0.0 });
    assert_eq!(low.visibility, Visibility::BehindGlobe);
    let high = target_view(
        &camera,
        terrain,
        far,
        TargetAltitude::Drawn { meters: 100_000.0 },
    );
    assert_eq!(high.visibility, Visibility::Visible);
}

#[test]
fn ground_below_sea_level_is_seen_when_drawn_and_hidden_by_the_sea_level_globe_otherwise() {
    let ground = Ground::around(SCENE, |_| -400.0);
    let camera = camera(SCENE, 70.0, 324.037_142_967_012_36, -400.0);
    let near = offset(SCENE, 0.0, 3000.0);
    let drawn = target_view(
        &camera,
        Some(ground.terrain()),
        near,
        TargetAltitude::OnGround,
    );
    assert_eq!(drawn.visibility, Visibility::Visible);
    assert!((drawn.elevation.expect("loaded ground") + 400.0).abs() < 0.05);
    let bare = target_view(
        &camera,
        None,
        near,
        TargetAltitude::Drawn { meters: -400.0 },
    );
    assert_eq!(bare.visibility, Visibility::BehindGlobe);
}

#[test]
fn a_target_behind_the_camera_is_outside_the_frustum() {
    let ground = Ground::around(SCENE, hills);
    let camera = camera(SCENE, 70.0, 90.0, 1500.0);
    let behind = offset(SCENE, -200_000.0, 0.0);
    let view = target_view(
        &camera,
        Some(ground.terrain()),
        behind,
        TargetAltitude::Drawn { meters: 2000.0 },
    );
    assert_eq!(view.visibility, Visibility::OutsideFrustum);
}

#[test]
fn unknown_ground_on_the_line_or_under_the_target_is_unknown() {
    let rendered = block(SCENE, ZOOM, 4);
    let scale = 2_f64.powi(i32::from(DEM_ZOOM));
    let column = (lat_lon_to_mercator(SCENE).x * scale).floor() as i32;
    let loaded: Vec<WorldTileCoords> = block(SCENE, DEM_ZOOM, 3)
        .iter()
        .copied()
        .filter(|tile| tile.x != column)
        .collect();
    let ground = Ground::new(&rendered, &loaded, |_| 0.0);
    let terrain = Some(ground.terrain());
    let west = offset(SCENE, -25_000.0, 0.0);
    let camera = camera(west, 80.0, 90.0, 0.0);
    let on_unknown = target_view(&camera, terrain, SCENE, TargetAltitude::OnGround);
    assert_eq!(on_unknown.visibility, Visibility::TerrainUnknown);
    assert_eq!(on_unknown.elevation, None);
    // Beyond the unloaded column, low enough that unknown ground could stand in the way.
    let beyond = offset(SCENE, 25_000.0, 0.0);
    let past = target_view(&camera, terrain, beyond, TargetAltitude::OnGround);
    assert_eq!(past.visibility, Visibility::TerrainUnknown);
}

#[test]
fn drawn_heights_carry_the_exaggeration_once() {
    let ground = Ground::around(SCENE, |_| 1000.0).exaggerated(2.0);
    let terrain = Some(ground.terrain());
    let camera = camera(SCENE, 70.0, 324.037_142_967_012_36, 2000.0);
    let near = offset(SCENE, 0.0, 2000.0);
    let on_ground = target_view(&camera, terrain, near, TargetAltitude::OnGround);
    assert!((on_ground.elevation.expect("loaded") - 2000.0).abs() < 0.05);
    assert_eq!(on_ground.visibility, Visibility::Visible);
    // A drawn height is already exaggerated: 1500 drawn metres lie under the drawn ground.
    let under = target_view(
        &camera,
        terrain,
        near,
        TargetAltitude::Drawn { meters: 1500.0 },
    );
    assert_eq!(under.visibility, Visibility::BehindTerrain);
    let above = target_view(
        &camera,
        terrain,
        near,
        TargetAltitude::AboveGround { meters: 10.0 },
    );
    assert!((above.elevation.expect("loaded") - 2010.0).abs() < 0.05);
    assert_eq!(above.visibility, Visibility::Visible);
    let TerrainPick::Ground(hit) =
        pick_globe_terrain(&camera, ground.terrain(), Point2::new(1165.0, 900.0))
    else {
        panic!("the center meets the ground");
    };
    assert!((hit.elevation - 2000.0).abs() < 0.05);
}

#[test]
fn a_target_ahead_beyond_the_far_plane_is_outside_the_frustum() {
    let ground = Ground::around(SCENE, hills);
    let camera = camera(SCENE, 0.0, 0.0, 1500.0);
    // Straight ahead through the globe, past its far side, where only depth rules it out.
    let antipode = LatLon::new(-SCENE.latitude, SCENE.longitude - 180.0);
    let beyond = target_view(
        &camera,
        Some(ground.terrain()),
        antipode,
        TargetAltitude::Drawn {
            meters: 0.5 * Body::EARTH.radius_meters,
        },
    );
    let pixel = beyond.pixel.expect("ahead of the camera");
    assert!((pixel - Point2::new(1165.0, 900.0)).magnitude() < 1.0);
    assert_eq!(beyond.visibility, Visibility::OutsideFrustum);
}

#[test]
fn a_line_into_the_body_over_ground_without_dem_is_unknown() {
    // Coarse tiles over 2000 km, the DEM loaded only around a target three tiles east.
    let rendered = block(SCENE, 7, 7);
    let scale = 2_f64.powi(6);
    let scene = lat_lon_to_mercator(SCENE);
    let column = (scene.x * scale).floor();
    let far = location(Point2::new((column + 2.5) / scale, scene.y));
    let loaded: Vec<WorldTileCoords> = block(far, 6, 0);
    let ground = Ground::new(&rendered, &loaded, |_| -400.0);
    let camera = camera(SCENE, 85.0, 90.0, 0.0);
    let view = target_view(
        &camera,
        Some(ground.terrain()),
        far,
        TargetAltitude::OnGround,
    );
    assert!(
        view.elevation.is_some(),
        "the target's own ground is loaded"
    );
    assert_eq!(view.visibility, Visibility::TerrainUnknown);
}

#[test]
fn before_anything_is_drawn_a_line_below_the_highest_ground_is_unknown() {
    let empty = Ground::new(&[], &[], |_| 0.0);
    let camera = camera(SCENE, 70.0, 0.0, 0.0);
    let near = offset(SCENE, 0.0, 2000.0);
    let low = target_view(
        &camera,
        Some(empty.terrain()),
        near,
        TargetAltitude::Drawn { meters: 0.0 },
    );
    assert_eq!(low.visibility, Visibility::TerrainUnknown);
    let high_camera = super::camera(SCENE, 0.0, 0.0, 30_000.0);
    let high = target_view(
        &high_camera,
        Some(empty.terrain()),
        SCENE,
        TargetAltitude::Drawn { meters: 30_000.0 },
    );
    assert_eq!(high.visibility, Visibility::Visible);
}

#[test]
fn a_depression_below_sea_level_amid_higher_land_is_seen_from_above_and_hidden_by_its_rim() {
    let floor = lat_lon_to_mercator(SCENE);
    let radius = 4000.0 / (Body::EARTH.circumference_meters() * SCENE.latitude.to_radians().cos());
    let ground = Ground::around(SCENE, move |mercator| {
        if (mercator - floor).magnitude() < radius {
            -300.0
        } else {
            1200.0
        }
    });
    let terrain = Some(ground.terrain());
    let above = camera(SCENE, 0.0, 0.0, -300.0);
    let seen = target_view(&above, terrain, SCENE, TargetAltitude::OnGround);
    assert_eq!(seen.visibility, Visibility::Visible);
    assert!((seen.elevation.expect("loaded") + 300.0).abs() < 0.05);
    // From low beside it, the rim hides the floor; the body does not.
    let beside = camera(offset(SCENE, -9000.0, 0.0), 85.0, 90.0, 1200.0);
    let hidden = target_view(&beside, terrain, SCENE, TargetAltitude::OnGround);
    assert_eq!(hidden.visibility, Visibility::BehindTerrain);
}
