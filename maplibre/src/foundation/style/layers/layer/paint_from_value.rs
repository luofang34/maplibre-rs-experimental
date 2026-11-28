//! Direct `&serde_json::Map` deserializers for [`LayerPaint`] avoiding `#[serde(derive)]` bloat.

use csscolorparser::Color;
use serde::de;
use serde_json::{Map, Value};

#[cfg(any(not(target_arch = "wasm32"), feature = "webgpu", feature = "web-webgl"))]
use super::paint::{BackgroundPaint, RasterPaint, RasterResampling};
use super::{
    paint::{FillExtrusionPaint, FillPaint, LinePaint, SymbolPaint, TranslateAnchor},
    LayerPaint, LayerVisibility, PropertyValue, StyleProperty,
};
#[cfg(any(not(target_arch = "wasm32"), feature = "webgpu", feature = "web-webgl"))]
use crate::style::hillshade::{
    ColorReliefPaint, HillshadeMethod, HillshadePaint, IlluminationAnchor,
};
use crate::{
    style::{
        circle::{CirclePaint, CirclePitchAlignment, CirclePitchScale},
        heatmap::HeatmapPaint,
        translation::Translation,
    },
    util::json::extract::field_err,
};

pub(super) fn parse_opt_prop<T: PropertyValue>(
    map: &Map<String, Value>,
    key: &str,
) -> Option<StyleProperty<T>> {
    map.get(key)
        .filter(|v| !v.is_null())
        .map(StyleProperty::<T>::parse)
}

#[inline(never)]
pub(super) fn parse_opt_f32(map: &Map<String, Value>, key: &str) -> Option<StyleProperty<f32>> {
    parse_opt_prop::<f32>(map, key)
}

#[inline(never)]
pub(super) fn parse_opt_color(map: &Map<String, Value>, key: &str) -> Option<StyleProperty<Color>> {
    parse_opt_prop::<Color>(map, key)
}

#[inline(never)]
pub(super) fn parse_opt_string_prop(
    map: &Map<String, Value>,
    key: &str,
) -> Option<StyleProperty<String>> {
    parse_opt_prop::<String>(map, key)
}

#[inline(never)]
pub(super) fn parse_opt_translation(map: &Map<String, Value>, key: &str) -> Option<Translation> {
    map.get(key)
        .filter(|v| !v.is_null())
        .map(Translation::from_json_value)
}

fn parse_translate_anchor(
    map: &Map<String, Value>,
    key: &str,
) -> Result<TranslateAnchor, serde_json::Error> {
    match crate::util::json::extract::json_opt_str(map, key)? {
        None | Some("map") => Ok(TranslateAnchor::Map),
        Some("viewport") => Ok(TranslateAnchor::Viewport),
        Some(_) => Err(field_err(key, "expected \"map\" or \"viewport\"")),
    }
}

pub(super) fn parse_visibility(value: &Value) -> Result<LayerVisibility, serde_json::Error> {
    match value.as_str() {
        Some("visible") => Ok(LayerVisibility::Visible),
        Some("none") => Ok(LayerVisibility::None),
        _ => Err(de::Error::custom(
            "visibility must be \"visible\" or \"none\"",
        )),
    }
}

#[cfg(any(not(target_arch = "wasm32"), feature = "webgpu", feature = "web-webgl"))]
#[inline(never)]
fn push_extras(map: &Map<String, Value>, known: &[&str], extra: &mut Map<String, Value>) {
    for (k, v) in map {
        if !k.ends_with("-transition") && !known.contains(&k.as_str()) {
            extra.insert(k.clone(), v.clone());
        }
    }
}

#[cfg(not(any(not(target_arch = "wasm32"), feature = "webgpu", feature = "web-webgl")))]
#[inline]
fn push_extras(_map: &Map<String, Value>, _known: &[&str], _extra: &mut Map<String, Value>) {}

