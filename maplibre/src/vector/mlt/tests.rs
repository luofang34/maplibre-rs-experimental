#![allow(clippy::expect_used, clippy::panic)]

use std::{cell::RefCell, rc::Rc};

use geozero::mvt::{tile, Message as _, Tile};
use mlt_core::encoder::{EncoderConfig, Network, Profile};

use super::{decode, is_valid_mlt};
use crate::{
    coords::WorldTileCoords,
    io::apc::{Context, IntoMessage, Message, SendError},
    projection::ProjectionType,
    style::layer::StyleLayer,
    vector::{
        index_mvt_bytes, looks_like_mvt, process_vector_tile,
        transferables::{
            DefaultLayerTessellated, DefaultSymbolLayerTessellated, DefaultVectorTransferables,
            VectorMessageTag,
        },
        ProcessVectorContext, VectorTileRequest,
    },
};

const TILE: &[u8] = include_bytes!("../../../../render-tests/src/assets/tiles/mlt/5/17/10.mlt");

#[derive(Default)]
struct RecordingContext(Rc<RefCell<Vec<Message>>>);

impl Context for RecordingContext {
    fn send_back<T: IntoMessage>(&self, message: T) -> Result<(), SendError> {
        self.0.borrow_mut().push(message.into());
        Ok(())
    }
}

fn zigzag(v: i32) -> u32 {
    ((v << 1) ^ (v >> 31)) as u32
}

#[test]
fn a_maplibre_tile_decodes_to_named_layers_with_features() {
    let tile = decode(TILE).expect("the fixture tile decodes");
    assert!(!tile.layers.is_empty());
    assert!(tile.layers.iter().any(|layer| !layer.features.is_empty()));
    assert!(tile.layers.iter().all(|layer| !layer.name.is_empty()));
}

#[test]
fn a_payload_that_is_not_a_tile_is_an_error() {
    assert!(decode(&[0xff, 0xff, 0xff, 0xff, 0xff]).is_err());
    assert!(!is_valid_mlt(&[0xff, 0xff, 0xff, 0xff, 0xff]));
}

#[test]
fn the_generic_decoder_falls_back_to_maplibre_tiles() {
    let tile = super::super::process_vector::decode_tile(TILE).expect("the tile decodes");
    assert!(!tile.layers.is_empty());
}

