//! Transparent source pixels composed directly and through terrain drapes.
use super::*;
use crate::{
    headless::{create_headless_renderer_with_settings, map::process_tile_layers},
    render::settings::{Msaa, RendererSettings},
    vector::{DefaultVectorTransferables, VectorPlugin},
};
use geozero::mvt::Message;

fn alpha_style(terrain: bool, fill_alpha: f32, raster_alpha: Option<u8>) -> Style {
    let mut style = coverage_style(false, false);
    style.terrain = terrain.then_some(style.terrain.take().expect("terrain"));
    style.sources.insert(
        "vector".into(),
        serde_json::from_value(serde_json::json!({
            "type":"vector", "tiles":["offline://vector"], "maxzoom":14
        }))
        .expect("vector source"),
    );
    style.layers = vec![serde_json::from_value(serde_json::json!({
        "id":"fill", "type":"fill", "source":"vector", "source-layer":"fill",
        "paint":{"fill-color":"#0000ff", "fill-opacity":fill_alpha}
    }))
    .expect("fill")];
    if raster_alpha.is_some() {
        let mut raster: crate::style::layer::StyleLayer =
            serde_json::from_value(serde_json::json!({
                "id":"paint", "type":"raster", "source":"paint"
            }))
            .expect("raster");
        raster.index = 1;
        style.layers.push(raster);
    }
    style
}

fn fill_source(style: &Style, coords: WorldTileCoords) -> ProcessedLayers {
    let bytes = geozero::mvt::Tile {
        layers: vec![geozero::mvt::tile::Layer {
            name: "fill".into(),
            version: 2,
            extent: Some(4096),
            features: vec![geozero::mvt::tile::Feature {
                r#type: Some(3),
                geometry: vec![9, 0, 0, 26, 8192, 0, 0, 8192, 8191, 0, 15],
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
    .encode_to_vec();
    let layer = style
        .layers
        .iter()
        .find(|layer| layer.id == "fill")
        .expect("fill layer");
    let projection = style
        .projection
        .as_ref()
        .map(|value| value.projection_type.clone())
        .unwrap_or_default();
    process_tile_layers(&bytes, layer, coords, projection).expect("fill geometry")
}

async fn alpha_map(
    terrain: bool,
    samples: u32,
    fill_alpha: f32,
    raster_alpha: Option<u8>,
) -> HeadlessMap {
    styled_alpha_map(
        alpha_style(terrain, fill_alpha, raster_alpha),
        samples,
        raster_alpha,
        true,
    )
    .await
}

async fn styled_alpha_map(
    style: Style,
    samples: u32,
    raster_alpha: Option<u8>,
    load_fill: bool,
) -> HeadlessMap {
    styled_alpha_map_at(style, samples, raster_alpha, load_fill, target()).await
}

async fn styled_alpha_map_at(
    style: Style,
    samples: u32,
    raster_alpha: Option<u8>,
    load_fill: bool,
    coords: WorldTileCoords,
) -> HeadlessMap {
    let processed = if load_fill {
        fill_source(&style, coords)
    } else {
        ProcessedLayers::default()
    };
    let settings = RendererSettings {
        msaa: Msaa { samples },
        ..Default::default()
    };
    let (kernel, renderer) = create_headless_renderer_with_settings(SIZE, SIZE, None, settings)
        .await
        .expect("renderer");
    let mut map = HeadlessMap::new(
        style,
        renderer,
        kernel,
        vec![
            Box::new(RenderPlugin),
            Box::new(crate::background::BackgroundPlugin),
            Box::new(VectorPlugin::<DefaultVectorTransferables>::default()),
            Box::new(RasterPlugin::<DefaultRasterTransferables>::default()),
            Box::new(TerrainPlugin::<DefaultDemTransferables>::default()),
            Box::new(
                HeadlessPlugin::new(false)
                    .preserve_tile_sources()
                    .retain_supplied_tiles(),
            ),
        ],
    )
    .expect("map");
    let raster = raster_alpha
        .map(|alpha| AvailableRasterLayerData {
            coords,
            source: "paint".into(),
            image: RgbaImage::from_pixel(256, 256, Rgba([255, 0, 0, alpha])),
        })
        .into_iter()
        .collect();
    map.render_frames_with_terrain(
        processed,
        raster,
        vec![(
            coords,
            RgbaImage::from_pixel(16, 16, Rgba([128, 0, 0, 255])),
        )],
        3,
    )
    .expect("alpha frame");
    let crate::render::eventually::Eventually::Initialized(depth) =
        &map.map_context.renderer.resources.depth_texture
    else {
        panic!("depth texture");
    };
    assert_eq!(
        depth.texture.sample_count(),
        samples,
        "exercise requested GPU sample count"
    );
    map
}

fn pixels_blocking(map: &HeadlessMap, case: &str) -> Vec<u8> {
    let terrain = map.map_context.style.terrain.is_some();
    let samples = map.map_context.renderer.settings.msaa.samples;
    read_blocking(map, &format!("alpha-{case}-terrain{terrain}-msaa{samples}"))
}

fn with_background(mut style: Style, color: &str) -> Style {
    let background = serde_json::from_value(serde_json::json!({
        "id":"background", "type":"background",
        "paint":{"background-color":color}
    }))
    .expect("background");
    style.layers.insert(0, background);
    for (index, layer) in style.layers.iter_mut().enumerate() {
        layer.index = index as u32;
    }
    style
}

fn assert_center(bytes: &[u8], expected: [u8; 4]) {
    for y in SIZE / 2 - 32..SIZE / 2 + 32 {
        for x in SIZE / 2 - 32..SIZE / 2 + 32 {
            let start = ((y * SIZE + x) * 4) as usize;
            let pixel = &bytes[start..start + 4];
            assert!(
                pixel.iter().zip(expected).all(|(a, b)| a.abs_diff(b) <= 2),
                "center ({x},{y}): {pixel:?}, expected {expected:?}"
            );
        }
    }
}

mod tests;

mod globe;