#[cfg(any(not(target_arch = "wasm32"), feature = "webgpu", feature = "web-webgl"))]
#[cfg(any(not(target_arch = "wasm32"), feature = "webgpu", feature = "web-webgl"))]
fn parse_background(map: &Map<String, Value>, extra: &mut Map<String, Value>) -> LayerPaint {
    use super::symbol_serde::KnownPaintKeys;
    push_extras(map, BackgroundPaint::KNOWN_KEYS, extra);
    LayerPaint::Background(BackgroundPaint {
        background_color: parse_opt_color(map, "background-color"),
        background_pattern: map
            .get("background-pattern")
            .filter(|v| !v.is_null())
            .cloned(),
        background_opacity: parse_opt_f32(map, "background-opacity"),
    })
}

fn parse_fill(
    map: &Map<String, Value>,
    extra: &mut Map<String, Value>,
) -> Result<LayerPaint, serde_json::Error> {
    use super::symbol_serde::KnownPaintKeys;
    push_extras(map, FillPaint::KNOWN_KEYS, extra);
    Ok(LayerPaint::Fill(FillPaint {
        fill_color: parse_opt_color(map, "fill-color"),
        fill_outline_color: parse_opt_color(map, "fill-outline-color"),
        fill_opacity: parse_opt_f32(map, "fill-opacity"),
        fill_pattern: map.get("fill-pattern").filter(|v| !v.is_null()).cloned(),
        fill_antialias: map.get("fill-antialias").filter(|v| !v.is_null()).cloned(),
        fill_translate: parse_opt_translation(map, "fill-translate"),
        fill_translate_anchor: parse_translate_anchor(map, "fill-translate-anchor")?,
    }))
}

fn parse_line(
    map: &Map<String, Value>,
    extra: &mut Map<String, Value>,
) -> Result<LayerPaint, serde_json::Error> {
    use super::symbol_serde::KnownPaintKeys;
    push_extras(map, LinePaint::KNOWN_KEYS, extra);
    Ok(LayerPaint::Line(LinePaint {
        line_color: parse_opt_color(map, "line-color"),
        line_width: parse_opt_f32(map, "line-width"),
        line_opacity: parse_opt_f32(map, "line-opacity"),
        line_translate: parse_opt_translation(map, "line-translate"),
        line_translate_anchor: parse_translate_anchor(map, "line-translate-anchor")?,
        line_gradient: parse_opt_color(map, "line-gradient"),
        line_offset: parse_opt_f32(map, "line-offset"),
        line_gap_width: parse_opt_f32(map, "line-gap-width"),
        line_blur: parse_opt_f32(map, "line-blur"),
        line_pattern: map.get("line-pattern").filter(|v| !v.is_null()).cloned(),
        line_dasharray: map.get("line-dasharray").filter(|v| !v.is_null()).cloned(),
        line_elevation: parse_opt_f32(map, "line-elevation"),
        line_elevation_datum: parse_opt_string_prop(map, "line-elevation-datum"),
    }))
}

fn parse_fill_extrusion(
    map: &Map<String, Value>,
    rounded_from_layout: Option<&Value>,
    extra: &mut Map<String, Value>,
) -> Result<LayerPaint, serde_json::Error> {
    use super::symbol_serde::KnownPaintKeys;
    push_extras(map, FillExtrusionPaint::KNOWN_KEYS, extra);
    let rounded = parse_opt_f32(map, "fill-extrusion-rounded-corner-distance").or_else(|| {
        rounded_from_layout
            .filter(|v| !v.is_null())
            .map(StyleProperty::<f32>::parse)
    });
    Ok(LayerPaint::FillExtrusion(FillExtrusionPaint {
        fill_extrusion_color: parse_opt_color(map, "fill-extrusion-color"),
        fill_extrusion_pattern: map
            .get("fill-extrusion-pattern")
            .filter(|v| !v.is_null())
            .cloned(),
        fill_extrusion_opacity: parse_opt_f32(map, "fill-extrusion-opacity"),
        fill_extrusion_height: parse_opt_f32(map, "fill-extrusion-height"),
        fill_extrusion_base: parse_opt_f32(map, "fill-extrusion-base"),
        fill_extrusion_vertical_gradient: crate::util::json::extract::json_opt_bool(
            map,
            "fill-extrusion-vertical-gradient",
        )?,
        fill_extrusion_translucent: crate::util::json::extract::json_opt_bool(
            map,
            "fill-extrusion-translucent",
        )?,
        fill_extrusion_base_datum: parse_opt_string_prop(map, "fill-extrusion-base-datum"),
        fill_extrusion_height_datum: parse_opt_string_prop(map, "fill-extrusion-height-datum"),
        fill_extrusion_part: parse_opt_string_prop(map, "fill-extrusion-part"),
        fill_extrusion_rounded_corner_distance: rounded,
        fill_extrusion_translate: parse_opt_translation(map, "fill-extrusion-translate"),
        fill_extrusion_translate_anchor: parse_translate_anchor(
            map,
            "fill-extrusion-translate-anchor",
        )?,
    }))
}

