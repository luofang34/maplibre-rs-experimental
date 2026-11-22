//! `global-state`: values set once on the style that expressions of any layer can read.
//!
//! An expression is resolved when its value changes: the layer as declared is kept, every
//! `["global-state", key]` in its filter and properties is replaced by the key's current value,
//! and the layer is parsed again. Evaluation therefore needs no extra input, and a layer that
//! reads no state is never touched.

use std::collections::BTreeMap;

use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};
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
        self.bump_revision();
        self.resolve_global_state()
    }

    /// The layer at `position` as declared: with its global-state references, unless it was
    /// edited since it was last resolved.
    pub(super) fn declared_layer_at(&self, position: usize) -> StyleLayer {
        let Some(layer) = self.layers.get(position) else {
            return StyleLayer::default();
        };
        let current = serde_json::to_value(layer).ok();
        match self.state_templates.get(&layer.id) {
            Some(template) if Some(&template.resolved) == current.as_ref() => {
                template.declared.clone()
            }
            _ => layer.clone(),
        }
    }

    /// Substitutes the current state into every layer that reads it and returns the ids of the
    /// layers that changed. Call it once after loading a style so declared defaults apply.
    ///
    /// A layer that was replaced or edited since it was last resolved is taken as newly declared,
    /// layers that share an id are left alone, and templates of layers that are gone are dropped.
    pub fn resolve_global_state(&mut self) -> Vec<String> {
        self.resolve_light();
        self.resolve_projection();
        let mut counts: HashMap<&str, usize> = HashMap::default();
        for layer in &self.layers {
            *counts.entry(layer.id.as_str()).or_default() += 1;
        }
        let duplicated: HashSet<String> = counts
            .into_iter()
            .filter(|(_, count)| *count > 1)
            .map(|(id, _)| id.to_owned())
            .collect();
        let present: HashSet<String> = self.layers.iter().map(|layer| layer.id.clone()).collect();
        self.state_templates.retain(|id, _| present.contains(id));
        let mut changed = Vec::new();
        let mut scratch = Vec::with_capacity(4096);
        for position in 0..self.layers.len() {
            if let Some(id) = self.resolve_layer_global_state(position, &duplicated, &mut scratch) {
                changed.push(id);
            }
        }
        if !changed.is_empty() {
            self.bump_revision();
        }
        changed
    }

    fn resolve_layer_global_state(
        &mut self,
        position: usize,
        duplicated: &HashSet<String>,
        scratch: &mut Vec<u8>,
    ) -> Option<String> {
        let layer_snapshot = self.layers.get(position)?;
        let id = layer_snapshot.id.clone();
        if duplicated.contains(&id) {
            tracing::warn!(layer = %id, "layers share an id, so global state is not applied to them");
            self.state_templates.remove(&id);
            return None;
        }
        scratch.clear();
        serde_json::to_writer(&mut *scratch, layer_snapshot).ok()?;
        let has_template = self.state_templates.contains_key(&id);
        if !has_template
            && !scratch
                .windows(b"\"global-state\"".len())
                .any(|w| w == b"\"global-state\"")
        {
            return None;
        }
        let current = serde_json::from_slice::<Value>(scratch).ok()?;
        let (declared, document) = match self.state_templates.get(&id) {
            Some(template) if template.resolved == current => {
                let declared = template.declared.clone();
                let document = serde_json::to_value(&declared).ok()?;
                (declared, document)
            }
            _ => (layer_snapshot.clone(), current.clone()),
        };
        if !reads_global_state(&document) {
            self.state_templates.remove(&id);
            return None;
        }
        let resolved = substitute_layer(&document, &|key| self.global_state_value(key));
        let Ok(mut layer) = StyleLayer::from_json_value(&resolved) else {
            tracing::warn!(layer = %id, "layer is invalid once global state is applied");
            return None;
        };
        layer.index = self.layers.get(position)?.index;
        let after = serde_json::to_value(&layer).ok()?;
        self.state_templates.insert(
            id.clone(),
            StateTemplate {
                declared,
                resolved: after.clone(),
            },
        );
        if current != after {
            if let Some(slot) = self.layers.get_mut(position) {
                *slot = layer;
            }
            return Some(id);
        }
        None
    }
}

impl Style {
    /// Substitutes the current state into the light, as into a layer's paint properties.
    fn resolve_light(&mut self) {
        if self.light_template.is_none() {
            let reads = self
                .light
                .as_ref()
                .and_then(|light| serde_json::to_value(light).ok())
                .is_some_and(|light| reads_global_state(&light));
            if !reads {
                return;
            }
            self.light_template = self.light.clone();
        }
        let Some(Value::Object(declared)) = self
            .light_template
            .as_ref()
            .and_then(|light| serde_json::to_value(light).ok())
        else {
            return;
        };
        let lookup = |key: &str| self.global_state_value(key);
        let resolved = declared
            .iter()
            .map(|(name, property)| (name.clone(), substitute(property, &lookup, true)))
            .collect();
        match crate::style::light::LightSpecification::from_json_value(&Value::Object(resolved)) {
            Ok(light) => self.light = Some(light),
            Err(error) => tracing::warn!(%error, "light is invalid once global state is applied"),
        }
    }

    /// Substitutes the current state into the projection specification when it reads global state.
    fn resolve_projection(&mut self) {
        let Some(template) = self.projection_template.as_ref() else {
            return;
        };
        let resolved = resolve_projection_json(template, &|key| self.global_state_value(key));
        if self.projection != resolved {
            self.projection = resolved;
            self.bump_revision();
        }
    }
}

/// Resolves a raw `projection` JSON object containing `global-state` references.
pub(super) fn resolve_projection_json(
    raw: &Value,
    lookup: &dyn Fn(&str) -> Value,
) -> Option<crate::projection::ProjectionSpecification> {
    let Value::Object(declared) = raw else {
        return None;
    };
    let resolved = declared
        .iter()
        .map(|(name, property)| (name.clone(), substitute(property, lookup, true)))
        .collect();
    match crate::projection::ProjectionSpecification::from_json_value(&Value::Object(resolved)) {
        Ok(projection) => Some(projection),
        Err(error) => {
            tracing::warn!(%error, "projection is invalid once global state is applied");
            None
        }
    }
}

/// A layer as declared, with the resolved form it produced, so a later edit is noticed.
#[doc(hidden)]
#[derive(Debug, Clone)]
pub struct StateTemplate {
    declared: StyleLayer,
    resolved: Value,
}

pub(super) fn reads_global_state(value: &Value) -> bool {
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
    /// Sets a global state key, updating paint-only layer dependencies in-place on resident GPU
    /// buffers and refetching loaded vector tiles only when layout, filter, or geometry changed.
    pub fn set_global_state(&mut self, key: &str, value: Value) {
        let before: HashMap<String, StyleLayer> = self
            .style
            .layers
            .iter()
            .map(|layer| (layer.id.clone(), layer.clone()))
            .collect();
        let changed = self.style.set_global_state(key, value);
        crate::render::frame_signals::mark_dirty(&mut self.world);
        if changed.is_empty() {
            return;
        }
        let needs_redraw = self.style.layers.iter().any(|after| {
            changed.contains(&after.id)
                && before
                    .get(&after.id)
                    .is_none_or(|prev| super::mutation::classify::needs_tile_redraw(prev, after))
        });
        crate::vector::upload_system::style_refresh::DirtyVectorStyle::mark_style_edit(
            &mut self.world,
            changed,
        );
        if needs_redraw {
            crate::io::tile_retry::refresh(
                &mut self.world,
                crate::io::tile_retry::RequestKind::Vector,
            );
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;
