#![allow(clippy::expect_used, clippy::panic)]

use cgmath::InnerSpace;

use geozero::mvt::{tile, Message as _, Tile};

use super::{initial_view_state, process_geojson_layers, process_tile_layers, ProcessedLayers};
use crate::{
    coords::{LatLon, WorldTileCoords, Zoom},
    projection::ProjectionType,
    style::{layer::StyleLayer, light::LightSpecification, Style},
    window::PhysicalSize,
};

#[test]
fn initial_view_uses_style_camera_options() {
    let style: Style = serde_json::from_str(
        r#"{
            "version": 8,
            "center": [160.0, 20.0],
            "zoom": 3.5,
            "bearing": 45.0,
            "pitch": 30.0,
            "sources": {},
            "layers": []
        }"#,
    )
    .expect("style should parse");
    let view = initial_view_state(
        PhysicalSize::new(512, 512).expect("size should be nonzero"),
        &style,
    );

    assert_eq!(view.zoom().value(), Zoom::new(3.5).value());
    assert!((view.camera().get_bearing().0.to_degrees() - 45.0).abs() <= 1e-12);
    assert!((view.camera().get_pitch().0.to_degrees() - 30.0).abs() <= 1e-12);

    let camera_center = crate::render::projection::globe_camera_for_view(&view)
        .expect("globe camera should be valid")
        .center();
    assert!((camera_center.latitude - LatLon::new(20.0, 160.0).latitude).abs() <= 1e-9);
    assert!((camera_center.longitude - LatLon::new(20.0, 160.0).longitude).abs() <= 1e-9);
}

#[test]
fn unrotated_headless_view_keeps_map_light_in_view_axes() {
    let style: Style = serde_json::from_str(
        r#"{
            "version": 8,
            "center": [0.0, 0.0],
            "zoom": 10,
            "sources": {},
            "layers": []
        }"#,
    )
    .expect("style should parse");
    let view = initial_view_state(
        PhysicalSize::new(512, 512).expect("size should be nonzero"),
        &style,
    );
    let camera = crate::render::projection::globe_camera_for_view(&view)
        .expect("globe camera should be valid");
    let map: LightSpecification =
        serde_json::from_str(r#"{"anchor":"map","position":[1.5,0,180]}"#)
            .expect("map light should parse");
    let viewport: LightSpecification =
        serde_json::from_str(r#"{"anchor":"viewport","position":[1.5,0,180]}"#)
            .expect("viewport light should parse");

    let map_direction = map
        .sun_direction_in_view(&camera, 10.0)
        .expect("map direction should be valid");
    let viewport_direction = viewport
        .sun_direction_in_view(&camera, 10.0)
        .expect("viewport direction should be valid");
    assert!((map_direction - viewport_direction).magnitude() <= 1e-12);
}

/// A `route_points` layer with one point at the tile centre, named `FALLBACK` and labelled
/// `V12`, exactly as a symbol source tile would carry it.
const LABELLED_POINT_TILE: &str = "1a460a0c726f7574655f706f696e7473121108011204000001011801220509802080201a046e616d651a056c6162656c220a0a0846414c4c4241434b22050a035631322880207802";

fn hex_bytes(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).expect("hex digit pair"))
        .collect()
}

fn zigzag(value: i64) -> u32 {
    ((value << 1) ^ (value >> 63)) as u32
}

/// The labelled point tile plus an `airways` line across the tile.
fn mixed_tile() -> Vec<u8> {
    let mut tile: Tile =
        Tile::decode(hex_bytes(LABELLED_POINT_TILE).as_slice()).expect("fixture decodes");
    let far = zigzag(4096);
    tile.layers.push(tile::Layer {
        version: 2,
        name: "airways".to_string(),
        features: vec![tile::Feature {
            id: Some(1),
            tags: Vec::new(),
            r#type: Some(tile::GeomType::Linestring as i32),
            geometry: vec![9, 0, 0, 10, far, far],
        }],
        extent: Some(4096),
        ..Default::default()
    });
    tile.encode_to_vec()
}

fn style_layer(json: serde_json::Value) -> StyleLayer {
    serde_json::from_value(json).expect("valid style layer")
}

