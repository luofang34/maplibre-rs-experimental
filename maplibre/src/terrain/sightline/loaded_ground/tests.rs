#![allow(clippy::expect_used, clippy::panic)]

use cgmath::Point2;
use image::{Rgba, RgbaImage};

use super::LoadedGround;
use crate::{
    coords::{WorldTileCoords, ZoomLevel},
    projection::body::Body,
    style::Style,
    terrain::{
        dem::DemTile,
        sightline::{
            synthetic::{camera, location},
            target_view, TargetAltitude, TerrainUnknown, Visibility,
        },
        source::{dem_source, DemSource},
    },
};

/// Terrarium encodes `R * 256 + G + B / 256 - 32768` metres: 1234 m and 2345.5 m.
const WEST: Rgba<u8> = Rgba([132, 210, 0, 255]);
const EAST: Rgba<u8> = Rgba([137, 41, 128, 255]);

fn style(maxzoom: u8) -> DemSource {
    let style: Style = serde_json::from_str(&format!(
        r#"{{"version":8,"sources":{{"dem":{{"type":"raster-dem","tiles":["https://dem.example/{{z}}/{{x}}/{{y}}.png"],"encoding":"terrarium","tileSize":256,"maxzoom":{maxzoom}}}}},"layers":[],"terrain":{{"source":"dem"}}}}"#
    ))
    .expect("style");
    dem_source(&style).expect("the style draws terrain")
}

fn tile(x: i32, y: i32, z: u8) -> WorldTileCoords {
    WorldTileCoords {
        x,
        y,
        z: ZoomLevel::new(z),
    }
}

/// The middle of the cell `(u, v)` in quarters of `coords`.
fn inside(coords: WorldTileCoords, u: f64, v: f64) -> crate::coords::LatLon {
    let scale = 2_f64.powi(i32::from(u8::from(coords.z)));
    location(Point2::new(
        (f64::from(coords.x) + u / 4.0) / scale,
        (f64::from(coords.y) + v / 4.0) / scale,
    ))
}

#[test]
fn the_dem_tiles_a_drawn_tile_samples_are_one_zoom_coarser_and_at_most_the_maxzoom() {
    let dem = style(12);
    assert_eq!(
        LoadedGround::dem_tiles_for([tile(1460, 852, 11), tile(1461, 853, 11)], &dem),
        vec![tile(730, 426, 10)]
    );
    assert_eq!(
        LoadedGround::dem_tiles_for([tile(23360, 13632, 15)], &dem),
        vec![tile(2920, 1704, 12)]
    );
}

#[test]
fn hand_encoded_terrarium_is_the_drawn_ground_and_missing_dem_is_unknown() {
    let dem = style(12);
    // Two drawn tiles share one DEM tile, its west half 1234 m, its east half 2345.5 m; a
    // third, beyond it, has no DEM.
    let west = tile(1460, 852, 11);
    let east = tile(1461, 852, 11);
    let bare = tile(1462, 852, 11);
    let wanted = LoadedGround::dem_tiles_for([west, east], &dem);
    assert_eq!(wanted, vec![tile(730, 426, 10)]);
    let image = RgbaImage::from_fn(256, 256, |x, _| if x < 128 { WEST } else { EAST });
    let decoded = DemTile::from_image(&image, dem.unpack).expect("decodes");
    let ground =
        LoadedGround::new([west, east, bare], [(wanted[0], decoded)], &dem).expect("valid");
    let terrain = ground.terrain(Body::EARTH);
    let at = inside(west, 1.5, 2.5);
    let camera = camera(at, 60.0, 0.0, 1234.0);
    for (location, meters) in [(at, 1234.0), (inside(east, 2.5, 1.5), 2345.5)] {
        let drawn = terrain.ground_at(location).expect("known").expect("drawn");
        assert!(
            (drawn - meters).abs() < 1e-3,
            "{drawn} m, encoded {meters} m"
        );
        let view = target_view(&camera, Some(terrain), location, TargetAltitude::OnGround);
        assert_eq!(view.elevation, Some(drawn));
    }
    let unloaded = inside(bare, 2.0, 2.0);
    assert_eq!(
        terrain.ground_at(unloaded),
        Err(TerrainUnknown { tile: bare })
    );
    let view = target_view(&camera, Some(terrain), unloaded, TargetAltitude::OnGround);
    assert_eq!(view.visibility, Visibility::TerrainUnknown);
    assert_eq!(view.elevation, None, "not sea level");
}

#[test]
fn a_tile_outside_the_pyramid_is_refused() {
    let dem = style(12);
    let outside = tile(5, 0, 2);
    assert!(LoadedGround::new([outside], [], &dem).is_err());
    let image = RgbaImage::from_pixel(4, 4, WEST);
    let decoded = DemTile::from_image(&image, dem.unpack).expect("decodes");
    assert!(LoadedGround::new([], [(outside, decoded)], &dem).is_err());
}
