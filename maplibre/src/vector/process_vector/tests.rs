#![allow(clippy::expect_used, clippy::panic)]

use std::{cell::RefCell, rc::Rc};

use geozero::mvt::{tile, Message as _, Tile};

use super::{process_vector_tile, ProcessVectorContext, VectorTileRequest};
use crate::{
    coords::WorldTileCoords,
    io::{
        apc::{Context, IntoMessage, Message, SendError},
        geometry_index::TileIndex,
    },
    projection::ProjectionType,
    style::layer::StyleLayer,
    vector::{
        transferables::{
            DefaultLayerIndexed, DefaultLayerMissing, DefaultLayerTessellated, LayerIndexed,
            LayerMissing,
        },
        DefaultVectorTransferables, LayerTessellated,
    },
};

/// Collects everything a worker would send back to the map.
#[derive(Default)]
struct CollectingContext(Rc<RefCell<Vec<Message>>>);

impl Context for CollectingContext {
    fn send_back<T: IntoMessage>(&self, message: T) -> Result<(), SendError> {
        self.0.borrow_mut().push(message.into());
        Ok(())
    }
}

fn zigzag(value: i64) -> u32 {
    ((value << 1) ^ (value >> 63)) as u32
}

/// One line from the tile origin to its far corner, in a layer declaring `extent`.
fn diagonal_line_tile(extent: u32) -> Vec<u8> {
    let far = zigzag(i64::from(extent));
    let layer = tile::Layer {
        version: 2,
        name: "airways".to_string(),
        features: vec![tile::Feature {
            id: Some(1),
            tags: vec![0, 0],
            r#type: Some(tile::GeomType::Linestring as i32),
            geometry: vec![9, 0, 0, 10, far, far],
        }],
        keys: vec!["level".to_string()],
        values: vec![tile::Value {
            string_value: Some("low".to_string()),
            ..Default::default()
        }],
        extent: Some(extent),
    };
    Tile {
        layers: vec![layer],
    }
    .encode_to_vec()
}

/// One point feature in a layer declaring `extent`.
fn point_tile(extent: u32, x: i64, y: i64) -> Vec<u8> {
    let layer = tile::Layer {
        version: 2,
        name: "airports".to_string(),
        features: vec![tile::Feature {
            id: Some(1),
            tags: Vec::new(),
            r#type: Some(tile::GeomType::Point as i32),
            geometry: vec![9, zigzag(x), zigzag(y)],
        }],
        extent: Some(extent),
        ..Default::default()
    };
    Tile {
        layers: vec![layer],
    }
    .encode_to_vec()
}

fn circle_layer() -> StyleLayer {
    serde_json::from_value(serde_json::json!({
        "id": "airports", "type": "circle", "source": "chart", "source-layer": "airports",
        "paint": {"circle-radius": 4, "circle-stroke-width": 1}
    }))
    .expect("valid style layer")
}

fn line_layer(filter: Option<serde_json::Value>) -> StyleLayer {
    let mut layer = serde_json::json!({
        "id": "airways", "type": "line", "source": "chart", "source-layer": "airways",
        "paint": {"line-color": "#000000", "line-width": 1}
    });
    if let Some(filter) = filter {
        layer["filter"] = filter;
    }
    serde_json::from_value(layer).expect("valid style layer")
}

fn process(bytes: &[u8], layer: StyleLayer) -> Vec<Message> {
    let mut processor = ProcessVectorContext::<DefaultVectorTransferables, CollectingContext>::new(
        CollectingContext::default(),
    );
    process_vector_tile(
        bytes,
        VectorTileRequest {
            coords: WorldTileCoords::default(),
            layers: [layer].into_iter().collect(),
            projection: ProjectionType::Mercator,
        },
        &mut processor,
    )
    .expect("tile processes");
    processor.take_context().0.take()
}

fn tessellated(messages: Vec<Message>) -> Vec<DefaultLayerTessellated> {
    messages
        .into_iter()
        .filter(|message| message.has_tag(DefaultLayerTessellated::message_tag()))
        .map(|message| *message.into_transferable::<DefaultLayerTessellated>())
        .collect()
}

