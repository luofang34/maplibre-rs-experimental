//! Terrain pixels with overlapping source pyramids and buffered vector geometry.
use super::*;
use crate::{
    headless::map::process_tile_layers,
    projection::ProjectionType,
    style::layer::StyleLayer,
    vector::{DefaultVectorTransferables, VectorPlugin},
};
use geozero::mvt::Message;

fn vector_tile(name: &str, geometry: Vec<u32>, kind: i32) -> Vec<u8> {
    geozero::mvt::Tile {
        layers: vec![geozero::mvt::tile::Layer {
            name: name.into(),
            version: 2,
            extent: Some(4096),
            features: vec![geozero::mvt::tile::Feature {
                r#type: Some(kind),
                geometry,
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
    .encode_to_vec()
}

fn polygon(name: &str, width: u32) -> Vec<u8> {
    vector_tile(
        name,
        vec![9, 0, 0, 26, width * 2, 0, 0, 8192, width * 2 - 1, 0, 15],
        3,
    )
}

fn water_style() -> Style {
    let mut style = coverage_style(true, false);
    style.sources.insert(
        "water".into(),
        serde_json::from_value(serde_json::json!({
            "type":"vector", "tiles":["offline://water"], "maxzoom":14
        }))
        .expect("source"),
    );
    let mut layer: StyleLayer = serde_json::from_value(serde_json::json!({
        "id":"water", "type":"fill", "source":"water", "source-layer":"water",
        "paint":{"fill-color":"#0000ff"}
    }))
    .expect("layer");
    layer.index = 2;
    style.layers.push(layer);
    style
}

fn process(bytes: &[u8], layer: &StyleLayer, coords: WorldTileCoords) -> ProcessedLayers {
    let layers = process_tile_layers(bytes, layer, coords, ProjectionType::Mercator)
        .expect("vector geometry");
    assert!(!layers.vector.is_empty());
    layers
}

async fn map_with(style: Style, processed: ProcessedLayers) -> HeadlessMap {
    let (kernel, renderer) = create_headless_renderer(SIZE, SIZE, None)
        .await
        .expect("renderer");
    let mut map = HeadlessMap::new(
        style,
        renderer,
        kernel,
        vec![
            Box::new(RenderPlugin),
            Box::new(VectorPlugin::<DefaultVectorTransferables>::default()),
            Box::new(RasterPlugin::<DefaultRasterTransferables>::default()),
            Box::new(HillshadePlugin),
            Box::new(TerrainPlugin::<DefaultDemTransferables>::default()),
            Box::new(
                HeadlessPlugin::new(false)
                    .preserve_tile_sources()
                    .retain_supplied_tiles(),
            ),
        ],
    )
    .expect("map");
    map.render_frames_with_terrain(
        processed,
        vec![tile(target(), true, false)],
        vec![(
            target(),
            RgbaImage::from_pixel(256, 256, Rgba([128, 0, 0, 255])),
        )],
        16,
    )
    .expect("parent frame");
    map
}

async fn water_map() -> HeadlessMap {
    let style = water_style();
    let processed = process(&polygon("water", 4096), &style.layers[2], target());
    map_with(style, processed).await
}

fn assert_pixel(bytes: &[u8], x: u32, y: u32, color: [u8; 4]) {
    let offset = ((y * SIZE + x) * 4) as usize;
    let pixel = &bytes[offset..offset + 4];
    assert!(
        pixel.iter().zip(color).all(|(a, b)| a.abs_diff(b) <= 2),
        "pixel ({x},{y}) {pixel:?}, expected {color:?}"
    );
}

mod tests;
