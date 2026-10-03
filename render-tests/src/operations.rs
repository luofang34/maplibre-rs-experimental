//! Applies the runtime operations a fixture lists before its comparison.
//!
//! GL JS fixtures change a map after it loads (`setPaintProperty`, `addLayer`, `setStyle`, ...)
//! and compare the result. The harness applies them to the style in order, through the same
//! mutation API a host uses, and renders the final state.

use maplibre::style::Style;
use serde_json::Value;

/// The operations under `metadata.test.operations`, in order.
pub(super) fn operations_of(style: &Value) -> Vec<Value> {
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

/// Whether a feature's id, or its promoted property, is `wanted`.
fn has_id(feature: &Value, promote: Option<&str>, wanted: &str) -> bool {
    let id = match promote {
        Some(property) => feature
            .get("properties")
            .and_then(|props| props.get(property)),
        None => feature.get("id"),
    };
    match id {
        Some(Value::String(text)) => text == wanted,
        Some(Value::Number(number)) => number.to_string() == wanted,
        _ => false,
    }
}

/// Stores the state of one GeoJSON feature among its properties, where `feature-state`
/// expressions read it. `removing` deletes the named keys, or all of them for `null`.
fn feature_state(
    style: &mut Style,
    target: &Value,
    states: &Value,
    (removing, vector_states): (bool, &mut crate::vector_feature_state::VectorFeatureStates),
) -> Result<(), String> {
    use maplibre::style::{
        expression::FEATURE_STATE_PREFIX,
        source::{GeoJsonData, PromoteId, Source},
    };

    let source = target
        .get("source")
        .and_then(Value::as_str)
        .ok_or("feature state needs a source")?;
    // Removing state without an id removes it from every feature of the source.
    let wanted = match target.get("id") {
        Some(Value::String(text)) => Some(text.clone()),
        Some(Value::Number(number)) => Some(number.to_string()),
        None if removing => None,
        _ => return Err("feature state needs an id".to_owned()),
    };
    if matches!(style.sources.get(source), Some(Source::Vector(_))) {
        return if removing {
            vector_states.remove(target, states)
        } else {
            vector_states.set(target, states)
        };
    }
    let Some(Source::GeoJson(geojson)) = style.sources.get_mut(source) else {
        return Err(format!(
            "feature state on `{source}`: only GeoJSON and vector sources are supported"
        ));
    };
    let promote = match &geojson.promote_id {
        Some(PromoteId::Property(property)) => Some(property.clone()),
        _ => None,
    };
    let GeoJsonData::Inline(data) = &mut geojson.data else {
        return Err("feature state needs inline GeoJSON".to_owned());
    };
    let mut document = data.as_ref().clone();
    let features: Vec<&mut Value> = match document.get("type").and_then(Value::as_str) {
        Some("FeatureCollection") => document
            .get_mut("features")
            .and_then(Value::as_array_mut)
            .map(|features| features.iter_mut().collect())
            .unwrap_or_default(),
        Some("Feature") => vec![&mut document],
        _ => Vec::new(),
    };
    for feature in features {
        if wanted
            .as_deref()
            .is_some_and(|wanted| !has_id(feature, promote.as_deref(), wanted))
        {
            continue;
        }
        let Some(properties) = feature
            .as_object_mut()
            .map(|object| {
                object
                    .entry("properties")
                    .or_insert_with(|| Value::Object(Default::default()))
            })
            .and_then(Value::as_object_mut)
        else {
            continue;
        };
        match (removing, states) {
            (true, Value::String(key)) => {
                properties.remove(&format!("{FEATURE_STATE_PREFIX}{key}"));
            }
            (true, _) => properties.retain(|key, _| !key.starts_with(FEATURE_STATE_PREFIX)),
            (false, Value::Object(values)) => {
                for (key, value) in values {
                    properties.insert(format!("{FEATURE_STATE_PREFIX}{key}"), value.clone());
                }
            }
            (false, _) => return Err("setFeatureState needs an object of states".to_owned()),
        }
    }
    *data = std::sync::Arc::new(document);
    Ok(())
}

/// A `setZoom` the fixture made: the zoom it left and the clock when it did.
#[derive(Clone, Copy, Debug)]
pub(super) struct ZoomChange {
    /// The zoom before the change.
    pub previous_zoom: f64,
    /// Milliseconds the fixture had waited when it changed the zoom.
    pub at: f64,
}

/// Applies every operation to `style`. An operation the mutation API has no counterpart for
/// is an error, so its fixture is reported instead of compared against the wrong state.
pub(super) fn apply(
    style: &mut Style,
    operations: &[Value],
    (transitions, vector_states, paused_tiles, zoom_change): (
        &mut crate::transitions::Transitions,
        &mut crate::vector_feature_state::VectorFeatureStates,
        &mut std::collections::HashMap<String, f64>,
        &mut Option<ZoomChange>,
    ),
) -> Result<(), String> {
    for operation in operations {
        let Some(items) = operation.as_array() else {
            return Err(format!("Malformed operation: {operation}"));
        };
        let name = items.first().and_then(Value::as_str).unwrap_or_default();
        let text = |index: usize| items.get(index).and_then(Value::as_str);
        let value = |index: usize| items.get(index).cloned().unwrap_or(Value::Null);
        let outcome = match (name, text(1)) {
            // Rendering is deterministic, so waiting for the map to settle changes nothing.
            ("wait" | "idle" | "sleep", _) => {
                transitions.wait(items.get(1).and_then(Value::as_f64).unwrap_or(0.0));
                Ok(())
            }
            // The source keeps the tiles its view needed now, and later views draw those.
            ("pauseTiles", Some(source)) => {
                paused_tiles.insert(source.to_owned(), style.zoom.unwrap_or(0.0));
                Ok(())
            }
            // Only the camera left by the last operation is drawn, so it becomes the style's.
            ("setZoom", _) => number(items, 1).map(|zoom| {
                *zoom_change = Some(ZoomChange {
                    previous_zoom: style.zoom.unwrap_or(0.0),
                    at: transitions.now(),
                });
                style.zoom = Some(zoom);
            }),
            ("setBearing", _) => number(items, 1).map(|bearing| style.bearing = Some(bearing)),
            ("setPitch", _) => number(items, 1).map(|pitch| style.pitch = Some(pitch)),
            ("setRoll", _) => number(items, 1).map(|roll| style.roll = Some(roll)),
            // The padding is applied to the camera by the harness, not to the style.
            // The harness applies these to the map's camera, not to the style.
            (
                "setPadding"
                | "setCenterClampedToGround"
                | "setCenterElevation"
                | "setVerticalFieldOfView",
                _,
            ) => Ok(()),
            ("easeTo", _) if crate::camera_options::eases_only_padding(&value(1)) => Ok(()),
            ("addImage", Some(name)) => add_image(style, name, &value(2), &value(3)),
            ("removeImage", Some(name)) => {
                style.remove_image(name);
                Ok(())
            }
            ("setLight", _) => {
                style.light = Some(
                    serde_json::from_value(value(1))
                        .map_err(|error| format!("setLight: invalid light: {error}"))?,
                );
                Ok(())
            }
            ("setTerrain", _) => {
                // Called without an argument, it removes the terrain.
                style.terrain = match value(1) {
                    Value::Null => None,
                    terrain => Some(
                        serde_json::from_value(terrain)
                            .map_err(|error| format!("setTerrain: invalid terrain: {error}"))?,
                    ),
                };
                Ok(())
            }
            ("setFeatureState", _) => {
                feature_state(style, &value(1), &value(2), (false, &mut *vector_states))
            }
            ("removeFeatureState", _) => {
                feature_state(style, &value(1), &value(2), (true, &mut *vector_states))
            }
            ("setGlobalStateProperty", Some(key)) => {
                style.set_global_state(key, value(2));
                Ok(())
            }
            ("setLayerZoomRange", Some(layer)) => {
                let (minzoom, maxzoom) = (number(items, 2)?, number(items, 3)?);
                let layer = style
                    .layers
                    .iter_mut()
                    .find(|candidate| candidate.id == layer)
                    .ok_or_else(|| format!("setLayerZoomRange: no layer `{layer}`"))?;
                layer.minzoom = Some(minzoom);
                layer.maxzoom = Some(maxzoom);
                Ok(())
            }
            ("setCenter", _) => match value(1).as_array().map(Vec::as_slice) {
                Some([longitude, latitude]) => match (longitude.as_f64(), latitude.as_f64()) {
                    (Some(longitude), Some(latitude)) => {
                        style.center = Some([longitude, latitude]);
                        Ok(())
                    }
                    _ => Err("setCenter needs numbers".to_owned()),
                },
                _ => Err("setCenter needs [longitude, latitude]".to_owned()),
            },
            ("setStyle", _) => {
                let replacement = match value(1) {
                    Value::String(url) => {
                        let path = crate::paths::local_style_path(&url)?;
                        let text = std::fs::read_to_string(&path)
                            .map_err(|error| format!("Cannot read {}: {error}", path.display()))?;
                        serde_json::from_str(&text)
                            .map_err(|error| format!("Cannot parse {}: {error}", path.display()))?
                    }
                    other => other,
                };
                let mut replacement = replacement;
                crate::tilesets::resolve_sources(&mut replacement)?;
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
            ("setPaintProperty", Some(layer)) => text(2)
                .ok_or_else(|| "setPaintProperty needs a property".to_owned())
                .and_then(|property| {
                    if property.ends_with("-transition") {
                        transitions.declare(layer, property, &value(3));
                        return Ok(());
                    }
                    transitions.before_change(style, layer, property, &value(3));
                    style
                        .set_paint_property(layer, property, value(3))
                        .map(drop)
                        .map_err(|error| error.to_string())
                }),
            ("setLayoutProperty", Some(layer)) => text(2)
                .ok_or_else(|| "setLayoutProperty needs a property".to_owned())
                .and_then(|property| {
                    style
                        .set_layout_property(layer, property, value(3))
                        .map(drop)
                        .map_err(|error| error.to_string())
                }),
            ("setFilter", Some(layer)) => {
                let filter = value(2);
                style
                    .set_filter(layer, (!filter.is_null()).then_some(filter))
                    .map(drop)
                    .map_err(|error| error.to_string())
            }
            ("removeLayer", Some(layer)) => style
                .remove_layer(layer)
                .map(drop)
                .map_err(|error| error.to_string()),
            ("removeSource", Some(source)) => style
                .remove_source(source)
                .map(drop)
                .map_err(|error| error.to_string()),
            ("addSource", Some(source)) => {
                let mut declaration = value(2);
                crate::tilesets::resolve_source(&mut declaration)?;
                serde_json::from_value(declaration)
            }
            .map_err(|error| format!("addSource: invalid source: {error}"))
            .and_then(|parsed| {
                style
                    .add_source(source, parsed)
                    .map(drop)
                    .map_err(|error| error.to_string())
            }),
            ("addLayer", _) => style
                .add_layer(value(1), text(2))
                .map(drop)
                .map_err(|error| error.to_string()),
            _ => Err(format!("Unsupported operation: {name}")),
        };
        outcome.map_err(|error| format!("{name}: {error}"))?;
    }
    Ok(())
}
