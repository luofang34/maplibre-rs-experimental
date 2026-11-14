//! Declarative `*-transition` timing and runtime paint property interpolation.
//!
//! Freshly deserialized styles draw their declared paint values immediately without initial
//! fade-in. Subsequent [`Style::set_paint_property`] and [`Style::diff_and_replace`] mutations
//! interpolate smoothly when the target property's layer `<prop>-transition` (or root
//! [`Style::transition`]) specifies a positive `duration` or `delay`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{
    mutation::{StyleChange, StyleMutationError},
    validation::{PropertyValidationError, StyleValidationError},
    Style,
};

mod interpolate;
pub(crate) use interpolate::default_paint_json;
pub use interpolate::{interpolate_paint_value, is_transitionable_property};

const DEFAULT_TRANSITION_DURATION_MS: f64 = 300.0;

fn default_duration() -> f64 {
    DEFAULT_TRANSITION_DURATION_MS
}

/// Duration and delay, in milliseconds, of a paint property transition.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
pub struct TransitionSpec {
    /// Duration of the interpolation in milliseconds (`300.0` when omitted in JSON).
    #[serde(default = "default_duration")]
    pub duration: f64,
    /// Delay before interpolation starts in milliseconds (`0.0` when omitted in JSON).
    #[serde(default)]
    pub delay: f64,
}

impl Default for TransitionSpec {
    fn default() -> Self {
        Self {
            duration: DEFAULT_TRANSITION_DURATION_MS,
            delay: 0.0,
        }
    }
}

impl TransitionSpec {
    /// Whether this transition has a positive duration or delay.
    pub fn is_active(self) -> bool {
        self.duration > 0.0 || self.delay > 0.0
    }

    pub(crate) fn is_valid(self) -> bool {
        self.duration.is_finite()
            && self.delay.is_finite()
            && self.duration >= 0.0
            && self.delay >= 0.0
    }
}

/// One in-flight paint property transition on a style layer.
#[derive(Debug, Clone, PartialEq)]
pub struct ActiveTransition {
    /// Target layer identifier.
    pub layer: String,
    /// Base paint property name (without `-transition`).
    pub property: String,
    /// Starting paint JSON value at `progress = 0.0`.
    pub from: Value,
    /// Destination paint JSON value committed when `progress >= 1.0`.
    pub to: Value,
    /// Duration and delay governing this transition.
    pub timing: TransitionSpec,
    /// Transition clock timestamp (milliseconds) when the mutation started.
    pub started_ms: f64,
}

/// Runtime clock and in-flight paint transitions carried by a [`Style`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ActiveTransitions {
    clock_ms: f64,
    entries: Vec<ActiveTransition>,
}

impl ActiveTransitions {
    pub(crate) fn with_clock(clock_ms: f64) -> Self {
        Self {
            clock_ms: clock_ms.max(0.0),
            entries: Vec::new(),
        }
    }

    /// Current transition clock in milliseconds.
    pub fn clock_ms(&self) -> f64 {
        self.clock_ms
    }

    /// Whether at least one paint property is currently mid-transition.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Slice of currently active paint transitions.
    pub fn entries(&self) -> &[ActiveTransition] {
        &self.entries
    }

    pub(crate) fn remove_layer(&mut self, layer_id: &str) {
        self.entries.retain(|entry| entry.layer != layer_id);
    }

    pub(crate) fn remove_property(&mut self, layer_id: &str, property: &str) {
        self.entries
            .retain(|entry| !(entry.layer == layer_id && entry.property == property));
    }

    /// Overrides the timing and start timestamp of an active transition, used by harness replay.
    pub fn override_entry(
        &mut self,
        layer_id: &str,
        property: &str,
        timing: Option<TransitionSpec>,
        started_ms: Option<f64>,
    ) {
        for entry in &mut self.entries {
            if entry.layer == layer_id && entry.property == property {
                if let Some(spec) = timing {
                    entry.timing = spec;
                }
                if let Some(started) = started_ms {
                    entry.started_ms = started;
                }
            }
        }
    }
}

