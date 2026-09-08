#![allow(clippy::expect_used, clippy::panic)]
use super::*;
use crate::{
    coords::LatLon,
    render::{
        camera::EyeFrustum,
        view_state::ExternalAnchor,
        xr::{EyeTarget, ScenePlacement, XrEye, XrFrame},
    },
};
use cgmath::{Deg, Matrix4, Rad, SquareMatrix, Vector3};

#[tokio::test]
async fn map_aligned_symbols_crossing_the_eye_plane_cannot_stretch_into_spikes() {
    let mut style = style(0.0, "ground");
    let Some(LayerPaint::Symbol(paint)) = &mut style.layers[2].paint else {
        panic!("paint");
    };
    for prefix in ["text", "icon"] {
        paint
            .properties
            .insert(format!("{prefix}-pitch-alignment"), "map".into());
        paint
            .properties
            .insert(format!("{prefix}-rotation-alignment"), "map".into());
    }
    let layers = layers(&style);
    let mut map = fixture_map(style, layers, 1).await;
    let frame = |distance: f64, height: f64, pitch: f64| XrFrame {
        opaque_environment: true,
        timestamp: std::time::Duration::ZERO,
        placement: ScenePlacement {
            anchor: ExternalAnchor {
                position: LatLon::new(-0.04394530819, 0.0439453125),
                altitude_meters: 1200.0,
            },
            world_from_scene: Matrix4::identity(),
        },
        eyes: vec![XrEye {
            world_from_eye: Matrix4::from_translation(Vector3::new(0.0, -distance, height))
                * Matrix4::from_angle_x(Deg(pitch)),
            frustum: EyeFrustum::symmetric(Rad(1.4), 1.0, 0.05, 1e8),
            target: EyeTarget::default(),
        }],
        request_overscan: 1.0,
        prefetch: None,
    };
    map.run_xr_frame(frame(0.0, 500.0, 0.0))
        .expect("visible frame");
    let red = |pixels: &[u8]| {
        pixels
            .chunks_exact(4)
            .filter(|p| p[0] > 180 && p[1] < 80 && p[2] < 80)
            .count()
    };
    assert!(
        red(&read_blocking(&map)) > 0,
        "exercise a visible label before moving"
    );
    map.run_xr_frame(frame(0.01, 0.06, 90.0))
        .expect("eye-plane crossing");
    let pixels = read_blocking(&map);
    assert_eq!(
        red(&pixels),
        0,
        "a map label spanning the eye plane must be hidden as a whole"
    );
}

#[tokio::test]
async fn collision_priority_and_queries_keep_the_original_feature_id_and_properties() {
    let mut style = style(0.0, "ground");
    let Some(LayerPaint::Symbol(paint)) = &mut style.layers[2].paint else {
        panic!("paint");
    };
    paint
        .properties
        .insert("text-allow-overlap".into(), false.into());
    paint
        .properties
        .insert("symbol-sort-key".into(), serde_json::json!(["get", "rank"]));
    paint.properties.remove("icon-image");
    let source = geozero::mvt::tile::Layer {
        name: "places".into(),
        version: 2,
        extent: Some(4096),
        keys: vec!["rank".into()],
        values: [20.0, 1.0, 10.0]
            .into_iter()
            .map(|value| geozero::mvt::tile::Value {
                double_value: Some(value),
                ..Default::default()
            })
            .collect(),
        features: [95, 93, 94]
            .into_iter()
            .enumerate()
            .map(|(index, id)| geozero::mvt::tile::Feature {
                id: Some(id),
                tags: vec![0, index as u32],
                r#type: Some(1),
                geometry: vec![9, 4096, 4096],
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    };
    let bytes = geozero::mvt::Tile {
        layers: vec![source],
    }
    .encode_to_vec();
    let coords = WorldTileCoords {
        x: 2048,
        y: 2048,
        z: ZoomLevel::from(12),
    };
    let mut processed =
        process_tile_layers(&bytes, &style.layers[1], coords, Default::default()).expect("base");
    processed.append(
        &mut process_tile_layers(&bytes, &style.layers[2], coords, Default::default())
            .expect("symbols"),
    );
    let map = fixture_map(style, processed, 1).await;
    let hits = map.query_rendered_symbols([256.0, 256.0], None);
    assert_eq!(hits.len(), 1, "overlapping labels must have one winner");
    assert_eq!(hits[0].id, Some(93));
    assert_eq!(hits[0].properties["rank"], 1.0);
}