fn parse_circle(
    map: &Map<String, Value>,
    extra: &mut Map<String, Value>,
) -> Result<LayerPaint, serde_json::Error> {
    use super::symbol_serde::KnownPaintKeys;
    push_extras(map, CirclePaint::KNOWN_KEYS, extra);
    let pitch_scale = match crate::util::json::extract::json_opt_str(map, "circle-pitch-scale")? {
        None | Some("map") => CirclePitchScale::Map,
        Some("viewport") => CirclePitchScale::Viewport,
        Some(_) => return Err(field_err("circle-pitch-scale", "expected map or viewport")),
    };
    let pitch_alignment =
        match crate::util::json::extract::json_opt_str(map, "circle-pitch-alignment")? {
            None | Some("viewport") => CirclePitchAlignment::Viewport,
            Some("map") => CirclePitchAlignment::Map,
            Some(_) => {
                return Err(field_err(
                    "circle-pitch-alignment",
                    "expected map or viewport",
                ))
            }
        };
    Ok(LayerPaint::Circle(CirclePaint {
        circle_radius: parse_opt_f32(map, "circle-radius"),
        circle_color: parse_opt_color(map, "circle-color"),
        circle_blur: parse_opt_f32(map, "circle-blur"),
        circle_opacity: parse_opt_f32(map, "circle-opacity"),
        circle_stroke_width: parse_opt_f32(map, "circle-stroke-width"),
        circle_stroke_color: parse_opt_color(map, "circle-stroke-color"),
        circle_stroke_opacity: parse_opt_f32(map, "circle-stroke-opacity"),
        circle_translate: parse_opt_translation(map, "circle-translate"),
        circle_translate_anchor: parse_translate_anchor(map, "circle-translate-anchor")?,
        circle_pitch_scale: pitch_scale,
        circle_pitch_alignment: pitch_alignment,
    }))
}

fn parse_heatmap(map: &Map<String, Value>, extra: &mut Map<String, Value>) -> LayerPaint {
    use super::symbol_serde::KnownPaintKeys;
    push_extras(map, HeatmapPaint::KNOWN_KEYS, extra);
    LayerPaint::Heatmap(HeatmapPaint {
        heatmap_radius: parse_opt_f32(map, "heatmap-radius"),
        heatmap_weight: parse_opt_f32(map, "heatmap-weight"),
        heatmap_intensity: parse_opt_f32(map, "heatmap-intensity"),
        heatmap_opacity: parse_opt_f32(map, "heatmap-opacity"),
        heatmap_color: parse_opt_color(map, "heatmap-color"),
    })
}

fn parse_symbol(map: &Map<String, Value>) -> LayerPaint {
    let text_field = parse_opt_prop(map, "text-field");
    let text_size = parse_opt_f32(map, "text-size");
    let mut properties = Map::new();
    for (key, value) in map {
        if key != "text-field" && key != "text-size" && !key.ends_with("-transition") {
            properties.insert(key.clone(), value.clone());
        }
    }
    LayerPaint::Symbol(SymbolPaint {
        text_field,
        text_size,
        properties,
        compiled: Default::default(),
    })
}

