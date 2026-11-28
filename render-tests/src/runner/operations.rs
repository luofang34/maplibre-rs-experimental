//! Applies the runtime operations a fixture lists before its comparison.
//!
//! GL JS fixtures change a map after it loads (`setPaintProperty`, `addLayer`, `setStyle`, ...)
//! and compare the result. The harness applies them to the style in order, through the same
//! mutation API a host uses, and renders the final state.

use maplibre::style::Style;
use serde_json::Value;

/// The operations under `metadata.test.operations`, in order.
pub(crate) fn operations_of(style: &Value) -> Vec<Value> {
    style
        .pointer("/metadata/test/operations")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn number(items: &[Value], index: usize) -> Result<f64, String> {
    items
        .get(index)
        .and_then(Value::as_f64)
        .ok_or_else(|| format!("{} needs a number", items.first().unwrap_or(&Value::Null)))
}

/// Reads the image at `path`, relative to the assets, and adds it to the style by name.
fn add_image(style: &mut Style, name: &str, path: &Value, options: &Value) -> Result<(), String> {
    let path = path.as_str().ok_or("addImage needs an image path")?;
    let relative = path.strip_prefix("./").unwrap_or(path);
    let file = std::path::Path::new("render-tests/src/assets").join(relative);
    let image = image::open(&file)
        .map_err(|error| format!("Cannot read image {}: {error}", file.display()))?
        .to_rgba8();
    style.add_image(
        name,
        maplibre::style::StyleImage {
            width: image.width(),
            height: image.height(),
            data: image.into_raw(),
            pixel_ratio: options
                .get("pixelRatio")
                .and_then(Value::as_f64)
                .map_or(1.0, |ratio| ratio as f32),
            sdf: options.get("sdf").and_then(Value::as_bool).unwrap_or(false),
        },
    );
    Ok(())
}

/// Applies `setFeatureState` or `removeFeatureState` through [`Style`]'s native `feature-state`
/// store.
fn feature_state(
    style: &mut Style,
    target: &Value,
    states: &Value,
    removing: bool,
) -> Result<(), String> {
    use maplibre::style::{
        feature_state::{FeatureId, FeatureStateTarget},
        source::Source,
    };

    let source = target
        .get("source")
        .and_then(Value::as_str)
        .ok_or("feature state needs a source")?;
    if !matches!(
        style.sources.get(source),
        Some(Source::GeoJson(_) | Source::Vector(_))
    ) {
        return Err(format!(
            "feature state on `{source}`: only GeoJSON and vector sources are supported"
        ));
    }
    let source_layer = target
        .get("sourceLayer")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let target_ref = FeatureStateTarget {
        source: source.to_owned(),
        source_layer,
    };
    let id = match target.get("id") {
        Some(value) if !value.is_null() => Some(
            FeatureId::from_json(value)
                .ok_or_else(|| "feature state needs a string or number id".to_owned())?,
        ),
        _ if removing => None,
        _ => return Err("feature state needs an id".to_owned()),
    };
    if removing {
        style.remove_feature_state(&target_ref, id, states.as_str());
        return Ok(());
    }
    let Value::Object(values) = states else {
        return Err("setFeatureState needs an object of states".to_owned());
    };
    let id = id.ok_or_else(|| "feature state needs an id".to_owned())?;
    style.set_feature_state(target_ref, id, values.clone());
    Ok(())
}

/// A `setZoom` the fixture made: the zoom it left and the clock when it did.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ZoomChange {
    /// The zoom before the change.
    pub previous_zoom: f64,
    /// Milliseconds the fixture had waited when it changed the zoom.
    pub at: f64,
}

struct ApplyContext<'a> {
    transitions: &'a mut super::transitions::Transitions,
    paused_tiles: &'a mut std::collections::HashMap<String, f64>,
    zoom_change: &'a mut Option<ZoomChange>,
}

fn arg_text(items: &[Value], index: usize) -> Option<&str> {
    items.get(index).and_then(Value::as_str)
}

fn arg_value(items: &[Value], index: usize) -> Value {
    items.get(index).cloned().unwrap_or(Value::Null)
}

fn apply_set_center(style: &mut Style, arg: Value) -> Result<(), String> {
    match arg.as_array().map(Vec::as_slice) {
        Some([longitude, latitude]) => match (longitude.as_f64(), latitude.as_f64()) {
            (Some(longitude), Some(latitude)) => {
                style.center = Some([longitude, latitude]);
                Ok(())
            }
            _ => Err("setCenter needs numbers".to_owned()),
        },
        _ => Err("setCenter needs [longitude, latitude]".to_owned()),
    }
}

