//! Labels toward the horizon of a pitched map stop where the camera would draw them at less
//! than GL JS's perspective-ratio cutoff of their size, rather than crowding it.

use std::sync::Arc;

use super::{SymbolMap, FONT, GLYPHS, SPRITE};
use crate::style::{source::GeoJsonData, Style};

/// A column of labels on the meridian the view looks along, from the centre into the distance.
fn column() -> serde_json::Value {
    let features: Vec<serde_json::Value> = (0..57)
        .map(|step| {
            serde_json::json!({"type":"Feature","properties":{},
                "geometry":{"type":"Point","coordinates":[-60.0, f64::from(step) * 1.5]}})
        })
        .collect();
    serde_json::json!({"type":"FeatureCollection","features":features})
}

fn style() -> Style {
    serde_json::from_value(serde_json::json!({
        "version":8,"center":[-60,0],"zoom":5,"pitch":0,
        "glyphs":GLYPHS,"sprite":SPRITE,
        "sources":{"places":{"type":"geojson","data":{"type":"FeatureCollection","features":[]}}},
        "layers":[
            {"id":"background","type":"background","paint":{"background-color":"#000000"}},
            {"id":"labels","type":"symbol","source":"places",
                "layout":{"text-field":"Wide","text-font":[FONT],"text-size":16,"text-allow-overlap":true},
                "paint":{"text-color":"#ffffff"}}
        ]
    }))
    .expect("style")
}

/// The heights of the placed labels' boxes, which a viewport label scales by its perspective
/// ratio.
fn placed_heights(map: &SymbolMap) -> Vec<f64> {
    map.map
        .map_context
        .world
        .resources
        .get::<crate::sdf::query::PlacedSymbols>()
        .map(|placed| {
            placed
                .0
                .iter()
                .filter_map(|symbol| symbol.rectangles[0])
                .map(|rect| rect[3] - rect[1])
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn labels_toward_the_horizon_stop_at_the_perspective_ratio_cutoff() {
    let mut map = SymbolMap::new(style()).await;
    map.map
        .map_context
        .set_geojson_data("places", GeoJsonData::Inline(Arc::new(column())))
        .expect("set data");
    // Flat, every label is drawn at its own size, which the pitched view divides by.
    map.settle().await;
    let flat = placed_heights(&map);
    let full = flat.iter().copied().fold(0.0, f64::max);
    assert!(full > 0.0, "labels are placed flat: {flat:?}");
    map.map.set_max_pitch(cgmath::Deg(85.0));
    map.map
        .view_state_mut()
        .camera_mut()
        .set_pitch(cgmath::Deg(75.0));
    map.settle().await;
    let ratios: Vec<f64> = placed_heights(&map).iter().map(|h| h / full).collect();
    let nearest = ratios.iter().copied().fold(0.0, f64::max);
    assert!(
        nearest > 0.9,
        "the label at the centre keeps its size: {ratios:.2?}"
    );
    let farthest = ratios.iter().copied().fold(f64::INFINITY, f64::min);
    assert!(
        (0.59..0.7).contains(&farthest),
        "labels reach the cutoff and none is placed beyond it: {ratios:.2?}"
    );
}
