//! Uniform values for readable text and sprite rendering.
use crate::style::{
    expression::FeatureProperties,
    layer::{StyleProperty, SymbolPaint},
};
use bytemuck_derive::{Pod, Zeroable};

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
}

impl SymbolUniforms {
    pub fn new(paint: &SymbolPaint, zoom: f64, size: [u32; 2]) -> Self {
        let properties = FeatureProperties::new();
        let number = |name, fallback| paint.number(name, &properties, zoom, fallback);
        Self {
            text_color: color(paint, "text-color", zoom, [0.0, 0.0, 0.0, 1.0]),
            halo_color: color(paint, "text-halo-color", zoom, [0.0; 4]),
            icon_color: color(paint, "icon-color", zoom, [1.0; 4]),
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
                keep_upright(paint, "text", true),
                keep_upright(paint, "icon", false),
            ],
        }
    }
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

fn layout(paint: &SymbolPaint, prefix: &str, zoom: f64) -> [f32; 4] {
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
        paint
            .number(
                &format!("{prefix}-rotate"),
                &FeatureProperties::new(),
                zoom,
                0.0,
            )
            .to_radians(),
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
