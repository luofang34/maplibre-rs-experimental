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

    fn take_sorted(&mut self, entry_count: usize) -> PreparedSpans {
        let mut spans = std::mem::take(&mut self.spans);
        if spans.is_empty() {
            return PreparedSpans::Empty {
                keyed: self.is_keyed(),
            };
        }
        if spans
            .windows(2)
            .all(|pair| matches!(pair, [a, b] if a.key <= b.key))
        {
            let mut keys = vec![0.0; entry_count];
            for span in &spans {
                for entry in span.entries.clone() {
                    if let Some(key) = keys.get_mut(entry) {
                        *key = span.key;
                    }
                }
            }
            return PreparedSpans::InOrder(keys);
        }
        spans.sort_by(|a, b| a.key.total_cmp(&b.key));
        let keys = spans
            .iter()
            .flat_map(|span| span.entries.clone().map(move |_| span.key))
            .collect();
        PreparedSpans::Reordered { keys, spans }
    }
}

enum PreparedSpans {
    Empty { keyed: bool },
    InOrder(Vec<f32>),
    Reordered { keys: Vec<f32>, spans: Vec<Span> },
}

struct Reordered<V, I> {
    vertices: Vec<V>,
    indices: Vec<I>,
    entries: Vec<(
        u32,
        Option<[f32; 4]>,
        Option<crate::vector::feature_paint::BucketFeatureEntry>,
    )>,
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
        match self.sort_key.take_sorted(self.feature_indices.len()) {
            PreparedSpans::Empty { keyed } => {
                if keyed {
                    self.entry_sort_keys = vec![0.0; self.feature_indices.len()];
                }
            }
            PreparedSpans::InOrder(keys) => self.entry_sort_keys = keys,
            PreparedSpans::Reordered { keys, spans } => {
                self.entry_sort_keys = keys;
                self.reorder_by_sort_keys(&spans);
            }
        }
    }

    fn reorder_by_sort_keys(&mut self, spans: &[Span]) {
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
        for span in spans {
            for entry in span.entries.clone() {
                let first = starts.get(entry).copied().unwrap_or(0) as usize;
                let count = self.feature_indices.get(entry).copied().unwrap_or(0) as usize;
                if let Some(slot) = shifts.get_mut(entry) {
                    *slot = out.vertices.len() as i64 - first as i64;
                }
                if let Some(verts) = self.buffer.vertices.get(first..first + count) {
                    out.vertices.extend_from_slice(verts);
                }
                out.entries.push((
                    count as u32,
                    self.feature_colors.get(entry).copied(),
                    self.feature_paint.get(entry).cloned(),
                ));
            }
            let ranges: Vec<(u32, u32, i64)> = span
                .entries
                .clone()
                .map(|entry| {
                    let s = starts.get(entry).copied().unwrap_or(0);
                    let len = self.feature_indices.get(entry).copied().unwrap_or(0);
                    let shift = shifts.get(entry).copied().unwrap_or(0);
                    (s, s + len, shift)
                })
                .collect();
            for &index in self.buffer.indices.get(span.indices.clone()).unwrap_or(&[]) {
                let index: u32 = index.into();
                let shift = ranges
                    .iter()
                    .find(|(from, to, _)| (*from..*to).contains(&index))
                    .map_or(0, |(_, _, shift)| *shift);
                out.indices
                    .push(I::from(VertexId((i64::from(index) + shift) as u32)));
            }
        }
        self.commit_reordered(out);
    }

    fn commit_reordered(&mut self, out: Reordered<crate::render::ShaderVertex, I>) {
        self.buffer = VertexBuffers {
            vertices: out.vertices,
            indices: out.indices,
        };
        self.feature_indices = out.entries.iter().map(|(count, _, _)| *count).collect();
        if !self.feature_colors.is_empty() {
            self.feature_colors = out
                .entries
                .iter()
                .filter_map(|(_, colour, _)| *colour)
                .collect();
        }
        if !self.feature_paint.is_empty() {
            self.feature_paint = out
                .entries
                .into_iter()
                .filter_map(|(_, _, paint)| paint)
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

pub use crate::style::layer::classify::sort_key_name;
