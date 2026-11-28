#![allow(clippy::expect_used, clippy::panic)]

use cgmath::{InnerSpace, Point2};

pub(super) use super::synthetic::*;
use super::{
    lat_lon_to_mercator, pick_globe_terrain, target_view, TargetAltitude, TerrainPick,
    TerrainUnknown, Visibility,
};
use crate::{
    coords::{LatLon, WorldTileCoords},
    projection::body::Body,
};

mod boundaries;
mod occlusion;
mod picking;

#[test]
fn level_ground_is_met_at_its_height_under_the_center() {
    for meters in [-400.0, 0.0, 4000.0] {
        let ground = Ground::around(SCENE, |_| meters);
        let terrain = ground.terrain();
        let found = terrain
            .ground_at(SCENE)
            .ok()
            .flatten()
            .expect("loaded ground");
        assert!(
            (found - meters).abs() < 0.05,
            "{meters} m: ground at {found} m"
        );
        for pitch in [0.0, 70.0, 85.0] {
            let camera = camera(SCENE, pitch, 324.037_142_967_012_36, meters);
            let TerrainPick::Ground(hit) =
                pick_globe_terrain(&camera, terrain, Point2::new(1165.0, 900.0))
            else {
                panic!("{meters} m, pitch {pitch}: the center pixel misses the ground");
            };
            let expected = lat_lon_to_mercator(SCENE);
            assert!(
                (hit.elevation - meters).abs() < 0.05,
                "{meters} m, pitch {pitch}: hit at {} m",
                hit.elevation
            );
            assert!(
                (hit.mercator - expected).magnitude() < 1e-9,
                "{meters} m, pitch {pitch}: hit at {:?}, center at {expected:?}",
                hit.mercator
            );
        }
    }
}

#[test]
fn a_target_on_the_ground_stands_at_the_drawn_ground_and_unknown_ground_is_unknown() {
    let ground = Ground::around(SCENE, hills);
    let terrain = ground.terrain();
    let camera = camera(SCENE, 60.0, 324.037_142_967_012_36, 1500.0);
    for (east, north) in [(0.0, 0.0), (900.0, -300.0), (-2500.0, 1700.0)] {
        let location = offset(SCENE, east, north);
        let drawn = terrain.ground_at(location).expect("known").expect("drawn");
        let view = target_view(&camera, Some(terrain), location, TargetAltitude::OnGround);
        assert_eq!(
            view.elevation,
            Some(drawn),
            "{east} m east, {north} m north"
        );
    }
    // A rendered tile whose DEM has not loaded is unknown ground, for both.
    let scene = lat_lon_to_mercator(SCENE);
    let rendered = [WorldTileCoords {
        x: (scene.x * f64::from(1 << ZOOM)).floor() as i32,
        y: (scene.y * f64::from(1 << ZOOM)).floor() as i32,
        z: crate::coords::ZoomLevel::new(ZOOM),
    }];
    let bare = Ground::new(&rendered, &[], hills);
    let terrain = bare.terrain();
    assert_eq!(
        terrain.ground_at(SCENE),
        Err(TerrainUnknown { tile: rendered[0] })
    );
    let view = target_view(&camera, Some(terrain), SCENE, TargetAltitude::OnGround);
    assert_eq!(view.visibility, Visibility::TerrainUnknown);
    assert_eq!(view.elevation, None);
}
