//! Paint transitions a fixture configures: the state of a changed property part-way through
//! its transition when the fixture stops waiting.

use std::collections::HashMap;

use maplibre::style::{expression::Color, Style};
use serde_json::Value;

/// Duration and delay, in milliseconds, of one property's transition.
#[derive(Clone, Copy, Default)]
struct Timing {
    duration: f64,
    delay: f64,
}

struct Pending {
    layer: String,
    property: String,
    from: Value,
    to: Value,
    timing: Timing,
    started: f64,
}

/// The transitions a fixture declares, the changes it makes under them, and the time it waits.
#[derive(Default)]
pub(super) struct Transitions {
    timings: HashMap<(String, String), Timing>,
    pending: Vec<Pending>,
    clock: f64,
}

fn timing_of(options: &Value) -> Timing {
    let read = |name: &str| options.get(name).and_then(Value::as_f64).unwrap_or(0.0);
    Timing {
        duration: read("duration"),
        delay: read("delay"),
    }
}

impl Transitions {
    /// Takes the `*-transition` members out of the layers' paint, which the style model does not
    /// carry, and remembers them.
    pub(super) fn extract(style: &mut Value) -> Self {
        let mut transitions = Self::default();
        let layers = style.get_mut("layers").and_then(Value::as_array_mut);
        for layer in layers.into_iter().flatten() {
            let id = layer.get("id").and_then(Value::as_str).unwrap_or_default();
            let id = id.to_owned();
            let Some(paint) = layer.get_mut("paint").and_then(Value::as_object_mut) else {
                continue;
            };
            let names: Vec<String> = paint
                .keys()
                .filter(|name| name.ends_with("-transition"))
                .cloned()
                .collect();
            for name in names {
                if let Some(options) = paint.remove(&name) {
                    transitions.declare(&id, &name, &options);
                }
            }
        }
        transitions
    }

    /// Declares the transition of `property` (written with its `-transition` suffix).
    pub(super) fn declare(&mut self, layer: &str, property: &str, options: &Value) {
        let base = property.strip_suffix("-transition").unwrap_or(property);
        self.timings
            .insert((layer.to_owned(), base.to_owned()), timing_of(options));
    }

    /// Notes that `property` is about to take `to`, starting its transition from what it is now.
    pub(super) fn before_change(&mut self, style: &Style, layer: &str, property: &str, to: &Value) {
        let Some(timing) = self
            .timings
            .get(&(layer.to_owned(), property.to_owned()))
            .copied()
        else {
            return;
        };
        let from = style
            .layers
            .iter()
            .find(|candidate| candidate.id == layer)
            .and_then(|candidate| serde_json::to_value(candidate).ok())
            .and_then(|document| document.pointer(&format!("/paint/{property}")).cloned());
        let Some(from) = from else {
            return;
        };
        self.pending
            .retain(|pending| !(pending.layer == layer && pending.property == property));
        self.pending.push(Pending {
            layer: layer.to_owned(),
            property: property.to_owned(),
            from,
            to: to.clone(),
            timing,
            started: self.clock,
        });
    }

    /// Milliseconds the fixture has waited so far.
    pub(super) fn now(&self) -> f64 {
        self.clock
    }

    /// Advances the clock by a `wait` of `milliseconds`.
    pub(super) fn wait(&mut self, milliseconds: f64) {
        self.clock += milliseconds;
    }

    /// Puts every property that is still in transition at its value for the current time.
    pub(super) fn settle(&self, style: &mut Style) -> Result<(), String> {
        for pending in &self.pending {
            let elapsed = self.clock - pending.started - pending.timing.delay;
            let progress = if pending.timing.duration > 0.0 {
                (elapsed / pending.timing.duration).clamp(0.0, 1.0)
            } else {
                1.0
            };
            if progress >= 1.0 {
                continue;
            }
            let Some(value) = interpolate(&pending.from, &pending.to, progress) else {
                continue;
            };
            style
                .set_paint_property(&pending.layer, &pending.property, value)
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }
}

/// The value `progress` of the way from `from` to `to`, for colours and numbers.
fn interpolate(from: &Value, to: &Value, progress: f64) -> Option<Value> {
    if let (Some(from), Some(to)) = (from.as_f64(), to.as_f64()) {
        return Some(Value::from(from + (to - from) * progress));
    }
    let (from, to) = (Color::parse(from.as_str()?)?, Color::parse(to.as_str()?)?);
    let (from, to) = (from.premultiplied(), to.premultiplied());
    let [red, green, blue, alpha] = std::array::from_fn(|i| from[i] + (to[i] - from[i]) * progress);
    let straight = |channel: f64| if alpha > 0.0 { channel / alpha } else { 0.0 };
    Some(Value::from(
        Color::new(straight(red), straight(green), straight(blue), alpha).css(),
    ))
}