/// Extracts `<prop>-transition` entries from a raw layer `paint` JSON object.
pub(crate) fn extract_layer_transitions(
    layer_id: &str,
    paint: &mut Option<Value>,
) -> Result<BTreeMap<String, TransitionSpec>, String> {
    let mut extracted = BTreeMap::new();
    let Some(Value::Object(map)) = paint.as_mut() else {
        return Ok(extracted);
    };
    let keys: Vec<String> = map
        .keys()
        .filter(|key| key.ends_with("-transition"))
        .cloned()
        .collect();
    for key in keys {
        let Some(raw) = map.remove(&key) else {
            continue;
        };
        let Some(base) = key.strip_suffix("-transition") else {
            continue;
        };
        if raw.is_null() {
            continue;
        }
        let spec: TransitionSpec = serde_json::from_value(raw)
            .map_err(|error| format!("layer `{layer_id}` paint.{key}: {error}"))?;
        if !spec.is_valid() {
            return Err(format!(
                "layer `{layer_id}` paint.{key}: duration and delay must be non-negative finite numbers"
            ));
        }
        extracted.insert(base.to_owned(), spec);
    }
    Ok(extracted)
}

fn merge_change(into: &mut StyleChange, next: StyleChange) {
    for layer in next.layers {
        if !into.layers.contains(&layer) {
            into.layers.push(layer);
        }
    }
    for layer in next.removed_layers {
        if !into.removed_layers.contains(&layer) {
            into.removed_layers.push(layer);
        }
    }
    into.redraw_tiles |= next.redraw_tiles;
}

impl Style {
    /// Effective transition timing for `property` on `layer`, falling back to root [`Self::transition`].
    pub fn effective_transition(&self, layer_id: &str, property: &str) -> Option<TransitionSpec> {
        let layer_spec = self
            .layers
            .iter()
            .find(|candidate| candidate.id == layer_id)
            .and_then(|candidate| candidate.transitions.get(property).copied());
        layer_spec.or(self.transition)
    }

    /// Whether any paint property is currently mid-transition.
    pub fn has_active_transitions(&self) -> bool {
        !self.active_transitions.is_empty()
    }

    /// Drops all in-flight transitions without changing current paint values.
    pub fn clear_transitions(&mut self) {
        self.active_transitions.entries.clear();
    }

    /// Advances the transition clock by `delta_ms` milliseconds and updates every active paint
    /// transition to its interpolated or settled value.
    pub fn advance_transitions(&mut self, delta_ms: f64) -> StyleChange {
        let next_clock = self.active_transitions.clock_ms + delta_ms.max(0.0);
        self.settle_transitions_at(next_clock)
    }

    /// Evaluates every active paint transition at absolute `clock_ms` milliseconds, removing
    /// completed transitions once their target value is committed.
    pub fn settle_transitions_at(&mut self, clock_ms: f64) -> StyleChange {
        let clock = clock_ms.max(0.0);
        self.active_transitions.clock_ms = clock;
        if self.active_transitions.entries.is_empty() {
            return StyleChange::default();
        }

        let snapshot = std::mem::take(&mut self.active_transitions.entries);
        let mut remaining = Vec::with_capacity(snapshot.len());
        let mut combined = StyleChange::default();

        for entry in snapshot {
            let elapsed = clock - entry.started_ms - entry.timing.delay;
            if elapsed <= 0.0 && entry.timing.is_active() {
                if let Some(initial) = interpolate_paint_value(&entry.from, &entry.to, 0.0) {
                    if let Ok(change) =
                        self.set_paint_property_immediate(&entry.layer, &entry.property, initial)
                    {
                        merge_change(&mut combined, change);
                    }
                }
                remaining.push(entry);
                continue;
            }
            if entry.timing.duration <= 0.0 || elapsed >= entry.timing.duration {
                if let Ok(change) = self.set_paint_property_immediate(
                    &entry.layer,
                    &entry.property,
                    entry.to.clone(),
                ) {
                    merge_change(&mut combined, change);
                }
                continue;
            }
            let progress = (elapsed / entry.timing.duration).clamp(0.0, 1.0);
            if let Some(value) = interpolate_paint_value(&entry.from, &entry.to, progress) {
                if let Ok(change) =
                    self.set_paint_property_immediate(&entry.layer, &entry.property, value)
                {
                    merge_change(&mut combined, change);
                }
            }
            remaining.push(entry);
        }

        self.active_transitions.entries = remaining;
        combined
    }

