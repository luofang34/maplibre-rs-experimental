//! Declarative `*-transition` timing and runtime paint property interpolation.
//!
//! Freshly deserialized styles draw their declared paint values immediately without initial
//! fade-in. Subsequent [`Style::set_paint_property`] and [`Style::diff_and_replace`] mutations
//! interpolate smoothly when the target property's layer `<prop>-transition` (or root
//! [`Style::transition`]) specifies a positive `duration` or `delay`.

use serde_json::Value;

pub use crate::style::property::{ActiveTransition, ActiveTransitions, TransitionSpec};
use crate::style::{
    mutation::{StyleChange, StyleMutationError},
    validation::{PropertyValidationError, StyleValidationError},
    Style,
};

mod interpolate;
pub(crate) use interpolate::default_paint_json;
pub use interpolate::{interpolate_paint_value, is_transitionable_property};

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
    /// Sets a paint property; `null` restores its default. When the layer or root style declares
    /// an active `transition` for `name`, starts an in-flight paint transition.
    pub fn set_paint_property(
        &mut self,
        layer: &str,
        name: &str,
        value: Value,
    ) -> Result<StyleChange, StyleMutationError> {
        self.apply_paint_with_transition(layer, name, value)
    }
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

    /// Overrides timing or start timestamp metadata on an in-flight paint transition.
    #[doc(hidden)]
    pub fn override_transition_entry(
        &mut self,
        layer_id: &str,
        property: &str,
        timing: Option<TransitionSpec>,
        started_ms: Option<f64>,
    ) {
        self.active_transitions
            .override_entry(layer_id, property, timing, started_ms);
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
            return self.apply_transition_spec(layer, name, base, value);
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

    fn apply_transition_spec(
        &mut self,
        layer: &str,
        name: &str,
        base: &str,
        value: Value,
    ) -> Result<StyleChange, StyleMutationError> {
        let target_layer = self
            .layers
            .iter_mut()
            .find(|candidate| candidate.id == layer)
            .ok_or_else(|| StyleMutationError::UnknownLayer {
                layer: layer.to_owned(),
            })?;
        if value.is_null() {
            let removed = target_layer.transitions.remove(base).is_some();
            return Ok(if removed {
                StyleChange {
                    layers: vec![layer.to_owned()],
                    ..Default::default()
                }
            } else {
                StyleChange::default()
            });
        }
        let spec = TransitionSpec::from_json_value(&value).map_err(|source| {
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
                        reason: "transition duration and delay must be non-negative finite numbers",
                    },
                }],
            });
        }
        let unchanged = target_layer.transitions.get(base) == Some(&spec);
        target_layer.transitions.insert(base.to_owned(), spec);
        Ok(if unchanged {
            StyleChange::default()
        } else {
            StyleChange {
                layers: vec![layer.to_owned()],
                ..Default::default()
            }
        })
    }
}

#[cfg(test)]
mod tests;