#[test]
fn columnar_mlt_matches_mvt_for_points_lines_holed_polygons_and_symbols() {
    let points_layer = tile::Layer {
        version: 2,
        name: "places".to_owned(),
        extent: Some(4096),
        keys: vec!["name".to_owned(), "rank".to_owned()],
        values: vec![
            tile::Value {
                string_value: Some("Alpha".to_owned()),
                ..Default::default()
            },
            tile::Value {
                sint_value: Some(1),
                ..Default::default()
            },
            tile::Value {
                string_value: Some("Beta".to_owned()),
                ..Default::default()
            },
            tile::Value {
                sint_value: Some(2),
                ..Default::default()
            },
        ],
        features: vec![
            tile::Feature {
                id: Some(10),
                tags: vec![0, 0, 1, 1],
                r#type: Some(tile::GeomType::Point as i32),
                geometry: vec![(1 << 3) | 1, zigzag(512), zigzag(768)],
            },
            tile::Feature {
                id: Some(11),
                tags: vec![0, 2, 1, 3],
                r#type: Some(tile::GeomType::Point as i32),
                geometry: vec![
                    (2 << 3) | 1,
                    zigzag(1024),
                    zigzag(1024),
                    zigzag(256),
                    zigzag(512),
                ],
            },
        ],
    };

    let roads_layer = tile::Layer {
        version: 2,
        name: "roads".to_owned(),
        extent: Some(4096),
        keys: vec!["class".to_owned()],
        values: vec![tile::Value {
            string_value: Some("primary".to_owned()),
            ..Default::default()
        }],
        features: vec![
            tile::Feature {
                id: Some(20),
                tags: vec![0, 0],
                r#type: Some(tile::GeomType::Linestring as i32),
                geometry: vec![
                    (1 << 3) | 1,
                    zigzag(100),
                    zigzag(200),
                    (2 << 3) | 2,
                    zigzag(400),
                    zigzag(300),
                    zigzag(500),
                    zigzag(-100),
                ],
            },
            tile::Feature {
                id: Some(21),
                tags: vec![0, 0],
                r#type: Some(tile::GeomType::Linestring as i32),
                geometry: vec![
                    (1 << 3) | 1,
                    zigzag(1000),
                    zigzag(1000),
                    (1 << 3) | 2,
                    zigzag(300),
                    zigzag(0),
                    (1 << 3) | 1,
                    zigzag(100),
                    zigzag(200),
                    (1 << 3) | 2,
                    zigzag(0),
                    zigzag(400),
                ],
            },
        ],
    };

    // Exterior CW ring (0,0)->(1000,0)->(1000,1000)->(0,1000) in screen-down MVT coords has
    // positive surveyor area (+2_000_000), followed by interior CCW hole (200,200)->(200,600)->(600,600)->(600,200).
    let land_layer = tile::Layer {
        version: 2,
        name: "land".to_owned(),
        extent: Some(4096),
        keys: vec!["kind".to_owned()],
        values: vec![tile::Value {
            string_value: Some("park".to_owned()),
            ..Default::default()
        }],
        features: vec![tile::Feature {
            id: Some(30),
            tags: vec![0, 0],
            r#type: Some(tile::GeomType::Polygon as i32),
            geometry: vec![
                // Exterior ring
                (1 << 3) | 1,
                zigzag(0),
                zigzag(0),
                (3 << 3) | 2,
                zigzag(1000),
                zigzag(0),
                zigzag(0),
                zigzag(1000),
                zigzag(-1000),
                zigzag(0),
                (1 << 3) | 7,
                // Interior hole relative to cursor (0, 1000) -> move to (200, 200) = (+200, -800)
                (1 << 3) | 1,
                zigzag(200),
                zigzag(-800),
                (3 << 3) | 2,
                zigzag(0),
                zigzag(400),
                zigzag(400),
                zigzag(0),
                zigzag(0),
                zigzag(-400),
                (1 << 3) | 7,
            ],
        }],
    };

    let mvt_bytes = Tile {
        layers: vec![points_layer, roads_layer, land_layer],
    }
    .encode_to_vec();

    let cfg = EncoderConfig::from_profile(Profile::Web(Network::default()));
    let mlt_tile_layers = mlt_core::mvt::mvt_to_tile_layers(&mvt_bytes).expect("convert to MLT");
    let mut mlt_bytes = Vec::new();
    for layer in &mlt_tile_layers {
        mlt_bytes.extend_from_slice(&layer.encode(cfg).expect("encode MLT layer"));
    }

    assert!(looks_like_mvt(&mvt_bytes));
    assert!(!looks_like_mvt(&mlt_bytes));
    assert!(is_valid_mlt(&mlt_bytes));

    let style_layers: Vec<StyleLayer> = serde_json::from_value(serde_json::json!([
        {
            "id": "places-circle",
            "index": 0,
            "type": "circle",
            "source": "s",
            "source-layer": "places",
            "paint": { "circle-radius": 5.0, "circle-color": "#ff3300" }
        },
        {
            "id": "roads-line",
            "index": 1,
            "type": "line",
            "source": "s",
            "source-layer": "roads",
            "paint": { "line-width": 2.0, "line-color": "#0066ff" }
        },
        {
            "id": "land-fill",
            "index": 2,
            "type": "fill",
            "source": "s",
            "source-layer": "land",
            "paint": { "fill-color": "#22aa44" }
        },
        {
            "id": "places-label",
            "index": 3,
            "type": "symbol",
            "source": "s",
            "source-layer": "places",
            "layout": { "text-field": ["get", "name"] },
            "paint": { "text-color": "#111111" }
        }
    ]))
    .expect("valid style layers");

    let run = |bytes: &[u8]| -> Vec<Message> {
        let mut ctx = ProcessVectorContext::<DefaultVectorTransferables, RecordingContext>::new(
            RecordingContext::default(),
        );
        process_vector_tile(
            bytes,
            VectorTileRequest {
                coords: WorldTileCoords::default(),
                layers: style_layers.iter().cloned().collect(),
                projection: ProjectionType::Mercator,
                overscaled_zoom: 0,
            },
            &mut ctx,
        )
        .expect("tile processes cleanly");
        ctx.take_context().0.take()
    };

    let mvt_msgs = run(&mvt_bytes);
    let mlt_msgs = run(&mlt_bytes);
    assert_eq!(mvt_msgs.len(), mlt_msgs.len());

    let unpack = |msgs: Vec<Message>| {
        let mut map = std::collections::BTreeMap::new();
        let mut sym = None;
        for msg in msgs {
            if msg.has_tag(&VectorMessageTag::LayerTessellated) {
                let item = msg
                    .into_transferable::<DefaultLayerTessellated>()
                    .expect("DefaultLayerTessellated");
                map.insert(
                    item.style_layer_id,
                    (
                        item.buffer.buffer.vertices,
                        item.buffer.buffer.indices,
                        item.feature_indices,
                        item.feature_colors,
                    ),
                );
            } else if msg.has_tag(&VectorMessageTag::SymbolLayerTessellated) {
                sym = Some(
                    *msg.into_transferable::<DefaultSymbolLayerTessellated>()
                        .expect("DefaultSymbolLayerTessellated"),
                );
            }
        }
        (map, sym.expect("symbol layer tessellated"))
    };

    let (mvt_layers, mvt_sym) = unpack(mvt_msgs);
    let (mlt_layers, mlt_sym) = unpack(mlt_msgs);
    assert_eq!(mvt_layers.len(), 3);
    for (id, mvt_out) in &mvt_layers {
        let mlt_out = mlt_layers.get(id).expect("matching MLT layer");
        assert!(!mvt_out.1.is_empty(), "layer {id} produced indices");
        assert_eq!(
            bytemuck::cast_slice::<_, u8>(&mvt_out.0),
            bytemuck::cast_slice::<_, u8>(&mlt_out.0),
            "vertices match for {id}"
        );
        assert_eq!(mvt_out.1, mlt_out.1, "indices match for {id}");
        assert_eq!(mvt_out.2, mlt_out.2, "feature_indices match for {id}");
        assert_eq!(mvt_out.3, mlt_out.3, "feature_colors match for {id}");
    }

    assert!(!mvt_sym.buffer.buffer.indices.is_empty());
    assert_eq!(
        bytemuck::cast_slice::<_, u8>(&mvt_sym.buffer.buffer.vertices),
        bytemuck::cast_slice::<_, u8>(&mlt_sym.buffer.buffer.vertices)
    );
    assert_eq!(mvt_sym.buffer.buffer.indices, mlt_sym.buffer.buffer.indices);
    assert_eq!(mvt_sym.features.len(), mlt_sym.features.len());

    let mvt_indexed = index_mvt_bytes(&mvt_bytes, None, None);
    let mlt_indexed = index_mvt_bytes(&mlt_bytes, None, None);
    assert_eq!(mvt_indexed.len(), mlt_indexed.len());
    for (a, b) in mvt_indexed.iter().zip(&mlt_indexed) {
        assert_eq!(a.source_layer, b.source_layer);
        assert_eq!(a.id, b.id);
        assert_eq!(a.properties, b.properties);
    }
}