    pub(crate) fn apply_paint_with_transition(
        &mut self,
        layer: &str,
        name: &str,
        value: Value,
    ) -> Result<StyleChange, StyleMutationError> {
        if let Some(base) = name.strip_suffix("-transition") {
            let position = self
                .layers
                .iter()
                .position(|candidate| candidate.id == layer)
                .ok_or_else(|| StyleMutationError::UnknownLayer {
                    layer: layer.to_owned(),
                })?;
            if value.is_null() {
                let removed = self.layers[position].transitions.remove(base).is_some();
                return Ok(if removed {
                    StyleChange {
                        layers: vec![layer.to_owned()],
                        ..Default::default()
                    }
                } else {
                    StyleChange::default()
                });
            }
            let spec: TransitionSpec = serde_json::from_value(value).map_err(|source| {
                StyleMutationError::InvalidLayer {
                    layer: layer.to_owned(),
                    source,
                }
            })?;
            if !spec.is_valid() {
                return Err(StyleMutationError::Unsupported {
                    layer: layer.to_owned(),
                    findings: vec![StyleValidationError::Property {
                        layer: layer.to_owned(),
                        property: format!("paint.{name}"),
                        source: PropertyValidationError::Unsupported {
                            reason:
                                "transition duration and delay must be non-negative finite numbers",
                        },
                    }],
                });
            }
            let unchanged = self.layers[position].transitions.get(base) == Some(&spec);
            self.layers[position]
                .transitions
                .insert(base.to_owned(), spec);
            return Ok(if unchanged {
                StyleChange::default()
            } else {
                StyleChange {
                    layers: vec![layer.to_owned()],
                    ..Default::default()
                }
            });
        }

        let previous = self
            .layers
            .iter()
            .find(|candidate| candidate.id == layer)
            .and_then(|candidate| {
                candidate
                    .property_maps()
                    .ok()
                    .and_then(|maps| maps.paint.get(name).cloned())
                    .or_else(|| default_paint_json(candidate, name))
            });
        let timing = self.effective_transition(layer, name);
        let was_transitioning = self
            .active_transitions
            .entries
            .iter()
            .any(|entry| entry.layer == layer && entry.property == name);

        let target_value = value.clone();
        let mut change = self.set_paint_property_immediate(layer, name, value)?;

        let should_animate = is_transitionable_property(name)
            && timing.is_some_and(TransitionSpec::is_active)
            && (!change.layers.is_empty() || was_transitioning)
            && previous
                .as_ref()
                .is_some_and(|from| interpolate_paint_value(from, &target_value, 0.5).is_some());

        self.active_transitions.remove_property(layer, name);
        if let (true, Some(spec), Some(from)) = (should_animate, timing, previous) {
            if interpolate_paint_value(&from, &target_value, 0.0)
                != interpolate_paint_value(&from, &target_value, 1.0)
            {
                let start_val = interpolate_paint_value(&from, &target_value, 0.0)
                    .unwrap_or_else(|| from.clone());
                let revert = self.set_paint_property_immediate(layer, name, start_val)?;
                merge_change(&mut change, revert);
                if !change.layers.iter().any(|id| id == layer) {
                    change.layers.push(layer.to_owned());
                }
                self.active_transitions.entries.push(ActiveTransition {
                    layer: layer.to_owned(),
                    property: name.to_owned(),
                    from,
                    to: target_value,
                    timing: spec,
                    started_ms: self.active_transitions.clock_ms,
                });
            }
        }
        Ok(change)
    }
}

#[cfg(test)]
mod tests;
