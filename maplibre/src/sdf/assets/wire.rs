//! Worker transport for symbol collision geometry and atlases.
use serde::{Deserialize, Serialize};

use crate::{
    euclid::{Box2D, Point2D},
    sdf::Feature,
};

/// Serializable collision geometry for one label and its icon.
#[derive(Serialize, Deserialize)]
pub struct SymbolFeature {
    bounds: [f32; 4],
    indices: [usize; 2],
    anchor: [f32; 2],
    text: String,
    #[serde(default)]
    parts: [Option<crate::sdf::placement_geometry::SymbolBounds>; 3],
    #[serde(default)]
    id: Option<u64>,
    #[serde(default)]
    properties: std::collections::BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    sort_key: f32,
    #[serde(default)]
    line: Option<LineLabelWire>,
    #[serde(default)]
    anchor_shifts: Vec<[f32; 2]>,
    #[serde(default)]
    text_sets: Vec<[usize; 2]>,
    #[serde(default)]
    anchor_sets: Vec<u8>,
}

/// The line a label follows, in a form that survives the worker boundary.
#[derive(Serialize, Deserialize)]
pub struct LineLabelWire {
    polyline: Vec<[f32; 2]>,
    anchor_distance: f32,
    glyph_offsets: Vec<f32>,
    first_glyph_index: usize,
}

impl From<&Feature> for SymbolFeature {
    fn from(feature: &Feature) -> Self {
        Self {
            bounds: [
                feature.bbox.min.x,
                feature.bbox.min.y,
                feature.bbox.max.x,
                feature.bbox.max.y,
            ],
            indices: [feature.indices.start, feature.indices.end],
            anchor: [feature.text_anchor.x, feature.text_anchor.y],
            text: feature.str.clone(),
            parts: feature.parts,
            id: feature.data.id,
            properties: feature
                .data
                .properties
                .iter()
                .map(|(key, value)| (key.clone(), value.to_json()))
                .collect(),
            sort_key: feature.data.sort_key,
            anchor_shifts: feature.anchor_shifts.clone(),
            text_sets: feature
                .text_sets
                .iter()
                .map(|set| [set.start, set.end])
                .collect(),
            anchor_sets: feature.anchor_sets.clone(),
            line: feature.line.as_ref().map(|line| LineLabelWire {
                polyline: line.polyline.to_vec(),
                anchor_distance: line.anchor_distance,
                glyph_offsets: line.glyph_offsets.clone(),
                first_glyph_index: line.first_glyph_index,
            }),
        }
    }
}

impl From<SymbolFeature> for Feature {
    fn from(feature: SymbolFeature) -> Self {
        Self {
            bbox: Box2D::new(
                Point2D::new(feature.bounds[0], feature.bounds[1]),
                Point2D::new(feature.bounds[2], feature.bounds[3]),
            ),
            indices: feature.indices[0]..feature.indices[1],
            text_anchor: Point2D::new(feature.anchor[0], feature.anchor[1]),
            str: feature.text,
            parts: feature.parts,
            data: crate::sdf::SymbolFeatureData {
                id: feature.id,
                properties: feature
                    .properties
                    .into_iter()
                    .map(|(key, value)| (key, crate::style::expression::Value::from_json(&value)))
                    .collect(),
                sort_key: feature.sort_key,
            },
            anchor_shifts: feature.anchor_shifts,
            text_sets: feature.text_sets.iter().map(|set| set[0]..set[1]).collect(),
            anchor_sets: feature.anchor_sets,
            line: feature.line.map(|line| crate::sdf::LineLabel {
                polyline: line.polyline.into(),
                anchor_distance: line.anchor_distance,
                glyph_offsets: line.glyph_offsets,
                first_glyph_index: line.first_glyph_index,
            }),
        }
    }
}