fn apply_camera_op(
    style: &mut Style,
    name: &str,
    items: &[Value],
    ctx: &mut ApplyContext<'_>,
) -> Option<Result<(), String>> {
    match (name, arg_text(items, 1)) {
        ("wait" | "idle" | "sleep", _) => {
            ctx.transitions
                .wait(items.get(1).and_then(Value::as_f64).unwrap_or(0.0));
            Some(Ok(()))
        }
        ("pauseTiles", Some(source)) => {
            ctx.paused_tiles
                .insert(source.to_owned(), style.zoom.unwrap_or(0.0));
            Some(Ok(()))
        }
        ("setZoom", _) => Some(number(items, 1).map(|zoom| {
            *ctx.zoom_change = Some(ZoomChange {
                previous_zoom: style.zoom.unwrap_or(0.0),
                at: ctx.transitions.now(),
            });
            style.zoom = Some(zoom);
        })),
        ("setBearing", _) => Some(number(items, 1).map(|bearing| style.bearing = Some(bearing))),
        ("setPitch", _) => Some(number(items, 1).map(|pitch| style.pitch = Some(pitch))),
        ("setRoll", _) => Some(number(items, 1).map(|roll| style.roll = Some(roll))),
        (
            "setPadding"
            | "setCenterClampedToGround"
            | "setCenterElevation"
            | "setVerticalFieldOfView",
            _,
        ) => Some(Ok(())),
        ("easeTo", _)
            if crate::comparison::camera_options::eases_only_padding(&arg_value(items, 1)) =>
        {
            Some(Ok(()))
        }
        ("setCenter", _) => Some(apply_set_center(style, arg_value(items, 1))),
        _ => None,
    }
}

fn apply_style_op(
    style: &mut Style,
    name: &str,
    items: &[Value],
    transitions: &mut super::transitions::Transitions,
) -> Option<Result<(), String>> {
    match (name, arg_text(items, 1)) {
        ("addImage", Some(image_name)) => Some(add_image(
            style,
            image_name,
            &arg_value(items, 2),
            &arg_value(items, 3),
        )),
        ("removeImage", Some(image_name)) => {
            style.remove_image(image_name);
            Some(Ok(()))
        }
        ("setLight", _) => Some(
            serde_json::from_value(arg_value(items, 1))
                .map_err(|error| format!("setLight: invalid light: {error}"))
                .map(|light| style.light = Some(light)),
        ),
        ("setTerrain", _) => Some(match arg_value(items, 1) {
            Value::Null => {
                style.terrain = None;
                Ok(())
            }
            terrain => serde_json::from_value(terrain)
                .map_err(|error| format!("setTerrain: invalid terrain: {error}"))
                .map(|parsed| style.terrain = Some(parsed)),
        }),
        ("setGlobalStateProperty", Some(key)) => {
            style.set_global_state(key, arg_value(items, 2));
            Some(Ok(()))
        }
        ("setLayerZoomRange", Some(layer)) => Some(apply_layer_zoom_range(style, layer, items)),
        ("setPaintProperty", Some(layer)) => {
            Some(apply_paint_property(style, layer, items, transitions))
        }
        ("setLayoutProperty", Some(layer)) => Some(
            arg_text(items, 2)
                .ok_or_else(|| "setLayoutProperty needs a property".to_owned())
                .and_then(|property| {
                    style
                        .set_layout_property(layer, property, arg_value(items, 3))
                        .map(drop)
                        .map_err(|error| error.to_string())
                }),
        ),
        ("setFilter", Some(layer)) => {
            let filter = arg_value(items, 2);
            Some(
                style
                    .set_filter(layer, (!filter.is_null()).then_some(filter))
                    .map(drop)
                    .map_err(|error| error.to_string()),
            )
        }
        _ => None,
    }
}

fn apply_layer_zoom_range(style: &mut Style, layer: &str, items: &[Value]) -> Result<(), String> {
    let (minzoom, maxzoom) = (number(items, 2)?, number(items, 3)?);
    let target = style
        .layers
        .iter_mut()
        .find(|candidate| candidate.id == layer)
        .ok_or_else(|| format!("setLayerZoomRange: no layer `{layer}`"))?;
    target.minzoom = Some(minzoom);
    target.maxzoom = Some(maxzoom);
    Ok(())
}

fn apply_paint_property(
    style: &mut Style,
    layer: &str,
    items: &[Value],
    transitions: &mut super::transitions::Transitions,
) -> Result<(), String> {
    let property =
        arg_text(items, 2).ok_or_else(|| "setPaintProperty needs a property".to_owned())?;
    let val = arg_value(items, 3);
    if property.ends_with("-transition") {
        transitions.declare(layer, property, &val);
        return Ok(());
    }
    transitions.before_change(style, layer, property, &val);
    style
        .set_paint_property(layer, property, val)
        .map(drop)
        .map_err(|error| error.to_string())
}

