//! Deterministic mutation fuzzing for malformed MVT and MLT tiles across decoding and
//! tessellation (including globe projection subdivision), modeled on `mln-rs`'s
//! `malformed_tile_test` suites.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::mutable_key_type)]

use std::{cell::RefCell, rc::Rc};

use geozero::mvt::{tile, Message as _, Tile};
use rustc_hash::FxHashSet as HashSet;

use super::super::{process_vector_tile, ProcessVectorContext, VectorTileRequest};
use crate::{
    coords::{WorldTileCoords, ZoomLevel},
    io::apc::{Context, IntoMessage, SendError},
    projection::ProjectionType,
    style::layer::StyleLayer,
    vector::DefaultVectorTransferables,
};

#[derive(Default)]
struct SinkContext(Rc<RefCell<usize>>);

impl Context for SinkContext {
    fn send_back<T: IntoMessage>(&self, _message: T) -> Result<(), SendError> {
        *self.0.borrow_mut() += 1;
        Ok(())
    }
}

struct XorShift64(u64);

impl XorShift64 {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn next_usize(&mut self, upper: usize) -> usize {
        if upper == 0 {
            0
        } else {
            (self.next() as usize) % upper
        }
    }
}

fn valid_seed_mvt() -> Vec<u8> {
    let layer = tile::Layer {
        version: 2,
        name: "water".to_owned(),
        extent: Some(4096),
        keys: vec!["class".to_owned()],
        values: vec![tile::Value {
            string_value: Some("ocean".to_owned()),
            ..Default::default()
        }],
        features: vec![
            tile::Feature {
                id: Some(1),
                tags: vec![0, 0],
                r#type: Some(tile::GeomType::Polygon as i32),
                // MoveTo(0,0), LineTo(2048,0), LineTo(0,2048), LineTo(-2048,0), ClosePath
                geometry: vec![9, 0, 0, 26, 4096, 0, 0, 4096, 4095, 0, 15],
            },
            tile::Feature {
                id: Some(2),
                tags: vec![0, 0],
                r#type: Some(tile::GeomType::Linestring as i32),
                geometry: vec![9, 0, 0, 18, 2048, 2048, 2048, 0],
            },
        ],
    };
    Tile {
        layers: vec![layer],
    }
    .encode_to_vec()
}

fn test_layers() -> HashSet<StyleLayer> {
    let fill: StyleLayer = serde_json::from_value(serde_json::json!({
        "id": "water-fill",
        "type": "fill",
        "source": "openmaptiles",
        "source-layer": "water",
        "paint": { "fill-color": "#102a4c" }
    }))
    .expect("valid fill layer");
    let line: StyleLayer = serde_json::from_value(serde_json::json!({
        "id": "water-line",
        "type": "line",
        "source": "openmaptiles",
        "source-layer": "water",
        "paint": { "line-color": "#5eead4", "line-width": 2.0 }
    }))
    .expect("valid line layer");
    [fill, line].into_iter().collect()
}

fn mutate(seed: &[u8], rng: &mut XorShift64) -> Vec<u8> {
    let mut bytes = seed.to_vec();
    match rng.next() % 5 {
        // Truncate
        0 => {
            let new_len = rng.next_usize(bytes.len() + 1);
            bytes.truncate(new_len);
        }
        // Flip 1..=4 bytes
        1 => {
            let flips = 1 + rng.next_usize(4);
            for _ in 0..flips {
                if !bytes.is_empty() {
                    let idx = rng.next_usize(bytes.len());
                    bytes[idx] ^= (rng.next() & 0xff) as u8;
                }
            }
        }
        // Splice random garbage
        2 => {
            let at = rng.next_usize(bytes.len() + 1);
            let extra = 1 + rng.next_usize(16);
            for _ in 0..extra {
                bytes.insert(at, (rng.next() & 0xff) as u8);
            }
        }
        // Corrupt varints (all-high-bit runs)
        3 => {
            let at = rng.next_usize(bytes.len() + 1);
            for _ in 0..12 {
                bytes.insert(at, 0xff);
            }
        }
        // Pure random bytes
        _ => {
            let len = rng.next_usize(96);
            bytes.clear();
            for _ in 0..len {
                bytes.push((rng.next() & 0xff) as u8);
            }
        }
    }
    bytes
}

const DEFAULT_FUZZ_SEED: u64 = 0x9E37_79B9_7F4A_7C15;
const DEFAULT_FUZZ_ROUNDS: usize = 256;

fn fuzz_seed() -> u64 {
    let raw = std::env::var("MAPLIBRE_FUZZ_SEED").ok();
    let parsed = raw.as_deref().and_then(|s| {
        let trimmed = s.trim();
        if let Some(hex) = trimmed
            .strip_prefix("0x")
            .or_else(|| trimmed.strip_prefix("0X"))
        {
            u64::from_str_radix(hex, 16).ok()
        } else {
            trimmed.parse::<u64>().ok()
        }
    });
    match parsed {
        Some(0) | None => DEFAULT_FUZZ_SEED,
        Some(seed) => seed,
    }
}

fn fuzz_rounds() -> usize {
    std::env::var("MAPLIBRE_FUZZ_ROUNDS")
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .map_or(DEFAULT_FUZZ_ROUNDS, |rounds| rounds.max(1))
}

#[test]
fn malformed_mvt_and_mlt_mutations_never_panic() {
    let seed_mvt = valid_seed_mvt();
    // Minimal MLT-like header bytes (version 1 + arbitrary stream tags)
    let seed_mlt: &[u8] = &[0x01, 0x05, 0x01, 0x01, b'w', 0x00, 0x10, 0x20, 0x30];
    let layers = test_layers();
    let mut rng = XorShift64(fuzz_seed());
    let rounds = fuzz_rounds();

    for (round, seed) in (0..rounds).map(|i| {
        if i % 3 == 0 {
            (i, seed_mlt)
        } else {
            (i, seed_mvt.as_slice())
        }
    }) {
        let mutated = if round == 0 {
            Vec::new()
        } else {
            mutate(seed, &mut rng)
        };
        let projection = if round % 2 == 0 {
            ProjectionType::VerticalPerspective
        } else {
            ProjectionType::Mercator
        };
        let request = VectorTileRequest {
            coords: WorldTileCoords::from((0, 0, ZoomLevel::new(2))),
            layers: layers.clone(),
            projection,
            overscaled_zoom: 0,
        };
        let mut context = ProcessVectorContext::<DefaultVectorTransferables, SinkContext>::new(
            SinkContext::default(),
        );
        // Must return Ok or clean Err without panicking.
        let _outcome = process_vector_tile(&mutated, request, &mut context);
    }
}