#[cfg(any(not(target_arch = "wasm32"), feature = "webgpu", feature = "web-webgl"))]
fn parse_raster(
    map: &Map<String, Value>,
    extra: &mut Map<String, Value>,
) -> Result<LayerPaint, serde_json::Error> {
    use super::symbol_serde::KnownPaintKeys;
    push_extras(map, RasterPaint::KNOWN_KEYS, extra);
    let raster_fade_duration =
        crate::util::json::extract::json_opt_u32(map, "raster-fade-duration")?;
    let raster_resampling = map
        .get("raster-resampling")
        .filter(|v| !v.is_null())
        .map(RasterResampling::from_json_value)
        .transpose()?;
    Ok(LayerPaint::Raster(RasterPaint {
        raster_brightness_max: parse_opt_f32(map, "raster-brightness-max"),
        raster_brightness_min: parse_opt_f32(map, "raster-brightness-min"),
        raster_contrast: parse_opt_f32(map, "raster-contrast"),
        raster_fade_duration,
        raster_hue_rotate: parse_opt_f32(map, "raster-hue-rotate"),
        raster_opacity: parse_opt_f32(map, "raster-opacity"),
        raster_resampling,
        raster_saturation: parse_opt_f32(map, "raster-saturation"),
    }))
}

#[cfg(any(not(target_arch = "wasm32"), feature = "webgpu", feature = "web-webgl"))]
fn parse_hillshade(
    map: &Map<String, Value>,
    extra: &mut Map<String, Value>,
) -> Result<LayerPaint, serde_json::Error> {
    use super::symbol_serde::KnownPaintKeys;
    push_extras(map, HillshadePaint::KNOWN_KEYS, extra);
    let anchor =
        match crate::util::json::extract::json_opt_str(map, "hillshade-illumination-anchor")? {
            None | Some("viewport") => IlluminationAnchor::Viewport,
            Some("map") => IlluminationAnchor::Map,
            Some(_) => {
                return Err(field_err(
                    "hillshade-illumination-anchor",
                    "expected map or viewport",
                ))
            }
        };
    let method = match crate::util::json::extract::json_opt_str(map, "hillshade-method")? {
        None | Some("standard") => HillshadeMethod::Standard,
        Some("basic") => HillshadeMethod::Basic,
        Some("combined") => HillshadeMethod::Combined,
        Some("igor") => HillshadeMethod::Igor,
        Some("multidirectional") => HillshadeMethod::Multidirectional,
        Some(_) => return Err(field_err("hillshade-method", "unknown hillshade-method")),
    };
    let resampling = map
        .get("hillshade-resampling")
        .filter(|v| !v.is_null())
        .map(RasterResampling::from_json_value)
        .transpose()?;
    Ok(LayerPaint::Hillshade(HillshadePaint {
        hillshade_illumination_direction: parse_opt_prop(map, "hillshade-illumination-direction"),
        hillshade_illumination_altitude: parse_opt_prop(map, "hillshade-illumination-altitude"),
        hillshade_illumination_anchor: anchor,
        hillshade_exaggeration: parse_opt_f32(map, "hillshade-exaggeration"),
        hillshade_shadow_color: parse_opt_prop(map, "hillshade-shadow-color"),
        hillshade_highlight_color: parse_opt_prop(map, "hillshade-highlight-color"),
        hillshade_accent_color: parse_opt_color(map, "hillshade-accent-color"),
        hillshade_method: method,
        hillshade_resampling: resampling,
    }))
}

#[cfg(any(not(target_arch = "wasm32"), feature = "webgpu", feature = "web-webgl"))]
fn parse_color_relief(
    map: &Map<String, Value>,
    extra: &mut Map<String, Value>,
) -> Result<LayerPaint, serde_json::Error> {
    use super::symbol_serde::KnownPaintKeys;
    push_extras(map, ColorReliefPaint::KNOWN_KEYS, extra);
    let resampling = map
        .get("color-relief-resampling")
        .filter(|v| !v.is_null())
        .map(RasterResampling::from_json_value)
        .transpose()?;
    Ok(LayerPaint::ColorRelief(ColorReliefPaint {
        color_relief_opacity: parse_opt_f32(map, "color-relief-opacity"),
        color_relief_color: parse_opt_color(map, "color-relief-color"),
        color_relief_resampling: resampling,
    }))
}