fn apply_set_style(style: &mut Style, arg: Value) -> Result<(), String> {
    let mut replacement = match arg {
        Value::String(url) => {
            let path = crate::paths::local_style_path(&url)?;
            let text = std::fs::read_to_string(&path)
                .map_err(|error| format!("Cannot read {}: {error}", path.display()))?;
            serde_json::from_str(&text)
                .map_err(|error| format!("Cannot parse {}: {error}", path.display()))?
        }
        other => other,
    };
    crate::sources::tilesets::resolve_sources(&mut replacement)?;
    let mut replaced: Style = serde_json::from_value(replacement)
        .map_err(|error| format!("setStyle: invalid style: {error}"))?;
    // A style that does not place the camera leaves it where it is.
    replaced.center = replaced.center.or(style.center);
    replaced.zoom = replaced.zoom.or(style.zoom);
    replaced.bearing = replaced.bearing.or(style.bearing);
    replaced.pitch = replaced.pitch.or(style.pitch);
    *style = replaced;
    Ok(())
}

fn apply_query_source_features(style: &Style, items: &[Value]) -> Result<(), String> {
    let source = arg_text(items, 1).or_else(|| {
        items
            .get(1)
            .and_then(Value::as_array)
            .and_then(|arr| arr.first())
            .and_then(Value::as_str)
    });
    match source {
        Some(source) if style.sources.contains_key(source) => Ok(()),
        Some(source) => Err(format!("unknown source `{source}`")),
        None => Err("needs a source".to_owned()),
    }
}

fn apply_add_source(style: &mut Style, source: &str, mut declaration: Value) -> Result<(), String> {
    crate::sources::tilesets::resolve_source(&mut declaration)?;
    let parsed = serde_json::from_value(declaration)
        .map_err(|error| format!("addSource: invalid source: {error}"))?;
    style
        .add_source(source, parsed)
        .map(drop)
        .map_err(|error| error.to_string())
}

fn apply_source_layer_op(
    style: &mut Style,
    name: &str,
    items: &[Value],
) -> Option<Result<(), String>> {
    match (name, arg_text(items, 1)) {
        ("setFeatureState", _) => Some(feature_state(
            style,
            &arg_value(items, 1),
            &arg_value(items, 2),
            false,
        )),
        ("removeFeatureState", _) => Some(feature_state(
            style,
            &arg_value(items, 1),
            &arg_value(items, 2),
            true,
        )),
        ("getFeatureState", _) => Some(Ok(())),
        ("querySourceFeatures", _) => Some(apply_query_source_features(style, items)),
        ("setStyle", _) => Some(apply_set_style(style, arg_value(items, 1))),
        ("removeLayer", Some(layer)) => Some(
            style
                .remove_layer(layer)
                .map(drop)
                .map_err(|error| error.to_string()),
        ),
        ("removeSource", Some(source)) => Some(
            style
                .remove_source(source)
                .map(drop)
                .map_err(|error| error.to_string()),
        ),
        ("addSource", Some(source)) => Some(apply_add_source(style, source, arg_value(items, 2))),
        ("addLayer", _) => Some(
            style
                .add_layer(arg_value(items, 1), arg_text(items, 2))
                .map(drop)
                .map_err(|error| error.to_string()),
        ),
        _ => None,
    }
}

fn apply_one(
    style: &mut Style,
    operation: &Value,
    ctx: &mut ApplyContext<'_>,
) -> Result<(), String> {
    let Some(items) = operation.as_array() else {
        return Err(format!("Malformed operation: {operation}"));
    };
    let name = arg_text(items, 0).unwrap_or_default();
    let outcome = apply_camera_op(style, name, items, ctx)
        .or_else(|| apply_style_op(style, name, items, ctx.transitions))
        .or_else(|| apply_source_layer_op(style, name, items))
        .unwrap_or_else(|| Err(format!("Unsupported operation: {name}")));
    outcome.map_err(|error| format!("{name}: {error}"))
}

/// Applies every operation to `style`. An operation the mutation API has no counterpart for
/// is an error, so its fixture is reported instead of compared against the wrong state.
pub(crate) fn apply(
    style: &mut Style,
    operations: &[Value],
    (transitions, paused_tiles, zoom_change): (
        &mut super::transitions::Transitions,
        &mut std::collections::HashMap<String, f64>,
        &mut Option<ZoomChange>,
    ),
) -> Result<(), String> {
    let mut ctx = ApplyContext {
        transitions,
        paused_tiles,
        zoom_change,
    };
    for operation in operations {
        apply_one(style, operation, &mut ctx)?;
    }
    Ok(())
}
