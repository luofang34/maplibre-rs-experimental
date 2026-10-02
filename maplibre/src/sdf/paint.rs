//! Uniform values for readable text and sprite rendering.
use bytemuck_derive::{Pod, Zeroable};

use crate::style::{
    expression::FeatureProperties,
    layer::{StyleProperty, SymbolPaint},
};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, PartialEq)]
pub(super) struct SymbolUniforms {
    pub text_color: [f32; 4],
    pub halo_color: [f32; 4],
    pub icon_color: [f32; 4],
    pub icon_halo_color: [f32; 4],
    pub text: [f32; 4],
    pub icon: [f32; 4],
    pub text_layout: [f32; 4],
    pub icon_layout: [f32; 4],
    pub atlas: [f32; 4],
    pub placement: [f32; 4],
    /// Shift in screen pixels of text (x, y) and icons (z, w) whose translation follows the
    /// viewport.
    pub translate: [f32; 4],
}

/// Whether text along a line is placed glyph by glyph: it lies on the map plane and turns with
/// the map. Other alignments keep the straight layout at the anchor.
pub(crate) fn text_follows_line(paint: &SymbolPaint, zoom: f64) -> bool {
    let layout = SymbolUniforms::new(paint, zoom, [1, 1]).text_layout;
    layout[0] > 0.5 && layout[1] > 0.5
}

impl SymbolUniforms {
    pub fn new(paint: &SymbolPaint, zoom: f64, size: [u32; 2]) -> Self {
        let properties = FeatureProperties::new();
        let number = |name, fallback| paint.number(name, &properties, zoom, fallback);
        Self {
            text_color: color(paint, "text-color", zoom, [0.0, 0.0, 0.0, 1.0]),
            halo_color: color(paint, "text-halo-color", zoom, [0.0; 4]),
            icon_color: color(paint, "icon-color", zoom, [0.0, 0.0, 0.0, 1.0]),
            icon_halo_color: color(paint, "icon-halo-color", zoom, [0.0; 4]),
            text: [
                paint
                    .text_size
                    .as_ref()
                    .and_then(|value| value.evaluate_at_zoom(zoom))
                    .unwrap_or(16.0),
                number("text-halo-width", 0.0),
                number("text-halo-blur", 0.0),
                number("text-opacity", 1.0),
            ],
            icon: [
                number("icon-size", 1.0),
                number("icon-halo-width", 0.0),
                number("icon-halo-blur", 0.0),
                number("icon-opacity", 1.0),
            ],
            text_layout: layout(paint, "text"),
            icon_layout: layout(paint, "icon"),
            atlas: [size[0] as f32, size[1] as f32, 0.0, 0.0],
            placement: [
                number("text-padding", 2.0),
                number("icon-padding", 2.0),
                keep_upright(paint, "text", true),
                keep_upright(paint, "icon", false),
            ],
            translate: {
                let [text_x, text_y] = crate::sdf::translation::viewport_shift(paint, "text", zoom);
                let [icon_x, icon_y] = crate::sdf::translation::viewport_shift(paint, "icon", zoom);
                [text_x, text_y, icon_x, icon_y]
            },
        }
    }
}

impl SymbolUniforms {
    /// The uniforms with `text-size` and `icon-size` evaluated for one feature's properties:
    /// a data-driven size has no value for the layer as a whole, and a label collides by the
    /// size it is drawn at.
    pub fn with_feature_sizes(
        mut self,
        paint: &SymbolPaint,
        properties: &FeatureProperties,
        zoom: f64,
    ) -> Self {
        self.text[0] = feature_style(paint, "text", properties, zoom)[2][0];
        self.icon[0] = feature_style(paint, "icon", properties, zoom)[2][0];
        self
    }
}

/// The fill colour, halo colour and size, halo width, halo blur and opacity of a feature's text
/// (`prefix` "text") or icon (`prefix` "icon"), each evaluated for its properties.
pub(crate) fn feature_style(
    paint: &SymbolPaint,
    prefix: &str,
    properties: &FeatureProperties,
    zoom: f64,
) -> [[f32; 4]; 3] {
    let color = |name: &str, fallback: [f32; 4]| {
        paint
            .properties
            .get(&format!("{prefix}-{name}"))
            .and_then(|value| {
                StyleProperty::<csscolorparser::Color>::parse(value).evaluate_for(properties, zoom)
            })
            .map_or(fallback, |color| {
                let [r, g, b, a] = color.to_array();
                [r as f32, g as f32, b as f32, a as f32]
            })
    };
    let number = |name: &str, fallback: f32| {
        paint.number(&format!("{prefix}-{name}"), properties, zoom, fallback)
    };
    let size = if prefix == "text" {
        paint
            .text_size
            .as_ref()
            .and_then(|value| value.evaluate_for(properties, zoom))
            .unwrap_or(16.0)
    } else {
        number("size", 1.0)
    };
    [
        color("color", [0.0, 0.0, 0.0, 1.0]),
        color("halo-color", [0.0; 4]),
        [
            size,
            number("halo-width", 0.0),
            number("halo-blur", 0.0),
            number("opacity", 1.0),
        ],
    ]
}

fn color(paint: &SymbolPaint, name: &str, zoom: f64, fallback: [f32; 4]) -> [f32; 4] {
    let Some(color) = paint.properties.get(name).and_then(|value| {
        StyleProperty::<csscolorparser::Color>::parse(value).evaluate_at_zoom(zoom)
    }) else {
        return fallback;
    };
    let [r, g, b, a] = color.to_array();
    [r as f32, g as f32, b as f32, a as f32]
}

fn layout(paint: &SymbolPaint, prefix: &str) -> [f32; 4] {
    let read = |suffix: &str| {
        paint
            .properties
            .get(&format!("{prefix}-{suffix}"))
            .and_then(|value| value.as_str())
    };
    let rotation = read("rotation-alignment");
    let map_rotation = rotation == Some("map")
        || (rotation.unwrap_or("auto") == "auto"
            && paint
                .properties
                .get("symbol-placement")
                .and_then(|value| value.as_str())
                .is_some_and(|value| matches!(value, "line" | "line-center")));
    let pitch = read("pitch-alignment");
    let map_pitch = pitch == Some("map") || (pitch.unwrap_or("auto") == "auto" && map_rotation);
    [
        u32::from(map_pitch) as f32,
        u32::from(map_rotation) as f32,
        // The tessellator bakes the rotation into each symbol's corners.
        0.0,
        u32::from(paint.uses_shared_height() || paint.height_follows_ground(prefix)) as f32,
    ]
}

fn keep_upright(paint: &SymbolPaint, prefix: &str, fallback: bool) -> f32 {
    let line = paint
        .properties
        .get("symbol-placement")
        .and_then(|v| v.as_str())
        .is_some_and(|value| matches!(value, "line" | "line-center"));
    f32::from(
        line && paint
            .properties
            .get(&format!("{prefix}-keep-upright"))
            .and_then(|v| v.as_bool())
            .unwrap_or(fallback),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sdf_icons_default_to_black_and_text_to_black_like_the_specification() {
        let uniforms = SymbolUniforms::new(&SymbolPaint::default(), 12.0, [1, 1]);
        assert_eq!(uniforms.icon_color, [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(uniforms.text_color, [0.0, 0.0, 0.0, 1.0]);
    }
}
