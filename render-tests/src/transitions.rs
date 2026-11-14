//! Adapter bridging render-test fixture `wait` operations onto [`maplibre::style::Style`]'s
//! core paint transition engine.

use maplibre::style::{transitions::TransitionSpec, Style};
use serde_json::{json, Value};

/// Records fixture clock steps and runtime `*-transition` declarations applied before rendering.
#[derive(Default)]
pub(super) struct Transitions {
    declared: Vec<(String, String, Value)>,
    started_at: Vec<(String, String, f64)>,
    clock: f64,
}

fn has_positive_numeric_wait(style: &Value) -> bool {
    style
        .pointer("/metadata/test/operations")
        .and_then(Value::as_array)
        .is_some_and(|ops| {
            ops.iter().any(|op| {
                let Some(items) = op.as_array() else {
                    return false;
                };
                matches!(
                    items.first().and_then(Value::as_str),
                    Some("wait" | "sleep")
                ) && items
                    .get(1)
                    .and_then(Value::as_f64)
                    .is_some_and(|ms| ms > 0.0)
            })
        })
}

impl Transitions {
    /// Leaves layer `*-transition` entries in `style` for [`Style`] to parse natively, seeding
    /// the spec's 300 ms root `transition` when a fixture omits `transition` and steps the clock.
    pub(super) fn extract(style: &mut Value) -> Self {
        if style.get("transition").is_none() && has_positive_numeric_wait(style) {
            if let Some(root) = style.as_object_mut() {
                root.insert(
                    "transition".into(),
                    json!({ "duration": 300.0, "delay": 0.0 }),
                );
            }
        }
        Self::default()
    }

    /// Records a runtime `<prop>-transition` mutation intercepted by `operations::apply`.
    pub(super) fn declare(&mut self, layer: &str, property: &str, options: &Value) {
        self.declared
            .push((layer.to_owned(), property.to_owned(), options.clone()));
    }

    /// Records the fixture timestamp at which `property` on `layer` began transitioning.
    pub(super) fn before_change(
        &mut self,
        _style: &Style,
        layer: &str,
        property: &str,
        _to: &Value,
    ) {
        self.started_at
            .retain(|(l, p, _)| !(l == layer && p == property));
        self.started_at
            .push((layer.to_owned(), property.to_owned(), self.clock));
    }

    /// Milliseconds the fixture has waited so far.
    pub(super) fn now(&self) -> f64 {
        self.clock
    }

    /// Advances the fixture clock by `milliseconds`.
    pub(super) fn wait(&mut self, milliseconds: f64) {
        self.clock += milliseconds;
    }

    /// Settles all active [`Style`] transitions at the fixture's waited timestamp.
    pub(super) fn settle(&self, style: &mut Style) -> Result<(), String> {
        for (layer, property, options) in &self.declared {
            style
                .set_paint_property(layer, property, options.clone())
                .map_err(|error| error.to_string())?;
            let base = property.strip_suffix("-transition").unwrap_or(property);
            let spec = serde_json::from_value::<TransitionSpec>(options.clone()).ok();
            style
                .active_transitions
                .override_entry(layer, base, spec, None);
        }
        for (layer, property, started) in &self.started_at {
            style
                .active_transitions
                .override_entry(layer, property, None, Some(*started));
        }
        style.settle_transitions_at(self.clock);
        style.clear_transitions();
        Ok(())
    }
}