fn symbol_layer(id: &str, text_field: &str) -> StyleLayer {
    style_layer(serde_json::json!({
        "id": id, "type": "symbol", "source": "chart", "source-layer": "route_points",
        "layout": {"text-field": text_field}
    }))
}

fn processed(tile: &[u8], layer: &StyleLayer) -> ProcessedLayers {
    process_tile_layers(
        tile,
        layer,
        WorldTileCoords::default(),
        ProjectionType::Mercator,
    )
    .expect("tile processes")
}

fn label_texts(layers: &ProcessedLayers) -> Vec<(String, Vec<String>)> {
    layers
        .symbols
        .iter()
        .map(|layer| {
            (
                layer.style_layer_id.clone(),
                layer
                    .features
                    .iter()
                    .map(|feature| feature.str.clone())
                    .collect(),
            )
        })
        .collect()
}

#[test]
fn headless_processing_keeps_the_symbol_bucket_of_a_label_layer() {
    let layers = processed(
        &hex_bytes(LABELLED_POINT_TILE),
        &symbol_layer("label", "{label}"),
    );

    assert!(
        layers.vector.is_empty(),
        "a symbol layer has no vector bucket"
    );
    let [symbols] = layers.symbols.as_slice() else {
        panic!("one symbol bucket, got {}", layers.symbols.len());
    };
    assert_eq!(symbols.style_layer_id, "label");
    assert_eq!(
        symbols.new_buffer.buffer.vertices.len(),
        12,
        "four vertices per glyph of V12"
    );
    assert_eq!(symbols.new_buffer.buffer.indices.len(), 18);
    assert_eq!(
        label_texts(&layers),
        [("label".to_string(), vec!["V12".to_string()])]
    );
}

#[test]
fn vector_and_symbol_buckets_of_mixed_layers_are_kept_apart() {
    let tile = mixed_tile();
    let mut layers = ProcessedLayers::default();
    for layer in [
        style_layer(serde_json::json!({
            "id": "airways", "type": "line", "source": "chart", "source-layer": "airways",
            "paint": {"line-color": "#000000", "line-width": 1}
        })),
        style_layer(serde_json::json!({
            "id": "points", "type": "circle", "source": "chart", "source-layer": "route_points"
        })),
        symbol_layer("label", "{label}"),
    ] {
        layers.append(&mut processed(&tile, &layer));
    }

    let vector_ids: Vec<&str> = layers
        .vector
        .iter()
        .map(|layer| layer.style_layer_id.as_str())
        .collect();
    assert_eq!(vector_ids, ["airways", "points"]);
    assert_eq!(
        label_texts(&layers),
        [("label".to_string(), vec!["V12".to_string()])]
    );
    assert!(!layers.is_empty());
}

#[test]
fn two_symbol_layers_on_one_source_layer_each_keep_their_bucket() {
    let tile = hex_bytes(LABELLED_POINT_TILE);
    let mut layers = processed(&tile, &symbol_layer("label-a", "{label}"));
    layers.append(&mut processed(&tile, &symbol_layer("label-b", "{name}")));

    assert_eq!(
        label_texts(&layers),
        [
            ("label-a".to_string(), vec!["V12".to_string()]),
            ("label-b".to_string(), vec!["FALLBACK".to_string()]),
        ]
    );
}

#[test]
fn geojson_symbol_layers_produce_symbol_buckets() {
    let geojson = serde_json::json!({
        "type": "FeatureCollection",
        "features": [{
            "type": "Feature",
            "geometry": {"type": "Point", "coordinates": [0.0, 0.0]},
            "properties": {"name": "FALLBACK", "label": "V12"}
        }]
    });
    let layer = style_layer(serde_json::json!({
        "id": "label", "type": "symbol", "source": "pts",
        "layout": {"text-field": ["get", "label"]}
    }));

    let layers = process_geojson_layers(
        &geojson,
        "pts",
        vec![layer],
        WorldTileCoords::default(),
        ProjectionType::Mercator,
    )
    .expect("GeoJSON processes");

    assert!(layers.vector.is_empty());
    assert_eq!(
        label_texts(&layers),
        [("label".to_string(), vec!["V12".to_string()])]
    );
}
