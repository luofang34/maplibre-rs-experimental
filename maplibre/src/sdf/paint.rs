//! Uniform values for readable text and sprite rendering.
use bytemuck_derive::{Pod, Zeroable};

use crate::style::{expression::FeatureProperties, layer::SymbolPaint};

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

/// Whether text along a line is placed glyph by glyph: it turns with the map, whether it lies
/// on the map plane or stands upright to the viewer, or each glyph stands upright on the screen
/// along the line (`viewport-glyph`). Text aligned with the viewport keeps the straight layout
/// at the anchor.
pub(crate) fn text_follows_line(paint: &SymbolPaint, zoom: f64) -> bool {
    SymbolUniforms::new(paint, zoom, [1, 1]).text_layout[1] > 0.5
        || glyphs_stay_upright(paint, zoom)
}

/// Whether the layer places its symbols along lines at `zoom`.
fn line_placed(paint: &SymbolPaint, zoom: f64) -> bool {
    paint
        .layout_enum("symbol-placement", zoom)
        .is_some_and(|value| matches!(value.as_str(), "line" | "line-center"))
}

/// Whether the glyphs of text along a line are placed along it each upright on the screen, as
/// GL JS's `viewport-glyph` rotation alignment places them: road shields in a row stay level
/// while the row follows its road.
pub(crate) fn glyphs_stay_upright(paint: &SymbolPaint, zoom: f64) -> bool {
    paint
        .layout_enum("text-rotation-alignment", zoom)
        .as_deref()
        == Some("viewport-glyph")
        && line_placed(paint, zoom)
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
            text_layout: layout(paint, "text", zoom),
            icon_layout: layout(paint, "icon", zoom),
            atlas: [size[0] as f32, size[1] as f32, 0.0, 0.0],
            placement: [
                number("text-padding", 2.0),
                number("icon-padding", 2.0),
                keep_upright(paint, "text", true, zoom),
                keep_upright(paint, "icon", false, zoom),
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
        let text_data_driven = paint
            .text_size
            .as_ref()
            .is_some_and(|s| !s.is_feature_constant());
        let icon_data_driven = paint
            .compiled_property::<f32>("icon-size")
            .is_some_and(|s| !s.is_feature_constant());
        if !text_data_driven && !icon_data_driven {
            return self;
        }
        if text_data_driven {
            self.text[0] = feature_style(paint, "text", properties, zoom)[2][0];
        }
        if icon_data_driven {
            self.icon[0] = feature_style(paint, "icon", properties, zoom)[2][0];
        }
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
    let is_text = prefix == "text";
    let (color_name, halo_color_name, halo_width_name, halo_blur_name, opacity_name) = if is_text {
        (
            "text-color",
            "text-halo-color",
            "text-halo-width",
            "text-halo-blur",
            "text-opacity",
        )
    } else {
        (
            "icon-color",
            "icon-halo-color",
            "icon-halo-width",
            "icon-halo-blur",
            "icon-opacity",
        )
    };
    let color = |full_name: &str, fallback: [f32; 4]| {
        if let Some(prop) = paint.compiled_property::<csscolorparser::Color>(full_name) {
            let res = if prop.is_feature_constant() {
                prop.evaluate_at_zoom(zoom)
            } else {
                prop.evaluate_for(properties, zoom)
            };
            return res.map_or(fallback, |color| color.to_array());
        }
        fallback
    };
    let number = |full_name: &str, fallback: f32| {
        if let Some(prop) = paint.compiled_property::<f32>(full_name) {
            let res = if prop.is_feature_constant() {
                prop.evaluate_at_zoom(zoom)
            } else {
                prop.evaluate_for(properties, zoom)
            };
            return res.filter(|value| value.is_finite()).unwrap_or(fallback);
        }
        fallback
    };
    let size = if is_text {
        if let Some(prop) = &paint.text_size {
            if prop.is_feature_constant() {
                prop.evaluate_at_zoom(zoom).unwrap_or(16.0)
            } else {
                prop.evaluate_for(properties, zoom).unwrap_or(16.0)
            }
        } else {
            16.0
        }
    } else {
        number("icon-size", 1.0)
    };
    [
        color(color_name, [0.0, 0.0, 0.0, 1.0]),
        color(halo_color_name, [0.0; 4]),
        [
            size,
            number(halo_width_name, 0.0),
            number(halo_blur_name, 0.0),
            number(opacity_name, 1.0),
        ],
    ]
}

fn color(paint: &SymbolPaint, name: &str, zoom: f64, fallback: [f32; 4]) -> [f32; 4] {
    let Some(color) = paint
        .compiled_property::<csscolorparser::Color>(name)
        .and_then(|prop| prop.evaluate_at_zoom(zoom))
    else {
        return fallback;
    };
    color.to_array()
}

fn layout(paint: &SymbolPaint, prefix: &str, zoom: f64) -> [f32; 4] {
    let read = |suffix: &str| paint.layout_enum(&format!("{prefix}-{suffix}"), zoom);
    let rotation = read("rotation-alignment");
    let map_rotation = rotation.as_deref() == Some("map")
        || (rotation.as_deref().unwrap_or("auto") == "auto" && line_placed(paint, zoom));
    let pitch = read("pitch-alignment");
    let map_pitch = pitch.as_deref() == Some("map")
        || (pitch.as_deref().unwrap_or("auto") == "auto" && map_rotation);
    [
        u32::from(map_pitch) as f32,
        u32::from(map_rotation) as f32,
        // The tessellator bakes the rotation into each symbol's corners.
        0.0,
        u32::from(paint.uses_shared_height() || paint.height_follows_ground(prefix)) as f32,
    ]
}

fn keep_upright(paint: &SymbolPaint, prefix: &str, fallback: bool, zoom: f64) -> f32 {
    let line = line_placed(paint, zoom);
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
