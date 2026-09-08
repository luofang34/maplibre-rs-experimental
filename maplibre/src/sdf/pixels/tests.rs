#![allow(clippy::expect_used, clippy::panic)]
use crate::{
    coords::{WorldTileCoords, ZoomLevel},
    headless::{
        create_headless_renderer_with_settings,
        map::{process_tile_layers, HeadlessMap},
    },
    render::RenderPlugin,
    sdf::{
        assets::{AtlasBuilder, AtlasEntry},
        tessellation_new::TextTessellatorNew,
        SdfPlugin,
    },
    style::{layer::LayerPaint, Style},
    vector::{
        transferables::DefaultSymbolLayerTessellated, DefaultVectorTransferables,
        SymbolLayerTessellated, VectorPlugin,
    },
};
use geozero::{mvt::Message, FeatureProcessor, GeomProcessor};
use std::sync::Arc;
const SIZE: u32 = 512;

fn style(offset: f32, anchor: &str) -> Style {
    serde_json::from_value(serde_json::json!({
        "version":8, "center":[0.0439453125,-0.04394530819], "zoom":12, "pitch":55,
        "sources":{
            "map":{"type":"vector","tiles":["https://unused.invalid/{z}/{x}/{y}"],"maxzoom":12},
            "dem":{"type":"raster-dem","tiles":["https://unused.invalid/dem/{z}/{x}/{y}"],"encoding":"terrarium","maxzoom":0}
        },
        "terrain":{"source":"dem","exaggeration":1},
        "layers":[
            {"id":"background","type":"background","paint":{"background-color":"#334455"}},
            {"id":"point","type":"circle","source":"map","source-layer":"places","paint":{"circle-radius":0.1}},
            {"id":"label","type":"symbol","source":"map","source-layer":"places",
                "layout":{"text-field":"Alps","text-size":28,"text-height-offset":offset,
                    "text-height-anchor":anchor,"text-allow-overlap":true,
                    "icon-image":"marker","icon-size":1,"icon-offset":[55,0]},
                "paint":{"text-color":"#ff0000","text-halo-color":"#ffffff","text-halo-width":1}}
        ]
    })).expect("symbol style")
}

async fn render(offset: f32, anchor: &str, zoom: f64, samples: u32) -> Vec<u8> {
    let mut style = style(offset, anchor);
    style.zoom = Some(zoom);
    let layers = layers(&style);
    let map = fixture_map(style, layers, samples).await;
    let pixels = read_blocking(&map);
    if let Some(index) = pixels
        .chunks_exact(4)
        .position(|p| p[0] > 180 && p[1] < 80 && p[2] < 80)
    {
        let point = [
            (index % SIZE as usize) as f64,
            (index / SIZE as usize) as f64,
        ];
        let hits = map.query_rendered_symbols(point, None);
        assert!(
            hits.iter()
                .any(|hit| hit.layer == "label" && hit.text == "Alps"),
            "visible text must be selectable at {point:?}: hits {hits:?}, placed {:?}",
            map.world()
                .resources
                .get::<crate::sdf::query::PlacedSymbols>()
        );
        assert!(
            map.query_rendered_symbols(point, Some(&["point"]))
                .is_empty(),
            "layer filter excludes symbol"
        );
    }
    pixels
}

fn layers(style: &Style) -> crate::headless::map::ProcessedLayers {
    let coords = WorldTileCoords {
        x: 2048,
        y: 2048,
        z: ZoomLevel::from(12),
    };
    let source = geozero::mvt::tile::Layer {
        name: "places".into(),
        version: 2,
        extent: Some(4096),
        features: vec![geozero::mvt::tile::Feature {
            r#type: Some(1),
            geometry: vec![9, 4096, 4096],
            ..Default::default()
        }],
        ..Default::default()
    };
    let bytes = geozero::mvt::Tile {
        layers: vec![source.clone()],
    }
    .encode_to_vec();
    let mut layers =
        process_tile_layers(&bytes, &style.layers[1], coords, Default::default()).expect("point");
    let atlas = atlas();
    let Some(LayerPaint::Symbol(paint)) = &style.layers[2].paint else {
        panic!("symbol paint");
    };
    let mut layout = TextTessellatorNew::default();
    layout.configure(paint.clone(), atlas.clone());
    layout.point_begin(0).expect("point begin");
    layout.xy(2048.0, 2048.0, 0).expect("position");
    layout.point_end(0).expect("point end");
    layout.feature_end(0).expect("feature");
    layout.finish();
    assert!(
        !layout.quad_buffer.indices.is_empty(),
        "symbol fixture produced no quads"
    );
    layers
        .symbols
        .push(Box::new(DefaultSymbolLayerTessellated::build_from(
            coords,
            lyon::tessellation::VertexBuffers::new().into(),
            layout.quad_buffer.into(),
            layout.features,
            Some(atlas),
            source,
            "label".into(),
        )));
    layers
}

