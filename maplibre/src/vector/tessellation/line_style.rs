//! Line width and offset evaluated per feature and packed into the stroke's vertices.
use crate::style::{expression::FeatureProperties, layer::StyleProperty};

/// Exponent bits of a normal float, so the packed payload survives the vertex fetch as one.
const NORMAL_EXPONENT: u32 = 0x4000_0000;
const OFFSET_BITS: u32 = 11;
const OFFSET_BIAS: i32 = 1 << (OFFSET_BITS - 1);
const WIDTH_BITS: u32 = 12;
/// Sixteenths of a pixel for the width, eighths for the offset.
const WIDTH_STEPS: f32 = 16.0;
const OFFSET_STEPS: f32 = 8.0;

/// A layer's width and offset properties, evaluated for each feature at `zoom`.
#[derive(Clone)]
pub struct LineFeatureStyle {
    /// The `line-width` property.
    pub width: Option<StyleProperty<f32>>,
    /// The `line-offset` property.
    pub offset: Option<StyleProperty<f32>>,
    /// Zoom the properties are evaluated at.
    pub zoom: f64,
}

impl LineFeatureStyle {
    /// The style for a layer whose width or offset varies by feature; `None` when both are
    /// the same for every feature and the layer's own values serve.
    pub fn for_paint(
        width: &Option<StyleProperty<f32>>,
        offset: &Option<StyleProperty<f32>>,
        zoom: f64,
    ) -> Option<Self> {
        let varies = |property: &Option<StyleProperty<f32>>| {
            property
                .as_ref()
                .is_some_and(|property| !property.is_feature_constant())
        };
        (varies(width) || varies(offset)).then(|| Self {
            width: width.clone(),
            offset: offset.clone(),
            zoom,
        })
    }

    /// The packed width and offset of one feature, as the float its vertices carry.
    pub fn pack(&self, properties: &FeatureProperties) -> f32 {
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
        f32::from_bits(NORMAL_EXPONENT | (width << OFFSET_BITS) | offset)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]
    use super::*;

    #[test]
    fn a_features_width_and_offset_survive_the_packing() {
        let style = LineFeatureStyle {
            width: Some(serde_json::from_value(serde_json::json!(["get", "w"])).expect("width")),
            offset: Some(serde_json::from_value(serde_json::json!(-12.5)).expect("offset")),
            zoom: 3.0,
        };
        let properties = FeatureProperties::from([(
            "w".to_string(),
            crate::style::expression::Value::Number(6.25),
        )]);
        let bits = style.pack(&properties).to_bits();
        let offset = (bits & ((1 << OFFSET_BITS) - 1)) as i32 - OFFSET_BIAS;
        let width = (bits >> OFFSET_BITS) & ((1 << WIDTH_BITS) - 1);
        assert_eq!(offset as f32 / OFFSET_STEPS, -12.5);
        assert_eq!(width as f32 / WIDTH_STEPS, 6.25);
        assert!(style.pack(&properties).is_normal());
    }
}
