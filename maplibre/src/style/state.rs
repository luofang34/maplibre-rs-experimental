//! `global-state`: values set once on the style that expressions of any layer can read.
//!
//! An expression is resolved when its value changes: the layer as declared is kept, every
//! `["global-state", key]` in its filter and properties is replaced by the key's current value,
//! and the layer is parsed again. Evaluation therefore needs no extra input, and a layer that
//! reads no state is never touched.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::style::{layer::StyleLayer, Style};

/// One key of the style's `state` property.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
pub struct StateDeclaration {
    /// The value used until the key is set; `null` when absent, as in GL JS.
    #[serde(default)]
    pub default: Option<Value>,
}

impl Style {
    /// The current value of a key: what was set, else its declared default, else `null`.
    pub fn global_state_value(&self, key: &str) -> Value {
        self.global_state
            .get(key)
            .or_else(|| self.state.get(key)?.default.as_ref())
            .cloned()
            .unwrap_or(Value::Null)
    }

    /// Every declared or set key with its current value, as GL JS `getGlobalState`.
    pub fn global_state_values(&self) -> BTreeMap<String, Value> {
        self.state
            .keys()
            .chain(self.global_state.keys())
            .map(|key| (key.clone(), self.global_state_value(key)))
            .collect()
    }

    /// Sets a key, as GL JS `setGlobalStateProperty`; `null` returns it to its default. Returns
    /// the ids of the layers whose filter or properties changed as a result.
    pub fn set_global_state(&mut self, key: &str, value: Value) -> Vec<String> {
        if value.is_null() {
            self.global_state.remove(key);
        } else {
            self.global_state.insert(key.to_owned(), value);
        }
        self.resolve_global_state()
    }

    /// Substitutes the current state into every layer that reads it and returns the ids of the
    /// layers that changed. Call it once after loading a style so declared defaults apply.
    pub fn resolve_global_state(&mut self) -> Vec<String> {
        let mut changed = Vec::new();
        for position in 0..self.layers.len() {
            let id = self.layers[position].id.clone();
            let declared = self
                .state_templates
                .get(&id)
                .unwrap_or(&self.layers[position]);
            let Ok(document) = serde_json::to_value(declared) else {
                continue;
            };
            if !reads_global_state(&document) {
                continue;
            }
            let template = declared.clone();
            let resolved = substitute_layer(&document, &|key| self.global_state_value(key));
            let Ok(mut layer) = serde_json::from_value::<StyleLayer>(resolved) else {
                tracing::warn!(layer = %id, "layer is invalid once global state is applied");
                continue;
            };
            layer.index = self.layers[position].index;
            let before = serde_json::to_value(&self.layers[position]).ok();
            let after = serde_json::to_value(&layer).ok();
            self.state_templates.entry(id.clone()).or_insert(template);
            if before != after {
                self.layers[position] = layer;
                changed.push(id);
            }
        }
        changed
    }
}

fn reads_global_state(value: &Value) -> bool {
    match value {
        Value::Array(items) => {
            is_global_state_reference(items) || items.iter().any(reads_global_state)
        }
        Value::Object(map) => map.values().any(reads_global_state),
        _ => false,
    }
}

fn is_global_state_reference(items: &[Value]) -> bool {
    matches!(items, [Value::String(operator), Value::String(_)] if operator == "global-state")
}

/// The layer with state substituted. A property or filter that is a bare reference becomes the
/// plain value; a reference nested in an expression becomes a `literal`, which keeps a string
/// from being read as a property name.
fn substitute_layer(layer: &Value, lookup: &dyn Fn(&str) -> Value) -> Value {
    let Value::Object(map) = layer else {
        return layer.clone();
    };
    let mut out = serde_json::Map::new();
    for (key, value) in map {
        let resolved = match key.as_str() {
            "paint" | "layout" => match value {
                Value::Object(properties) => Value::Object(
                    properties
                        .iter()
                        .map(|(name, property)| (name.clone(), substitute(property, lookup, true)))
                        .collect(),
                ),
                other => other.clone(),
            },
            "filter" => substitute(value, lookup, true),
            _ => value.clone(),
        };
        out.insert(key.clone(), resolved);
    }
    Value::Object(out)
}

fn substitute(value: &Value, lookup: &dyn Fn(&str) -> Value, whole: bool) -> Value {
    match value {
        Value::Array(items) => {
            if let [Value::String(operator), Value::String(key)] = items.as_slice() {
                if operator == "global-state" {
                    let current = lookup(key);
                    // A reference with no value stays: it evaluates to null, as the spec says,
                    // and keeps its dependency for the next change.
                    if current.is_null() {
                        return value.clone();
                    }
                    return if whole && !current.is_array() && !current.is_object() {
                        current
                    } else {
                        json!(["literal", current])
                    };
                }
            }
            Value::Array(
                items
                    .iter()
                    .map(|item| substitute(item, lookup, false))
                    .collect(),
            )
        }
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, item)| (key.clone(), substitute(item, lookup, false)))
                .collect(),
        ),
        other => other.clone(),
    }
}

impl crate::context::MapContext {
    /// Sets a global state key and asks the request system to fetch the loaded tiles of every
    /// vector layer that read it again; their old content stays until the new tiles arrive.
    pub fn set_global_state(&mut self, key: &str, value: Value) {
        if !self.style.set_global_state(key, value).is_empty() {
            crate::io::tile_retry::refresh(
                &mut self.world,
                crate::io::tile_retry::RequestKind::Vector,
            );
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;
