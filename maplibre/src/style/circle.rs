//! Paint properties of `circle` layers, following the GL JS style specification defaults.

use csscolorparser::Color;
use serde::{Deserialize, Serialize};

use crate::style::layer::{StyleProperty, TranslateAnchor};

/// Whether circles keep their pixel size (`viewport`) or shrink with distance (`map`).
#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default, Eq, PartialEq)]
pub enum CirclePitchScale {
    /// Circles farther from the camera shrink, as if lying on the map.
    #[default]
    #[serde(rename = "map")]
    Map,
    /// Circles keep their pixel size regardless of distance.
    #[serde(rename = "viewport")]
    Viewport,
}

/// Whether circles lie on the map plane (`map`) or face the screen (`viewport`).
#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default, Eq, PartialEq)]
pub enum CirclePitchAlignment {
    /// Circles lie flat on the map plane and foreshorten when pitched.
    #[serde(rename = "map")]
    Map,
    /// Circles face the screen and stay round when pitched.
    #[default]
    #[serde(rename = "viewport")]
    Viewport,
}

/// Paint of a `circle` layer; an absent property means the specification default.
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct CirclePaint {
    /// Radius in screen pixels.
    #[serde(rename = "circle-radius")]
    #[serde(
        default,
        deserialize_with = "StyleProperty::<f32>::deserialize_f32_or_none"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub circle_radius: Option<StyleProperty<f32>>,
    /// Fill colour.
    #[serde(rename = "circle-color")]
    #[serde(
        default,
        deserialize_with = "StyleProperty::<Color>::deserialize_color_or_none"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub circle_color: Option<StyleProperty<Color>>,
    /// Blur as a ratio of the radius; 1 fades the fill out entirely.
    #[serde(rename = "circle-blur")]
    #[serde(
        default,
        deserialize_with = "StyleProperty::<f32>::deserialize_f32_or_none"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub circle_blur: Option<StyleProperty<f32>>,
    /// Fill opacity.
    #[serde(rename = "circle-opacity")]
    #[serde(
        default,
        deserialize_with = "StyleProperty::<f32>::deserialize_f32_or_none"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub circle_opacity: Option<StyleProperty<f32>>,
    /// Stroke width in screen pixels, drawn outside the radius.
    #[serde(rename = "circle-stroke-width")]
    #[serde(
        default,
        deserialize_with = "StyleProperty::<f32>::deserialize_f32_or_none"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub circle_stroke_width: Option<StyleProperty<f32>>,
    /// Stroke colour.
    #[serde(rename = "circle-stroke-color")]
    #[serde(
        default,
        deserialize_with = "StyleProperty::<Color>::deserialize_color_or_none"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub circle_stroke_color: Option<StyleProperty<Color>>,
    /// Stroke opacity.
    #[serde(rename = "circle-stroke-opacity")]
    #[serde(
        default,
        deserialize_with = "StyleProperty::<f32>::deserialize_f32_or_none"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub circle_stroke_opacity: Option<StyleProperty<f32>>,
    /// Translation in screen pixels before conversion to tile units.
    #[serde(rename = "circle-translate", default)]
    pub circle_translate: Option<[f32; 2]>,
    /// Whether the translation follows the map or the viewport.
    #[serde(rename = "circle-translate-anchor", default)]
    pub circle_translate_anchor: TranslateAnchor,
    /// Whether circles shrink with distance on a pitched map.
    #[serde(rename = "circle-pitch-scale", default)]
    pub circle_pitch_scale: CirclePitchScale,
    /// Whether circles lie on the map plane or face the screen.
    #[serde(rename = "circle-pitch-alignment", default)]
    pub circle_pitch_alignment: CirclePitchAlignment,
}

impl CirclePaint {
    /// Radius in pixels when the style gives none.
    pub const DEFAULT_RADIUS: f32 = 5.0;

    /// Radius property with the specification default filled in.
    pub fn radius(&self) -> StyleProperty<f32> {
        self.circle_radius
            .clone()
            .unwrap_or(StyleProperty::Constant(Self::DEFAULT_RADIUS))
    }

    /// Stroke width property with the specification default filled in.
    pub fn stroke_width(&self) -> StyleProperty<f32> {
        self.circle_stroke_width
            .clone()
            .unwrap_or(StyleProperty::Constant(0.0))
    }

    /// Fill opacity at a zoom; data-driven values fall back to opaque.
    pub fn opacity_at(&self, zoom: f64) -> f32 {
        number_at(self.circle_opacity.as_ref(), zoom, 1.0)
    }

