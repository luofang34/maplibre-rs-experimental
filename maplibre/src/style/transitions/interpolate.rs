//! Paint JSON interpolation and MapLibre Style Spec default fallback values for transitions.

use serde_json::Value;

use crate::style::{expression::Color, layer::StyleLayer};

/// Whether `property` supports smooth paint interpolation per the MapLibre Style Spec.
pub fn is_transitionable_property(property: &str) -> bool {
    !property.ends_with("-transition")
        && !property.ends_with("-fade-duration")
        && !property.ends_with("-pattern")
        && !property.ends_with("-anchor")
        && !property.ends_with("-alignment")
        && !property.ends_with("-resampling")
        && !property.ends_with("-datum")
        && !matches!(
            property,
            "fill-antialias"
                | "fill-extrusion-vertical-gradient"
                | "fill-extrusion-translucent"
                | "fill-extrusion-part"
                | "line-dasharray"
                | "line-gradient"
                | "color-relief-color"
                | "circle-pitch-scale"
        )
}

fn parse_constant_color(value: &Value) -> Option<Color> {
    if let Some(text) = value.as_str() {
        return Color::parse(text);
    }
    let items = value.as_array()?;
    let op = items.first()?.as_str()?;
    match (op, items.len()) {
        ("rgb", 4) => {
            let r = items.get(1)?.as_f64()?;
            let g = items.get(2)?.as_f64()?;
            let b = items.get(3)?.as_f64()?;
            Some(Color::new(
                (r / 255.0).clamp(0.0, 1.0),
                (g / 255.0).clamp(0.0, 1.0),
                (b / 255.0).clamp(0.0, 1.0),
                1.0,
            ))
        }
        ("rgba", 5) => {
            let r = items.get(1)?.as_f64()?;
            let g = items.get(2)?.as_f64()?;
            let b = items.get(3)?.as_f64()?;
            let a = items.get(4)?.as_f64()?;
            Some(Color::new(
                (r / 255.0).clamp(0.0, 1.0),
                (g / 255.0).clamp(0.0, 1.0),
                (b / 255.0).clamp(0.0, 1.0),
                a.clamp(0.0, 1.0),
            ))
        }
        _ => None,
    }
}

fn interpolate_color(from: Color, to: Color, progress: f64) -> String {
    let (from_pm, to_pm) = (from.premultiplied(), to.premultiplied());
    let [red, green, blue, alpha] =
        std::array::from_fn(|i| from_pm[i] + (to_pm[i] - from_pm[i]) * progress);
    let alpha = alpha.clamp(0.0, 1.0);
    let straight = |channel: f64| {
        if alpha > 0.0 {
            (channel / alpha).clamp(0.0, 1.0)
        } else {
            0.0
        }
    };
    Color::new(straight(red), straight(green), straight(blue), alpha).css()
}

/// Interpolates two constant paint JSON values (`f64`/`f32` numbers, premultiplied-alpha colors,
/// `[f32; 2]` translations/offsets, and fixed-length numeric or color arrays) at `progress` in
/// `[0.0, 1.0]`. Returns `None` for expressions, nulls, booleans, and mismatched types.
pub fn interpolate_paint_value(from: &Value, to: &Value, progress: f64) -> Option<Value> {
    let t = progress.clamp(0.0, 1.0);
    if let (Some(a), Some(b)) = (from.as_f64(), to.as_f64()) {
        return Some(Value::from(a + (b - a) * t));
    }
    if let (Some(a), Some(b)) = (parse_constant_color(from), parse_constant_color(to)) {
        return Some(Value::from(interpolate_color(a, b, t)));
    }
    let (Some(a_arr), Some(b_arr)) = (from.as_array(), to.as_array()) else {
        return None;
    };
    if a_arr.is_empty() || a_arr.len() != b_arr.len() {
        return None;
    }
    let nums: Option<Vec<Value>> = a_arr
        .iter()
        .zip(b_arr.iter())
        .map(|(x, y)| Some(Value::from(x.as_f64()? + (y.as_f64()? - x.as_f64()?) * t)))
        .collect();
    if let Some(values) = nums {
        return Some(Value::Array(values));
    }
    let colors: Option<Vec<Value>> = a_arr
        .iter()
        .zip(b_arr.iter())
        .map(|(x, y)| {
            Some(Value::from(interpolate_color(
                parse_constant_color(x)?,
                parse_constant_color(y)?,
                t,
            )))
        })
        .collect();
    colors.map(Value::Array)
}

/// Default MapLibre Style Spec JSON value for an unset paint property when starting a transition.
pub(crate) fn default_paint_json(layer: &StyleLayer, property: &str) -> Option<Value> {
    match property {
        "background-color"
        | "fill-color"
        | "line-color"
        | "circle-color"
        | "circle-stroke-color"
        | "fill-extrusion-color"
        | "icon-color"
        | "text-color"
        | "icon-halo-color"
        | "text-halo-color"
        | "hillshade-shadow-color"
        | "hillshade-accent-color" => Some(Value::from("#000000")),
        "hillshade-highlight-color" => Some(Value::from("#ffffff")),
        "fill-outline-color" => layer
            .property_maps()
            .ok()
            .and_then(|maps| maps.paint.get("fill-color").cloned())
            .or_else(|| Some(Value::from("#000000"))),
        "background-opacity"
        | "fill-opacity"
        | "line-opacity"
        | "circle-opacity"
        | "circle-stroke-opacity"
        | "fill-extrusion-opacity"
        | "raster-opacity"
        | "color-relief-opacity"
        | "icon-opacity"
        | "text-opacity"
        | "raster-brightness-max"
        | "heatmap-intensity"
        | "heatmap-opacity" => Some(Value::from(1.0)),
        "line-width" => Some(Value::from(1.0)),
        "circle-radius" => Some(Value::from(5.0)),
        "heatmap-radius" => Some(Value::from(30.0)),
        "hillshade-exaggeration" => Some(Value::from(0.5)),
        "fill-translate"
        | "line-translate"
        | "circle-translate"
        | "fill-extrusion-translate"
        | "icon-translate"
        | "text-translate" => Some(serde_json::json!([0.0, 0.0])),
        "circle-blur"
        | "circle-stroke-width"
        | "line-blur"
        | "line-offset"
        | "line-gap-width"
        | "fill-extrusion-height"
        | "fill-extrusion-base"
        | "fill-extrusion-rounded-corner-distance"
        | "icon-halo-width"
        | "icon-halo-blur"
        | "text-halo-width"
        | "text-halo-blur"
        | "raster-brightness-min"
        | "raster-contrast"
        | "raster-saturation"
        | "raster-hue-rotate" => Some(Value::from(0.0)),
        _ => None,
    }
}
