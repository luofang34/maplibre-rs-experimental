#![allow(clippy::unwrap_used, clippy::expect_used, clippy::mutable_key_type)]

use std::collections::HashSet;

use criterion::{criterion_group, criterion_main, Criterion};
use maplibre::{
    benchmarking::io::static_tile_fetcher::StaticTileFetcher,
    coords::{TileCoords, ZoomLevel},
    io::apc::{Context, IntoMessage, SendError},
    projection::ProjectionType,
    style::{layer::StyleLayer, source::TileAddressingScheme, Style},
    vector::{
        process_vector_tile, DefaultVectorTransferables, ProcessVectorContext, VectorTileRequest,
    },
};
use mlt_core::encoder::{EncoderConfig, Network, Profile};

// https://tile.openstreetmap.org/15/17425/11365.png
const MUNICH_COORDS: TileCoords = TileCoords {
    x: 17425,
    y: 11365,
    z: ZoomLevel::new(15u8),
};

pub struct DummyContext;

impl Context for DummyContext {
    fn send_back<T: IntoMessage>(&self, _message: T) -> Result<(), SendError> {
        Ok(())
    }
}

fn encode_mvt_as_mlt(mvt_bytes: &[u8]) -> Box<[u8]> {
    let cfg = EncoderConfig::from_profile(Profile::Web(Network::default()));
    let layers = mlt_core::mvt::mvt_to_tile_layers(mvt_bytes).expect("convert MVT fixture to MLT");
    let mut out = Vec::new();
    for layer in &layers {
        out.extend_from_slice(&layer.encode(cfg).expect("encode MLT layer"));
    }
    out.into_boxed_slice()
}

fn bench_process_vector_tile(c: &mut Criterion) {
    let fetcher = StaticTileFetcher::new();
    let mvt_data = fetcher
        .sync_fetch_tile(&MUNICH_COORDS)
        .unwrap()
        .into_boxed_slice();
    let mlt_data = encode_mvt_as_mlt(&mvt_data);
    let coords = MUNICH_COORDS
        .into_world_tile(TileAddressingScheme::XYZ)
        .unwrap();
    let layers: HashSet<StyleLayer> = Style::default().layers.iter().cloned().collect();

    let mut group = c.benchmark_group("process_vector_tile");
    group.bench_function("mvt", |b| {
        b.iter(|| {
            process_vector_tile(
                &mvt_data,
                VectorTileRequest {
                    coords,
                    layers: layers.clone(),
                    projection: ProjectionType::Mercator,
                    overscaled_zoom: 0,
                },
                &mut ProcessVectorContext::<DefaultVectorTransferables, _>::new(DummyContext),
            )
            .unwrap();
        })
    });
    group.bench_function("mlt_columnar", |b| {
        b.iter(|| {
            process_vector_tile(
                &mlt_data,
                VectorTileRequest {
                    coords,
                    layers: layers.clone(),
                    projection: ProjectionType::Mercator,
                    overscaled_zoom: 0,
                },
                &mut ProcessVectorContext::<DefaultVectorTransferables, _>::new(DummyContext),
            )
            .unwrap();
        })
    });
    group.finish();
}

criterion_group!(benches, bench_process_vector_tile);
criterion_main!(benches);
