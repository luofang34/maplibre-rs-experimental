#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::{Duration, Instant};

use criterion::{criterion_group, criterion_main, Criterion};
use geozero::mvt::{tile, Message as _, Tile};
use maplibre::{
    headless::{create_headless_renderer, map::HeadlessMap, HeadlessPlugin},
    plugin::Plugin,
    render::RenderPlugin,
    style::Style,
    vector::{DefaultVectorTransferables, VectorPlugin},
};

fn zigzag(value: i32) -> u32 {
    ((value << 1) ^ (value >> 31)) as u32
}

/// Builds a deterministic offline 4096-extent MVT tile with a clockwise `water` polygon so the
/// benchmark never makes external network requests.
fn water_mvt_tile() -> Box<[u8]> {
    let origin = zigzag(512);
    let span = zigzag(2048);
    Tile {
        layers: vec![tile::Layer {
            version: 2,
            name: "water".to_string(),
            features: vec![tile::Feature {
                id: Some(1),
                tags: Vec::new(),
                r#type: Some(tile::GeomType::Polygon as i32),
                geometry: vec![
                    9,
                    origin,
                    origin,
                    26,
                    span,
                    0,
                    0,
                    span,
                    zigzag(-2048),
                    0,
                    15,
                ],
            }],
            keys: Vec::new(),
            values: Vec::new(),
            extent: Some(4096),
        }],
    }
    .encode_to_vec()
    .into_boxed_slice()
}

fn headless_render(c: &mut Criterion) {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Failed to create benchmark Tokio runtime");
    let tile = water_mvt_tile();

    let (mut map, water_layer) = runtime.block_on(async {
        let (kernel, renderer) = create_headless_renderer(1000, 1000, None)
            .await
            .unwrap_or_else(|error| panic!("Failed to create benchmark renderer: {error}"));
        let style = Style {
            center: Some([0.0, 0.0]),
            zoom: Some(0.0),
            ..Style::default()
        };

        let plugins: Vec<Box<dyn Plugin<_>>> = vec![
            Box::new(RenderPlugin),
            Box::new(VectorPlugin::<DefaultVectorTransferables>::default()),
            Box::new(HeadlessPlugin::new(false).preserve_tile_sources()),
        ];

        let map = HeadlessMap::new(style.clone(), renderer, kernel, plugins).unwrap();

        let water_layer = style
            .layers
            .iter()
            .find(|layer| layer.source_layer.as_deref() == Some("water"))
            .expect("water layer must exist")
            .clone();

        (map, water_layer)
    });

    c.bench_function("headless_render", |b| {
        // Rendering consumes the processed tile, so each iteration processes it afresh outside
        // the timed span on the shared Tokio runtime.
        b.iter_custom(|iterations| {
            let mut rendering = Duration::ZERO;
            for _ in 0..iterations {
                let layers = runtime
                    .block_on(map.process_tile(tile.clone(), &water_layer))
                    .unwrap_or_else(|error| panic!("Failed to process benchmark tile: {error}"));
                let start = Instant::now();
                map.render_tile(layers)
                    .unwrap_or_else(|error| panic!("Failed to render benchmark tile: {error}"));
                rendering += start.elapsed();
            }
            rendering
        });
    });
}

criterion_group!(name = benches;
    config = Criterion::default().significance_level(0.1).sample_size(20);
    targets = headless_render);
criterion_main!(benches);