/// Parses optional layer `paint` directly from JSON without going through `serde` derives.
pub(super) fn parse_layer_paint_from_value(
    layer_type: &str,
    paint: Option<&Value>,
    rounded_from_layout: Option<&Value>,
    extra: &mut Map<String, Value>,
) -> Result<Option<LayerPaint>, serde_json::Error> {
    let has_paint =
        paint.is_some() || (layer_type == "fill-extrusion" && rounded_from_layout.is_some());
    if !has_paint {
        return Ok(match layer_type {
            "circle" => Some(LayerPaint::Circle(Default::default())),
            "fill" => Some(LayerPaint::Fill(Default::default())),
            "line" => Some(LayerPaint::Line(Default::default())),
            #[cfg(any(not(target_arch = "wasm32"), feature = "webgpu", feature = "web-webgl"))]
            "background" => Some(LayerPaint::Background(Default::default())),
            "fill-extrusion" => Some(LayerPaint::FillExtrusion(Default::default())),
            "heatmap" => Some(LayerPaint::Heatmap(Default::default())),
            #[cfg(any(not(target_arch = "wasm32"), feature = "webgpu", feature = "web-webgl"))]
            "hillshade" => Some(LayerPaint::Hillshade(Default::default())),
            #[cfg(any(not(target_arch = "wasm32"), feature = "webgpu", feature = "web-webgl"))]
            "color-relief" => Some(LayerPaint::ColorRelief(Default::default())),
            "symbol" => Some(LayerPaint::Symbol(Default::default())),
            "custom" => Some(LayerPaint::Custom(Default::default())),
            _ => None,
        });
    }
    let map = match paint {
        Some(Value::Object(map)) => map,
        Some(_) => return Err(de::Error::custom("layer paint must be a JSON object")),
        None => {
            if layer_type == "fill-extrusion" {
                static EMPTY: std::sync::OnceLock<Map<String, Value>> = std::sync::OnceLock::new();
                EMPTY.get_or_init(Map::new)
            } else {
                return Ok(None);
            }
        }
    };
    let parsed = match layer_type {
        #[cfg(any(not(target_arch = "wasm32"), feature = "webgpu", feature = "web-webgl"))]
        "background" => parse_background(map, extra),
        #[cfg(not(any(not(target_arch = "wasm32"), feature = "webgpu", feature = "web-webgl")))]
        "background" => return Ok(None),
        "line" => parse_line(map, extra)?,
        "fill" => parse_fill(map, extra)?,
        "fill-extrusion" => parse_fill_extrusion(map, rounded_from_layout, extra)?,
        #[cfg(any(not(target_arch = "wasm32"), feature = "webgpu", feature = "web-webgl"))]
        "raster" => parse_raster(map, extra)?,
        #[cfg(not(any(not(target_arch = "wasm32"), feature = "webgpu", feature = "web-webgl")))]
        "raster" | "hillshade" | "color-relief" => return Ok(None),
        #[cfg(any(not(target_arch = "wasm32"), feature = "webgpu", feature = "web-webgl"))]
        "hillshade" => parse_hillshade(map, extra)?,
        #[cfg(any(not(target_arch = "wasm32"), feature = "webgpu", feature = "web-webgl"))]
        "color-relief" => parse_color_relief(map, extra)?,
        "circle" => parse_circle(map, extra)?,
        "heatmap" => parse_heatmap(map, extra),
        "symbol" => parse_symbol(map),
        "custom" => {
            for (k, v) in map {
                if !k.ends_with("-transition") {
                    extra.insert(k.clone(), v.clone());
                }
            }
            LayerPaint::Custom(Default::default())
        }
        _ => {
            for (k, v) in map {
                if !k.ends_with("-transition") {
                    extra.insert(k.clone(), v.clone());
                }
            }
            return Ok(None);
        }
    };
    Ok(Some(parsed))
}
