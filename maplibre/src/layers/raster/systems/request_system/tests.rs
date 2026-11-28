#![allow(clippy::expect_used)]

use super::*;
use crate::{
    raster::{AvailableRasterLayerData, MissingRasterLayerData, RasterLayerData},
    style::Style,
    tcs::world::World,
};

fn source_results(missing: &str, available: &str) -> (Style, World, WorldTileCoords) {
    let style = serde_json::from_value(serde_json::json!({
        "version":8,
        "sources":{
            "a":{"type":"raster","tiles":["offline://a"],"minzoom":12},
            "b":{"type":"raster","tiles":["offline://b"],"minzoom":10}
        },
        "layers":[{"id":"a","type":"raster","source":"a"},
            {"id":"b","type":"raster","source":"b"}]
    }))
    .expect("style");
    let coords = WorldTileCoords::from((2423, 1389, 12_u8.into()));
    let mut world = World::default();
    let mut data = RasterLayersDataComponent::default();
    data.record(RasterLayerData::Available(AvailableRasterLayerData {
        coords,
        source: available.into(),
        image: image::RgbaImage::new(1, 1),
    }));
    data.record(RasterLayerData::Missing(MissingRasterLayerData {
        coords,
        source: missing.into(),
    }));
    world.tiles.spawn_mut(coords).expect("tile").insert(data);
    (style, world, coords)
}

#[test]
fn another_sources_image_and_minzoom_do_not_suppress_missing_source_fallback() {
    let (style, world, coords) = source_results("b", "a");
    assert_eq!(
        wanted_tiles(
            vec![("a".into(), vec![coords]), ("b".into(), vec![coords])],
            Vec::new(),
            &style,
            &world
        ),
        vec![coords, coords, coords.get_parent().expect("parent")],
    );
}

#[test]
fn another_sources_lower_minzoom_does_not_extend_missing_source_fallback() {
    let (style, world, coords) = source_results("a", "b");
    assert_eq!(
        wanted_tiles(
            vec![("a".into(), vec![coords]), ("b".into(), vec![coords])],
            Vec::new(),
            &style,
            &world
        ),
        vec![coords, coords],
    );
}

/// An eye in flight never finishes loading the detail under a terrain tile, so the tile that
/// fills the whole terrain tile must not queue behind that detail.
#[test]
fn a_drape_targets_own_tile_is_requested_before_the_detail_under_it() {
    let (style, world, coords) = source_results("a", "b");
    let target = coords.get_parent().expect("parent");
    assert_eq!(
        wanted_tiles(
            vec![("a".into(), vec![coords]), ("b".into(), vec![coords])],
            vec![target],
            &style,
            &world
        ),
        vec![target, coords, coords],
        "once, from the source whose zoom range holds it, ahead of every wanted tile"
    );
}

#[test]
fn a_stand_in_stays_within_the_levels_its_source_serves() {
    let target = WorldTileCoords::from((9692, 5556, 14_u8.into()));
    assert_eq!(stand_in_tile(target, 0, 18), Some(target));
    assert_eq!(
        stand_in_tile(target, 0, 12),
        Some(WorldTileCoords::from((2423, 1389, 12_u8.into()))),
        "a source that stops above the terrain tile fills it from its deepest level"
    );
    assert_eq!(stand_in_tile(target, 15, 18), None);
}
