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
            &style,
            &world
        ),
        vec![coords, coords],
    );
}