    /// Stroke opacity at a zoom.
    pub fn stroke_opacity_at(&self, zoom: f64) -> f32 {
        number_at(self.circle_stroke_opacity.as_ref(), zoom, 1.0)
    }

    /// Blur as a ratio of the radius at a zoom.
    pub fn blur_at(&self, zoom: f64) -> f32 {
        number_at(self.circle_blur.as_ref(), zoom, 0.0)
    }

    /// Stroke colour as straight (non-premultiplied) RGBA; black when unset or data-driven.
    pub fn stroke_color_rgba(&self) -> [f32; 4] {
        match &self.circle_stroke_color {
            Some(StyleProperty::Constant(color)) => [
                color.r as f32,
                color.g as f32,
                color.b as f32,
                color.a as f32,
            ],
            _ => [0.0, 0.0, 0.0, 1.0],
        }
    }
}

fn number_at(property: Option<&StyleProperty<f32>>, zoom: f64, default: f32) -> f32 {
    property
        .and_then(|property| property.evaluate_number(&Default::default(), zoom))
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::panic)]

    use super::{CirclePaint, CirclePitchAlignment, CirclePitchScale};
    use crate::style::layer::{LayerPaint, StyleLayer, StyleProperty};

    fn circle_layer(json: serde_json::Value) -> CirclePaint {
        let layer: StyleLayer = serde_json::from_value(json).expect("layer parses");
        match layer.paint {
            Some(LayerPaint::Circle(paint)) => paint,
            other => panic!("expected circle paint, got {other:?}"),
        }
    }

    #[test]
    fn a_circle_layer_keeps_its_paint() {
        let paint = circle_layer(serde_json::json!({
            "id": "points", "type": "circle", "source": "chart", "source-layer": "route_points",
            "paint": {"circle-color": "#ff0000", "circle-radius": 3, "circle-stroke-width": 2,
                      "circle-stroke-color": "#00ff00", "circle-pitch-alignment": "map",
                      "circle-pitch-scale": "viewport", "circle-blur": 0.5}
        }));

        assert!(
            matches!(paint.circle_radius, Some(StyleProperty::Constant(radius)) if radius == 3.0)
        );
        assert!(matches!(paint.radius(), StyleProperty::Constant(radius) if radius == 3.0));
        assert!(matches!(paint.stroke_width(), StyleProperty::Constant(width) if width == 2.0));
        assert_eq!(paint.stroke_color_rgba(), [0.0, 1.0, 0.0, 1.0]);
        assert_eq!(paint.circle_pitch_alignment, CirclePitchAlignment::Map);
        assert_eq!(paint.circle_pitch_scale, CirclePitchScale::Viewport);
        assert_eq!(paint.blur_at(4.0), 0.5);
        assert_eq!(paint.opacity_at(4.0), 1.0);
    }

    #[test]
    fn empty_and_absent_paint_fall_back_to_the_specification_defaults() {
        for json in [
            serde_json::json!({"id": "a", "type": "circle", "source": "s", "paint": {}}),
            serde_json::json!({"id": "b", "type": "circle", "source": "s"}),
        ] {
            let paint = circle_layer(json);
            assert!(matches!(paint.radius(), StyleProperty::Constant(radius) if radius == 5.0));
            assert!(matches!(paint.stroke_width(), StyleProperty::Constant(width) if width == 0.0));
            assert_eq!(paint.circle_pitch_scale, CirclePitchScale::Map);
            assert_eq!(paint.circle_pitch_alignment, CirclePitchAlignment::Viewport);
            assert_eq!(paint.stroke_color_rgba(), [0.0, 0.0, 0.0, 1.0]);
        }
    }

    #[test]
    fn zoom_driven_paint_values_evaluate_at_the_zoom() {
        let paint = circle_layer(serde_json::json!({
            "id": "a", "type": "circle", "source": "s",
            "paint": {"circle-opacity": {"stops": [[2, 0.2], [4, 0.8]]},
                      "circle-radius": ["interpolate", ["linear"], ["zoom"], 0, 2, 10, 12]}
        }));
        assert!((paint.opacity_at(3.0) - 0.5).abs() < 1e-6);
        assert!(
            (paint
                .radius()
                .evaluate_number(&Default::default(), 5.0)
                .expect("radius evaluates")
                - 7.0)
                .abs()
                < 1e-6
        );
    }
}
