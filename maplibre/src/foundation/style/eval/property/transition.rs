//! Paint transition timing specifications and active transition storage.

#[cfg(any(not(target_arch = "wasm32"), feature = "webgpu", feature = "web-webgl"))]
use rustc_hash::FxHashMap;
use serde::{de, Deserialize, Deserializer, Serialize};
use serde_json::Value;

const DEFAULT_TRANSITION_DURATION_MS: f64 = 300.0;

/// Duration and delay, in milliseconds, of a paint property transition.
#[derive(Serialize, Debug, Clone, Copy, PartialEq)]
pub struct TransitionSpec {
    /// Duration of the interpolation in milliseconds (`300.0` when omitted in JSON).
    pub duration: f64,
    /// Delay before interpolation starts in milliseconds (`0.0` when omitted in JSON).
    pub delay: f64,
}

impl TransitionSpec {
    /// Parses a transition object directly from a JSON [`Value`].
    pub(crate) fn from_json_value(value: &Value) -> Result<Self, serde_json::Error> {
        let Some(map) = value.as_object() else {
            return Err(de::Error::custom("expected transition object"));
        };
        let duration = crate::util::json::extract::json_f64_or(
            map,
            "duration",
            DEFAULT_TRANSITION_DURATION_MS,
        )?;
        let delay = crate::util::json::extract::json_f64_or(map, "delay", 0.0)?;
        Ok(Self { duration, delay })
    }
}

impl<'de> Deserialize<'de> for TransitionSpec {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        Self::from_json_value(&value).map_err(de::Error::custom)
    }
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

/// Runtime clock and in-flight paint transitions carried by a [`crate::style::Style`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ActiveTransitions {
    pub(crate) clock_ms: f64,
    pub(crate) entries: Vec<ActiveTransition>,
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
#[cfg(any(not(target_arch = "wasm32"), feature = "webgpu", feature = "web-webgl"))]
pub(crate) fn extract_layer_transitions_ref(
    layer_id: &str,
    paint: Option<&serde_json::Map<String, Value>>,
) -> Result<FxHashMap<String, TransitionSpec>, String> {
    let mut extracted = FxHashMap::default();
    let Some(map) = paint else {
        return Ok(extracted);
    };
    for (key, raw) in map {
        let Some(base) = key.strip_suffix("-transition") else {
            continue;
        };
        if raw.is_null() {
            continue;
        }
        let spec = TransitionSpec::from_json_value(raw)
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
