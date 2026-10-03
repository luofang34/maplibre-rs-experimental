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
