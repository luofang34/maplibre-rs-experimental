//! The order a layer's labels are drawn in: by their height on the screen, as GL JS draws labels
//! that may overlap, so the lower of two overlapping labels covers the other.
use lyon::tessellation::VertexBuffers;

use crate::{
    render::shaders::ShaderSymbolVertex,
    sdf::Feature,
    style::{
        expression::FeatureProperties,
        layer::{StyleProperty, SymbolPaint},
    },
};

/// A label's height on the screen turned by `bearing`, as GL JS `sortFeatures` measures it:
/// from its anchor in whole units of 8192-unit tiles.
pub(crate) fn rotated_height(anchor: [f32; 2], bearing: f64) -> i64 {
    let angle = -bearing;
    let [x, y] = anchor.map(|value| (f64::from(value) * 2.0).round());
    (angle.sin() * x + angle.cos() * y).round() as i64
}

/// The order a layer's labels are drawn in at `bearing`: by rotated height, equal heights the
/// later label first, and a second way of writing a label with the label it belongs to.
pub(crate) fn drawn_order(features: &[Feature], bearing: f64) -> Vec<usize> {
    let mut group = 0;
    let groups: Vec<usize> = features
        .iter()
        .enumerate()
        .map(|(index, feature)| {
            if !feature.fallback {
                group = index;
            }
            group
        })
        .collect();
    let mut order: Vec<(usize, i64, usize)> = features
        .iter()
        .enumerate()
        .map(|(index, feature)| {
            let anchor = feature.text_anchor;
            let height = rotated_height([anchor.x, anchor.y], bearing);
            let grp = groups.get(index).copied().unwrap_or(0);
            (index, height, grp)
        })
        .collect();
    order.sort_by_key(|&(_, height, grp)| (height, std::cmp::Reverse(grp)));
    order.into_iter().map(|(index, _, _)| index).collect()
}

/// Whether the layer draws its labels by screen height: `symbol-z-order: viewport-y`, or `auto`
/// without a `symbol-sort-key`, for labels that may overlap.
pub(crate) fn sorts_by_height(paint: &SymbolPaint, zoom: f64) -> bool {
    let order = paint.layout_enum("symbol-z-order", zoom);
    let by_height = match order.as_deref().unwrap_or("auto") {
        "viewport-y" => true,
        "auto" => !paint.properties.contains_key("symbol-sort-key"),
        _ => false,
    };
    by_height && may_overlap(paint, zoom)
}

fn may_overlap(paint: &SymbolPaint, zoom: f64) -> bool {
    let flag = |name: &str| {
        paint
            .properties
            .get(name)
            .and_then(|value| {
                StyleProperty::<bool>::parse(value)
                    .evaluate_for(&FeatureProperties::default(), zoom)
            })
            .unwrap_or(false)
    };
    ["text", "icon"].into_iter().any(|prefix| {
        flag(&format!("{prefix}-allow-overlap"))
            || flag(&format!("{prefix}-ignore-placement"))
            || paint
                .layout_enum(&format!("{prefix}-overlap"), zoom)
                .is_some_and(|mode| mode != "never")
    })
}

