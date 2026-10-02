use std::time::{Duration, Instant};

use criterion::{criterion_group, criterion_main, Criterion};
use maplibre::{
    coords::{WorldTileCoords, ZoomLevel},
    headless::{create_headless_renderer, map::HeadlessMap, HeadlessPlugin},
    platform::run_multithreaded,
    plugin::Plugin,
    render::RenderPlugin,
    style::Style,
    vector::{DefaultVectorTransferables, VectorPlugin},
};

fn headless_render(c: &mut Criterion) {
    c.bench_function("headless_render", |b| {
        let (mut map, tile, water_layer) = run_multithreaded(async {
            let (kernel, renderer) = create_headless_renderer(1000, 1000, None)
                .await
                .unwrap_or_else(|error| panic!("Failed to create benchmark renderer: {error}"));
            let style = Style::default();

            let plugins: Vec<Box<dyn Plugin<_>>> = vec![
                Box::new(RenderPlugin),
                Box::new(VectorPlugin::<DefaultVectorTransferables>::default()),
                Box::new(HeadlessPlugin::new(false)),
            ];

            let map = HeadlessMap::new(style.clone(), renderer, kernel, plugins).unwrap();

            let tile = map
                .fetch_tile(WorldTileCoords::from((0, 0, ZoomLevel::default())))
                .await
                .expect("Failed to fetch!");

            let water_layer = style
                .layers
                .iter()
                .find(|layer| layer.source_layer == Some("water".to_string()))
                .expect("water layer must exist")
                .clone();

            (map, tile, water_layer)
        });

        // Rendering consumes the processed tile, so each iteration processes it afresh outside
        // the timed span.
        b.iter_custom(|iterations| {
            let mut rendering = Duration::ZERO;
            for _ in 0..iterations {
                let layers = run_multithreaded(map.process_tile(tile.clone(), &water_layer))
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