fn atlas() -> Arc<crate::sdf::assets::SymbolAtlas> {
    let mut atlas = AtlasBuilder::new();
    atlas
        .glyph_range(
            "Open Sans Regular",
            include_bytes!("../../../../data/0-255.pbf"),
        )
        .expect("glyphs");
    let rect = atlas
        .pack(16, 16, &[0, 255, 0, 255].repeat(256))
        .expect("sprite");
    atlas.atlas.icons.insert(
        "marker".into(),
        AtlasEntry {
            rect,
            metrics: [0., 0., 0., 1.],
            kind: 1,
        },
    );
    atlas.finish()
}

fn read_blocking(map: &HeadlessMap) -> Vec<u8> {
    let texture = map.head_texture().expect("color");
    let buffer = map.device().create_buffer(&wgpu::BufferDescriptor {
        label: Some("symbol pixels"),
        size: u64::from(SIZE * SIZE * 4),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = map.device().create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::ImageCopyBuffer {
            buffer: &buffer,
            layout: wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(SIZE * 4),
                rows_per_image: None,
            },
        },
        texture.size(),
    );
    map.queue().submit([encoder.finish()]);
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, |result| result.expect("readback"));
    map.device().poll(wgpu::Maintain::Wait);
    let bytes = buffer.slice(..).get_mapped_range().to_vec();
    buffer.unmap();
    bytes
}

fn colored_bounds(pixels: &[u8], channel: usize) -> (usize, [usize; 4]) {
    let mut count = 0;
    let mut bounds = [SIZE as usize, SIZE as usize, 0, 0];
    for (index, pixel) in pixels.chunks_exact(4).enumerate() {
        if pixel[channel] > 150 && pixel[(channel + 1) % 3] < 100 && pixel[(channel + 2) % 3] < 100
        {
            count += 1;
            let (x, y) = (index % SIZE as usize, index / SIZE as usize);
            bounds = [
                bounds[0].min(x),
                bounds[1].min(y),
                bounds[2].max(x),
                bounds[3].max(y),
            ];
        }
    }
    (count, bounds)
}

#[tokio::test]
async fn elevated_text_and_sprite_render_over_an_ancestor_dem() {
    let ground = render(0., "ground", 12., 4).await;
    let (red, text) = colored_bounds(&ground, 0);
    let (green, icon) = colored_bounds(&ground, 1);
    assert!(
        red > 70,
        "text invisible or unreadably small: {red} {text:?}"
    );
    assert!(green > 100, "sprite missing: {green} {icon:?}");
    assert!(
        text[2] - text[0] > 35 && text[3] - text[1] > 12,
        "text squashed: {text:?}"
    );
    let raised = render(700., "ground", 12., 4).await;
    let (_, raised) = colored_bounds(&raised, 0);
    assert!(
        raised[1] + 8 < text[1],
        "height offset did not lift text: {text:?} {raised:?}"
    );
    let sea = render(0., "sea", 12., 4).await;
    let (red, _) = colored_bounds(&sea, 0);
    assert_eq!(
        red, 0,
        "terrain must occlude a sea-level label below 1200 m ground"
    );
}

#[tokio::test]
async fn overzoomed_parent_symbols_keep_readable_pixel_sizes_without_msaa() {
    let pixels = render(0., "ground", 14., 1).await;
    let (red, bounds) = colored_bounds(&pixels, 0);
    assert!(red > 70, "parent tile text vanished: {red}");
    assert!(
        bounds[2] - bounds[0] > 35 && bounds[2] - bounds[0] < 100,
        "text scales with tile extent: {bounds:?}"
    );
    let (green, _) = colored_bounds(&pixels, 1);
    assert!(green > 100, "overzoomed sprite vanished");
}

async fn fixture_map(
    style: Style,
    layers: crate::headless::map::ProcessedLayers,
    samples: u32,
) -> HeadlessMap {
    let (kernel, renderer) = create_headless_renderer_with_settings(
        SIZE,
        SIZE,
        None,
        crate::render::settings::RendererSettings {
            msaa: crate::render::settings::Msaa { samples },
            ..Default::default()
        },
    )
    .await
    .expect("renderer");
    let mut map = HeadlessMap::new(
        style,
        renderer,
        kernel,
        vec![
            Box::new(RenderPlugin),
            Box::new(crate::background::BackgroundPlugin),
            Box::new(crate::terrain::TerrainPlugin::<
                crate::terrain::transferables::DefaultDemTransferables,
            >::default()),
            Box::new(VectorPlugin::<DefaultVectorTransferables>::default()),
            Box::new(SdfPlugin::<DefaultVectorTransferables>::default()),
            Box::new(crate::headless::HeadlessPlugin::new(false).preserve_tile_sources()),
        ],
    )
    .expect("map");
    let dem = image::RgbaImage::from_pixel(16, 16, image::Rgba([132, 176, 0, 255]));
    map.render_frames_with_terrain(
        layers,
        vec![],
        vec![(
            WorldTileCoords {
                x: 0,
                y: 0,
                z: ZoomLevel::from(0),
            },
            dem,
        )],
        16,
    )
    .expect("frame");
    map
}

mod navigation;
