//! Line width, offset, gap width and blur evaluated per feature and packed into the stroke's
//! vertices.
use crate::style::{
    expression::FeatureProperties,
    layer::{LinePaint, StyleProperty},
};

/// Exponent bits of a normal float, so the packed payload survives the vertex fetch as one.
const NORMAL_EXPONENT: u32 = 0x4000_0000;
/// Exponent bits of a float below -1e20, which is how a stroke vertex says it has no elevation.
const STROKE_EXPONENT: u32 = 0xE100_0000;
const OFFSET_BITS: u32 = 11;
const OFFSET_BIAS: i32 = 1 << (OFFSET_BITS - 1);
const WIDTH_BITS: u32 = 12;
/// Sixteenths of a pixel for the width, eighths for the offset, the gap and the blur.
const WIDTH_STEPS: f32 = 16.0;
const OFFSET_STEPS: f32 = 8.0;
const DETAIL_BITS: u32 = 11;
/// A gap or blur of this value is the layer's own.
const DETAIL_UNSET: u32 = (1 << DETAIL_BITS) - 1;

/// What a feature's line vertices carry: its width and offset, and its gap width and blur.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PackedLine {
    /// Width and offset, in the `edge_distance` slot.
    pub style: f32,
    /// Gap width and blur, in the low bits of a stroke's elevation slot.
    pub detail: u32,
}

impl PackedLine {
    /// What a line carries when nothing varies by feature.
    pub const LAYER: Self = Self {
        style: 0.0,
        detail: (DETAIL_UNSET << DETAIL_BITS) | DETAIL_UNSET,
    };

    /// The elevation slot of a stroke vertex: the side of the line it is on and the detail.
    pub fn stroke_elevation(self, negative_side: bool) -> f32 {
        f32::from_bits(
            STROKE_EXPONENT | (u32::from(negative_side) << (2 * DETAIL_BITS)) | self.detail,
        )
    }
}

/// A layer's width, offset, gap width and blur, evaluated for each feature at `zoom`.
#[derive(Clone)]
pub struct LineFeatureStyle {
    width: Option<StyleProperty<f32>>,
    offset: Option<StyleProperty<f32>>,
    gap_width: Option<StyleProperty<f32>>,
    blur: Option<StyleProperty<f32>>,
    zoom: f64,
}

fn varies(property: &Option<StyleProperty<f32>>) -> bool {
    property
        .as_ref()
        .is_some_and(|property| !property.is_feature_constant())
}

impl LineFeatureStyle {
    /// The style for a layer where one of the four varies by feature; `None` when none does
    /// and the layer's own values serve.
    pub fn for_paint(paint: &LinePaint, zoom: f64) -> Option<Self> {
        let any = varies(&paint.line_width)
            || varies(&paint.line_offset)
            || varies(&paint.line_gap_width)
            || varies(&paint.line_blur);
        any.then(|| Self {
            width: paint.line_width.clone(),
            offset: paint.line_offset.clone(),
            gap_width: paint.line_gap_width.clone(),
            blur: paint.line_blur.clone(),
            zoom,
        })
    }

    /// What the vertices of one feature carry.
    pub fn pack(&self, properties: &FeatureProperties) -> PackedLine {
        let value = |property: &Option<StyleProperty<f32>>, default: f32| {
            property
                .as_ref()
                .and_then(|property| property.evaluate_for(properties, self.zoom))
                .filter(|value| value.is_finite())
                .unwrap_or(default)
        };
        let width = (value(&self.width, 1.0).max(0.0) * WIDTH_STEPS).round();
        let offset = (value(&self.offset, 0.0) * OFFSET_STEPS).round();
        let width = (width as u32).min((1 << WIDTH_BITS) - 1);
        let offset = (offset as i32 + OFFSET_BIAS).clamp(0, (1 << OFFSET_BITS) - 1) as u32;
        let detail = |property: &Option<StyleProperty<f32>>| {
            ((value(property, 0.0).max(0.0) * OFFSET_STEPS).round() as u32).min(DETAIL_UNSET - 1)
        };
        PackedLine {
            style: f32::from_bits(NORMAL_EXPONENT | (width << OFFSET_BITS) | offset),
            detail: (detail(&self.gap_width) << DETAIL_BITS) | detail(&self.blur),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]
    use super::*;

    fn paint(json: serde_json::Value) -> LinePaint {
        serde_json::from_value(json).expect("line paint")
    }

    #[test]
    fn a_features_width_offset_gap_and_blur_survive_the_packing() {
        let style = LineFeatureStyle::for_paint(
            &paint(serde_json::json!({
                "line-width": ["get", "w"], "line-offset": -12.5,
                "line-gap-width": 3, "line-blur": ["get", "w"]
            })),
            3.0,
        )
        .expect("width varies");
        let properties = FeatureProperties::from([(
            "w".to_string(),
            crate::style::expression::Value::Number(6.25),
        )]);
        let packed = style.pack(&properties);
        let bits = packed.style.to_bits();
        let offset = (bits & ((1 << OFFSET_BITS) - 1)) as i32 - OFFSET_BIAS;
        let width = (bits >> OFFSET_BITS) & ((1 << WIDTH_BITS) - 1);
        assert_eq!(offset as f32 / OFFSET_STEPS, -12.5);
        assert_eq!(width as f32 / WIDTH_STEPS, 6.25);
        assert!(packed.style.is_normal());
        assert_eq!((packed.detail >> DETAIL_BITS) as f32 / OFFSET_STEPS, 3.0);
        assert_eq!((packed.detail & DETAIL_UNSET) as f32 / OFFSET_STEPS, 6.25);
        let elevation = packed.stroke_elevation(true);
        assert!(elevation < -1e20 && elevation.is_normal());
    }

    #[test]
    fn a_layer_where_nothing_varies_needs_no_packing() {
        let style = LineFeatureStyle::for_paint(&paint(serde_json::json!({"line-width": 4})), 1.0);
        assert!(style.is_none());
    }
}