fn maximum_vertex_x(bytes: &[u8]) -> f32 {
    let layers = tessellated(process(bytes, line_layer(None)));
    layers
        .iter()
        .flat_map(|layer| layer.buffer.buffer.vertices.iter())
        .map(|vertex| vertex.position[0])
        .fold(f32::NEG_INFINITY, f32::max)
}

fn maximum_index_x(bytes: &[u8]) -> f64 {
    let indexed = process(bytes, line_layer(None))
        .into_iter()
        .find(|message| message.has_tag(DefaultLayerIndexed::message_tag()))
        .map(|message| *message.into_transferable::<DefaultLayerIndexed>())
        .expect("index message");
    let TileIndex::Linear { list } = indexed.to_tile_index() else {
        panic!("worker index is linear");
    };
    list.iter()
        .map(|geometry| geometry.bounds.upper().x())
        .fold(f64::NEG_INFINITY, f64::max)
}

#[test]
fn layers_with_a_larger_extent_land_on_the_same_grid() {
    let at_4096 = maximum_vertex_x(&diagonal_line_tile(4096));
    let at_8192 = maximum_vertex_x(&diagonal_line_tile(8192));

    assert!(at_4096.is_finite(), "the line produced vertices");
    assert!(
        (at_4096 - at_8192).abs() < 1e-3,
        "the same geographic line must end at the same place: {at_4096} vs {at_8192}"
    );
    assert!(
        (at_4096 - 4096.0).abs() < 1.0,
        "the far corner sits on the 4096 grid, not {at_4096}"
    );
}

#[test]
fn the_query_index_follows_the_layer_extent() {
    assert!((maximum_index_x(&diagonal_line_tile(4096)) - 4096.0).abs() < 1e-6);
    assert!((maximum_index_x(&diagonal_line_tile(8192)) - 4096.0).abs() < 1e-6);
}

fn feature_count(filter: serde_json::Value) -> usize {
    tessellated(process(&diagonal_line_tile(4096), line_layer(Some(filter))))
        .iter()
        .map(|layer| layer.feature_indices.len())
        .sum()
}

#[test]
fn expression_and_legacy_filters_select_the_same_features() {
    assert_eq!(feature_count(serde_json::json!(["==", "level", "low"])), 1);
    assert_eq!(
        feature_count(serde_json::json!(["==", ["get", "level"], "low"])),
        1
    );
    assert_eq!(feature_count(serde_json::json!(["==", "level", "high"])), 0);
    assert_eq!(
        feature_count(serde_json::json!(["!=", ["get", "level"], "low"])),
        0
    );
    assert_eq!(
        feature_count(serde_json::json!(["==", ["geometry-type"], "LineString"])),
        1
    );
}

#[test]
fn an_unsupported_filter_reports_the_layer_missing_instead_of_guessing() {
    let messages = process(
        &diagonal_line_tile(4096),
        line_layer(Some(
            serde_json::json!(["within", {"type": "Polygon", "coordinates": []}]),
        )),
    );

    assert!(
        messages
            .iter()
            .all(|message| !message.has_tag(DefaultLayerTessellated::message_tag())),
        "no geometry is rendered for a filter that cannot be evaluated"
    );
    let missing: Vec<String> = messages
        .into_iter()
        .filter(|message| message.has_tag(DefaultLayerMissing::message_tag()))
        .map(|message| {
            message
                .into_transferable::<DefaultLayerMissing>()
                .layer_name()
                .to_string()
        })
        .collect();
    assert_eq!(missing, vec!["airways".to_string()]);
}

#[test]
fn a_point_in_a_larger_extent_becomes_one_circle_quad_on_the_4096_grid() {
    let layers = tessellated(process(&point_tile(8192, 4096, 2048), circle_layer()));
    let [layer] = layers.as_slice() else {
        panic!("one tessellated layer, got {}", layers.len());
    };
    let buffer = &layer.buffer.buffer;

    assert_eq!(buffer.indices.len(), 6, "one quad per point");
    assert_eq!(buffer.vertices.len(), 4);
    for vertex in &buffer.vertices {
        assert_eq!(vertex.position, [2048.0, 1024.0]);
        assert_eq!(
            vertex.normal,
            [4.0, 1.0],
            "radius and stroke width ride along"
        );
    }
}
