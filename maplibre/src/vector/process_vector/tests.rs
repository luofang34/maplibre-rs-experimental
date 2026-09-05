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
            DefaultLayerIndexed, DefaultLayerMissing, DefaultLayerTessellated,
            DefaultSymbolLayerTessellated, LayerIndexed, LayerMissing, SymbolLayerTessellated,
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

/// One point at the tile centre named `FALLBACK` and labelled `V12`, so a label that reads
/// the wrong property is told apart from one that reads the right property, with the
/// altitude, course and distance a chart annotates.
fn labelled_point_tile() -> Vec<u8> {
    let string = |text: &str| tile::Value {
        string_value: Some(text.to_string()),
        ..Default::default()
    };
    let double = |number: f64| tile::Value {
        double_value: Some(number),
        ..Default::default()
    };
    let layer = tile::Layer {
        version: 2,
        name: "route_points".to_string(),
        features: vec![tile::Feature {
            id: Some(1),
            tags: vec![0, 0, 1, 1, 2, 2, 3, 3, 4, 4],
            r#type: Some(tile::GeomType::Point as i32),
            geometry: vec![9, zigzag(2048), zigzag(2048)],
        }],
        keys: ["name", "label", "alt", "course", "dist"]
            .map(str::to_string)
            .to_vec(),
        values: vec![
            string("FALLBACK"),
            string("V12"),
            tile::Value {
                int_value: Some(5000),
                ..Default::default()
            },
            double(270.5),
            double(12.34),
        ],
        extent: Some(4096),
    };
    Tile {
        layers: vec![layer],
    }
    .encode_to_vec()
}

fn symbol_layer(layout: Option<serde_json::Value>) -> StyleLayer {
    let mut layer = serde_json::json!({
        "id": "label", "type": "symbol", "source": "chart", "source-layer": "route_points"
    });
    if let Some(layout) = layout {
        layer["layout"] = layout;
    }
    serde_json::from_value(layer).expect("valid style layer")
}

/// The text of every label a symbol layer produced for the labelled point.
fn label_texts(layout: Option<serde_json::Value>) -> Vec<String> {
    process(&labelled_point_tile(), symbol_layer(layout))
        .into_iter()
        .filter(|message| message.has_tag(DefaultSymbolLayerTessellated::message_tag()))
        .map(|message| *message.into_transferable::<DefaultSymbolLayerTessellated>())
        .flat_map(|layer| layer.features.into_iter().map(|feature| feature.str))
        .collect()
}

#[test]
fn a_text_field_template_reads_the_named_property() {
    assert_eq!(
        label_texts(Some(serde_json::json!({"text-field": "{label}"}))),
        ["V12"]
    );
}

#[test]
fn text_field_expressions_read_the_property_they_name() {
    assert_eq!(
        label_texts(Some(serde_json::json!({"text-field": ["get", "label"]}))),
        ["V12"]
    );
    assert_eq!(
        label_texts(Some(
            serde_json::json!({"text-field": ["coalesce", ["get", "label"], ""]})
        )),
        ["V12"]
    );
}

#[test]
fn a_literal_text_field_labels_every_feature_with_the_literal() {
    assert_eq!(
        label_texts(Some(serde_json::json!({"text-field": "FIX"}))),
        ["FIX"]
    );
}

#[test]
fn a_missing_property_or_absent_text_field_draws_no_label() {
    assert!(label_texts(Some(serde_json::json!({"text-field": "{missing}"}))).is_empty());
    assert!(label_texts(None).is_empty());
}

#[test]
fn a_zoom_dependent_text_field_evaluates_at_the_tile_zoom() {
    let layout = serde_json::json!({
        "text-field": {"stops": [[2, "{name}"], [4, "{label}"]]}
    });
    assert_eq!(
        label_texts(Some(layout)),
        ["FALLBACK"],
        "the tile is at zoom 0"
    );
}

#[test]
fn chart_annotation_expressions_read_text_and_numbers() {
    for (text_field, expected) in [
        (
            serde_json::json!(["concat", ["get", "label"], " ", ["get", "alt"]]),
            "V12 5000",
        ),
        (
            serde_json::json!([
                "case",
                ["has", "course"],
                ["concat", ["to-string", ["get", "course"]], " deg"],
                "n/a"
            ]),
            "270.5 deg",
        ),
        (
            serde_json::json!(["concat", ["get", "dist"], " nm"]),
            "12.34 nm",
        ),
        (
            serde_json::json!(["coalesce", ["get", "missing"], ["get", "label"]]),
            "V12",
        ),
        (serde_json::json!(["to-string", ["get", "alt"]]), "5000"),
    ] {
        assert_eq!(
            label_texts(Some(serde_json::json!({"text-field": text_field}))),
            [expected],
            "{text_field}"
        );
    }
}
