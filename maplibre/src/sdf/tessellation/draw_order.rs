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
    let mut order: Vec<usize> = (0..features.len()).collect();
    let height = |index: usize| {
        let anchor = features[index].text_anchor;
        rotated_height([anchor.x, anchor.y], bearing)
    };
    order.sort_by_key(|index| (height(*index), std::cmp::Reverse(groups[*index])));
    order
}

/// Whether the layer draws its labels by screen height: `symbol-z-order: viewport-y`, or `auto`
/// without a `symbol-sort-key`, for labels that may overlap.
pub(crate) fn sorts_by_height(paint: &SymbolPaint, zoom: f64) -> bool {
    let order = paint
        .properties
        .get("symbol-z-order")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("auto");
    let by_height = match order {
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
                StyleProperty::<bool>::parse(value).evaluate_for(&FeatureProperties::new(), zoom)
            })
            .unwrap_or(false)
    };
    ["text", "icon"].into_iter().any(|prefix| {
        flag(&format!("{prefix}-allow-overlap"))
            || flag(&format!("{prefix}-ignore-placement"))
            || paint
                .properties
                .get(&format!("{prefix}-overlap"))
                .and_then(serde_json::Value::as_str)
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
    let contiguous = spans.windows(2).all(|pair| pair[0].end == pair[1].start);
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
    let mut order: Vec<usize> = (0..features.len()).collect();
    let height = |index: usize| (features[index].text_anchor.y * 2.0).round() as i64;
    order.sort_by_key(|index| (height(*index), std::cmp::Reverse(groups[*index])));
    let mut indices = Vec::with_capacity(buffer.indices.len());
    for index in order {
        let feature = &mut features[index];
        let start = indices.len();
        indices.extend_from_slice(&buffer.indices[feature.indices.clone()]);
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

#[cfg(test)]
mod tests;
