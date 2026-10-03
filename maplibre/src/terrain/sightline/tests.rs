#![allow(clippy::expect_used, clippy::panic)]

use cgmath::{InnerSpace, Point2};

pub(super) use super::synthetic::*;
use super::{
    lat_lon_to_mercator, pick_globe_terrain, target_view, TargetAltitude, TerrainPick, Visibility,
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
        let found = terrain.ground_at(SCENE).expect("loaded ground");
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