/// Rewrites the index buffer so that the labels draw from the top of the screen down, equal
/// heights later labels first, as GL JS sorts them. The labels themselves keep their order,
/// which placement follows. A label that is the second way of writing the one before it stays
/// with it.
pub(super) fn reorder(
    buffer: &mut VertexBuffers<ShaderSymbolVertex, u32>,
    features: &mut [Feature],
) {
    let mut spans: Vec<_> = features
        .iter()
        .map(|feature| feature.indices.clone())
        .collect();
    spans.sort_by_key(|span| span.start);
    let covered: usize = spans.iter().map(|span| span.len()).sum();
    let contiguous = spans
        .windows(2)
        .all(|pair| matches!(pair, [a, b] if a.end == b.start));
    if covered != buffer.indices.len() || !contiguous || features.len() < 2 {
        return;
    }
    let mut group = 0;
    let groups: Vec<usize> = features
        .iter()
        .enumerate()
        .map(|(index, feature)| {
            if !feature.fallback {
                group = index;
            }
            group
        })
        .collect();
    let mut order: Vec<(usize, i64, usize)> = features
        .iter()
        .enumerate()
        .map(|(index, feature)| {
            let height = (feature.text_anchor.y * 2.0).round() as i64;
            let grp = groups.get(index).copied().unwrap_or(0);
            (index, height, grp)
        })
        .collect();
    order.sort_by_key(|&(_, height, grp)| (height, std::cmp::Reverse(grp)));
    let mut indices = Vec::with_capacity(buffer.indices.len());
    for (index, _, _) in order {
        let Some(feature) = features.get_mut(index) else {
            continue;
        };
        let Some(slice) = buffer.indices.get(feature.indices.clone()) else {
            continue;
        };
        let start = indices.len();
        indices.extend_from_slice(slice);
        let (old, shift) = (
            feature.indices.start,
            start as isize - feature.indices.start as isize,
        );
        let moved = |at: usize| (at as isize + shift) as usize;
        feature.indices = start..start + (feature.indices.end - old);
        for set in &mut feature.text_sets {
            *set = moved(set.start)..moved(set.end);
        }
        for (range, _) in &mut feature.text_colors {
            *range = moved(range.start)..moved(range.end);
        }
        if let Some(line) = &mut feature.line {
            line.first_glyph_index = moved(line.first_glyph_index);
        }
    }
    buffer.indices = indices;
}

/// Worker-side configuration applied to a [`super::TextTessellator`] before streaming features.
#[derive(Default)]
pub(crate) struct TextTessellationInput {
    pub coordinate_scale: f64,
    pub overscaling: f64,
    pub source_ids: Vec<Option<u64>>,
    pub source_indices: Vec<Option<u32>>,
    pub shared_properties: Vec<std::sync::Arc<FeatureProperties>>,
    pub feature_state_ctx: Option<crate::style::feature_state::FeatureStateSyncContext>,
    pub stream_point_runtime_keys: Option<Option<rustc_hash::FxHashSet<String>>>,
}

/// Finalized symbol vertex buffer and collision features produced by [`super::TextTessellator`].
pub(crate) struct TextTessellationOutput {
    pub buffer: crate::render::resource::vertex_buffer::OverAlignedVertexBuffer<
        ShaderSymbolVertex,
        crate::render::resource::vertex_buffer::IndexDataType,
    >,
    pub features: Vec<Feature>,
    pub has_geometry: bool,
}

impl super::TextTessellator {
    /// Applies coordinate scaling, source IDs, shared properties, and streaming configuration.
    pub(crate) fn configure_input(&mut self, input: TextTessellationInput) {
        if input.coordinate_scale > 0.0 {
            self.coordinate_scale = input.coordinate_scale;
        }
        if input.overscaling > 0.0 {
            self.overscaling = input.overscaling;
        }
        self.source_ids = input.source_ids;
        self.source_indices = input.source_indices;
        self.shared_properties = input.shared_properties;
        self.feature_state_ctx = input.feature_state_ctx;
        self.stream_point_runtime_keys = input.stream_point_runtime_keys;
    }

    /// Completes line merging, optional runtime property pruning, and buffer shrinking.
    pub(crate) fn finish_output(
        mut self,
        commit_bare_geometry: bool,
        prune: Option<(&crate::style::layer::StyleLayer, Option<&str>)>,
    ) -> TextTessellationOutput {
        if commit_bare_geometry && self.features.is_empty() {
            drop(geozero::FeatureProcessor::feature_end(&mut self, 0));
        }
        self.finish();
        if let Some((style, promote_prop)) = prune
            && self.stream_point_runtime_keys.is_none()
        {
            super::prune::prune_symbol_features(&mut self.features, style, promote_prop);
        }
        self.quad_buffer.vertices.shrink_to_fit();
        self.quad_buffer.indices.shrink_to_fit();
        self.features.shrink_to_fit();
        let has_geometry = !self.quad_buffer.indices.is_empty();
        TextTessellationOutput {
            buffer: self.quad_buffer.into(),
            features: self.features,
            has_geometry,
        }
    }
}

#[cfg(test)]
mod tests;
