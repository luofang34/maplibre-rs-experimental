//! Legacy JSON wire representation for symbol features.
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::{
    euclid::{Box2D, Point2D},
    sdf::Feature,
    style::expression::{empty_feature_properties, FeatureProperties, Value},
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
    geometry_type: crate::style::filter::GeometryType,
    #[serde(default)]
    line: Option<LineLabelWire>,
    #[serde(default)]
    anchor_shifts: Vec<[f32; 2]>,
    #[serde(default)]
    text_sets: Vec<[usize; 2]>,
    #[serde(default)]
    anchor_sets: Vec<u8>,
    #[serde(default)]
    text_colors: Vec<([usize; 2], [f32; 4])>,
    #[serde(default)]
    fallback: bool,
    #[serde(default)]
    repeat_key: Option<String>,
}

/// The line a label follows, in a form that survives the worker boundary.
#[derive(Serialize, Deserialize)]
pub struct LineLabelWire {
    polyline: Vec<[f32; 2]>,
    anchor_distance: f32,
    glyph_offsets: Vec<f32>,
    #[serde(default)]
    image_sizes: Vec<[f32; 2]>,
    first_glyph_index: usize,
    #[serde(default)]
    text_offset_y: f32,
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
            geometry_type: feature.data.geometry_type,
            anchor_shifts: feature.anchor_shifts.clone(),
            text_sets: feature
                .text_sets
                .iter()
                .map(|set| [set.start, set.end])
                .collect(),
            anchor_sets: feature.anchor_sets.clone(),
            text_colors: feature
                .text_colors
                .iter()
                .map(|(range, color)| ([range.start, range.end], *color))
                .collect(),
            fallback: feature.fallback,
            repeat_key: feature.repeat_key.as_deref().map(str::to_owned),
            line: feature.line.as_ref().map(|line| LineLabelWire {
                polyline: line.polyline.to_vec(),
                anchor_distance: line.anchor_distance,
                glyph_offsets: line.glyph_offsets.clone(),
                image_sizes: line.image_sizes.clone(),
                first_glyph_index: line.first_glyph_index,
                text_offset_y: line.text_offset_y,
            }),
        }
    }
}

impl SymbolFeature {
    /// Decodes a legacy JSON symbol feature into a runtime collision [`Feature`].
    pub fn into_feature(self) -> Feature {
        let properties = if self.properties.is_empty() {
            Arc::clone(empty_feature_properties())
        } else {
            let map: FeatureProperties = self
                .properties
                .into_iter()
                .map(|(key, value)| (key, Value::from_json(&value)))
                .collect();
            Arc::new(map)
        };
        Feature {
            bbox: Box2D::new(
                Point2D::new(self.bounds[0], self.bounds[1]),
                Point2D::new(self.bounds[2], self.bounds[3]),
            ),
            indices: self.indices[0]..self.indices[1],
            text_anchor: Point2D::new(self.anchor[0], self.anchor[1]),
            str: self.text,
            parts: self.parts,
            data: crate::sdf::SymbolFeatureData {
                id: self.id,
                properties,
                sort_key: self.sort_key,
                geometry_type: self.geometry_type,
                source_feature_index: None,
            },
            anchor_shifts: self.anchor_shifts,
            text_sets: self.text_sets.iter().map(|set| set[0]..set[1]).collect(),
            anchor_sets: self.anchor_sets,
            text_colors: self
                .text_colors
                .into_iter()
                .map(|(range, color)| (range[0]..range[1], color))
                .collect(),
            fallback: self.fallback,
            repeat_key: self.repeat_key.map(Arc::from),
            line: self.line.map(|line| crate::sdf::LineLabel {
                polyline: line.polyline.into(),
                anchor_distance: line.anchor_distance,
                glyph_offsets: line.glyph_offsets,
                image_sizes: line.image_sizes,
                first_glyph_index: line.first_glyph_index,
                text_offset_y: line.text_offset_y,
            }),
        }
    }
}
