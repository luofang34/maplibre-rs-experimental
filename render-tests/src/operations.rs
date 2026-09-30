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

/// Applies every operation to `style`. An operation the mutation API has no counterpart for
/// is an error, so its fixture is reported instead of compared against the wrong state.
pub(super) fn apply(style: &mut Style, operations: &[Value]) -> Result<(), String> {
    for operation in operations {
        let Some(items) = operation.as_array() else {
            return Err(format!("Malformed operation: {operation}"));
        };
        let name = items.first().and_then(Value::as_str).unwrap_or_default();
        let text = |index: usize| items.get(index).and_then(Value::as_str);
        let value = |index: usize| items.get(index).cloned().unwrap_or(Value::Null);
        let outcome = match (name, text(1)) {
            // Rendering is deterministic, so waiting for the map to settle changes nothing.
            ("wait" | "idle" | "sleep", _) => Ok(()),
            // Only the camera left by the last operation is drawn, so it becomes the style's.
            ("setZoom", _) => number(items, 1).map(|zoom| style.zoom = Some(zoom)),
            ("setBearing", _) => number(items, 1).map(|bearing| style.bearing = Some(bearing)),
            ("setPitch", _) => number(items, 1).map(|pitch| style.pitch = Some(pitch)),
            ("setRoll", _) => number(items, 1).map(|roll| style.roll = Some(roll)),
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
                *style = serde_json::from_value(replacement)
                    .map_err(|error| format!("setStyle: invalid style: {error}"))?;
                Ok(())
            }
            ("setPaintProperty", Some(layer)) => text(2)
                .ok_or_else(|| "setPaintProperty needs a property".to_owned())
                .and_then(|property| {
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
            ("addSource", Some(source)) => serde_json::from_value(value(2))
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
