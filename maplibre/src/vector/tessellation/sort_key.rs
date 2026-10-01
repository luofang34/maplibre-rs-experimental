//! Feature draw order from `fill-sort-key`, `line-sort-key` and `circle-sort-key`.

use std::ops::Range;

use lyon::tessellation::{geometry_builder::MaxIndex, VertexBuffers, VertexId};

use super::ZeroTessellator;
use crate::style::{expression::FeatureProperties, layer::StyleProperty};

/// Where one feature's geometry sits in the tessellator's buffers, and how it ranks.
struct Span {
    key: f32,
    entries: Range<usize>,
    indices: Range<usize>,
}

/// Collects the sort key of each feature as it is tessellated.
#[derive(Default)]
pub struct SortKeys {
    property: Option<StyleProperty<f32>>,
    spans: Vec<Span>,
    indices_end: usize,
}

impl SortKeys {
    /// Whether a property ranks the features, rather than none or a grouping alone.
    pub fn is_keyed(&self) -> bool {
        self.property.is_some()
    }

    /// Ranks features by `property`; a feature without a key ranks as zero.
    pub fn by(property: Option<StyleProperty<f32>>) -> Self {
        Self {
            property,
            ..Self::default()
        }
    }

    pub(super) fn record(
        &mut self,
        properties: &FeatureProperties,
        zoom: f64,
        entries: Range<usize>,
        (indices_end, grouped_by): (usize, Option<f32>),
    ) {
        let key = match (&self.property, grouped_by) {
            (_, Some(group)) => group,
            (Some(property), None) => property.evaluate_for(properties, zoom).unwrap_or(0.0),
            (None, None) => return,
        };
        self.spans.push(Span {
            key,
            entries,
            indices: self.indices_end..indices_end,
        });
        self.indices_end = indices_end;
    }
}

struct Reordered<V, I> {
    vertices: Vec<V>,
    indices: Vec<I>,
    entries: Vec<(u32, Option<[f32; 4]>)>,
}

impl<I> ZeroTessellator<I>
where
    I: std::ops::Add + From<VertexId> + MaxIndex + Copy + Into<u32>,
{
    /// The sort key of each entry of `feature_indices` when a property ranks the features;
    /// empty otherwise.
    pub fn sort_key_values(&mut self) -> Vec<f32> {
        if self.sort_key.is_keyed() {
            std::mem::take(&mut self.entry_sort_keys)
        } else {
            Vec::new()
        }
    }

    /// Reorders the finished features so that greater sort keys draw later, keeping the order
    /// of equal keys.
    pub fn apply_sort_keys(&mut self) {
        let mut spans = std::mem::take(&mut self.sort_key.spans);
        self.entry_sort_keys = vec![0.0; self.feature_indices.len()];
        if spans.windows(2).all(|pair| pair[0].key <= pair[1].key) {
            for span in &spans {
                for entry in span.entries.clone() {
                    if let Some(key) = self.entry_sort_keys.get_mut(entry) {
                        *key = span.key;
                    }
                }
            }
            return;
        }
        spans.sort_by(|a, b| a.key.total_cmp(&b.key));
        self.entry_sort_keys = spans
            .iter()
            .flat_map(|span| span.entries.clone().map(move |_| span.key))
            .collect();
        let mut starts = Vec::with_capacity(self.feature_indices.len());
        let mut start = 0_u32;
        for count in &self.feature_indices {
            starts.push(start);
            start = start.wrapping_add(*count);
        }
        let mut out = Reordered {
            vertices: Vec::with_capacity(self.buffer.vertices.len()),
            indices: Vec::with_capacity(self.buffer.indices.len()),
            entries: Vec::with_capacity(self.feature_indices.len()),
        };
        // Each entry's vertices land contiguously, so the index shift is one number per entry.
        let mut shifts = vec![0_i64; self.feature_indices.len()];
        for span in &spans {
            for entry in span.entries.clone() {
                let (first, count) = (starts[entry] as usize, self.feature_indices[entry] as usize);
                shifts[entry] = out.vertices.len() as i64 - first as i64;
                out.vertices
                    .extend_from_slice(&self.buffer.vertices[first..first + count]);
                out.entries
                    .push((count as u32, self.feature_colors.get(entry).copied()));
            }
            let entries = span.entries.clone();
            let ranges: Vec<(u32, u32, i64)> = entries
                .map(|entry| {
                    (
                        starts[entry],
                        starts[entry] + self.feature_indices[entry],
                        shifts[entry],
                    )
                })
                .collect();
            for index in &self.buffer.indices[span.indices.clone()] {
                let index: u32 = (*index).into();
                let shift = ranges
                    .iter()
                    .find(|(from, to, _)| (*from..*to).contains(&index))
                    .map_or(0, |(_, _, shift)| *shift);
                out.indices
                    .push(I::from(VertexId((i64::from(index) + shift) as u32)));
            }
        }
        self.buffer = VertexBuffers {
            vertices: out.vertices,
            indices: out.indices,
        };
        self.feature_indices = out.entries.iter().map(|(count, _)| *count).collect();
        if !self.feature_colors.is_empty() {
            self.feature_colors = out
                .entries
                .iter()
                .filter_map(|(_, colour)| *colour)
                .collect();
        }
    }
}

/// The `*-sort-key` layout property of a fill, line or circle layer.
pub fn sort_key_of(layer: &crate::style::layer::StyleLayer) -> Option<StyleProperty<f32>> {
    let name = sort_key_name(&layer.type_)?;
    layer
        .unrecognized
        .layout
        .get(name)
        .map(StyleProperty::parse)
}

/// The sort-key property a layer type accepts, if any.
pub fn sort_key_name(layer_type: &str) -> Option<&'static str> {
    match layer_type {
        "fill" => Some("fill-sort-key"),
        "line" => Some("line-sort-key"),
        "circle" => Some("circle-sort-key"),
        _ => None,
    }
}
