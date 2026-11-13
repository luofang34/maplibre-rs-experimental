//! Runtime `feature-state`: per-feature key/value state keyed by source, optional `source-layer`,
//! and canonical feature identifier (either `promoteId` or the feature's own ID).

mod sync;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
pub(crate) use sync::{
    extract_feature_state, extract_source_properties, layer_geometry_reads_feature_state,
    layer_paint_reads_feature_state, resolve_geojson_state_id, sync_arc_properties_feature_state,
    sync_properties_feature_state, FeatureStateArcMemo,
};

use crate::style::{
    expression::{js_number, Value as ExprValue},
    source::{PromoteId, Source, GEOJSON_LAYER},
    Style,
};

/// Internal property key carrying a feature's canonical stringified state ID across tile encoding.
///
/// Prefixed with `\u{1}` so [`crate::style::expression::is_internal_property`] hides it from
/// `["properties"]` expressions and `query_rendered_features` results.
pub const STATE_ID_PROPERTY: &str = "\u{1}state_id";

/// Canonical feature identifier used by `set_feature_state` and `promoteId` lookups.
///
/// Numeric and string representations of the same JavaScript number (such as `42` and `"42"`, or
/// `1.2` and `"1.2"`) map to the same canonical key, while arbitrary strings and 64-bit integers
/// above `2^53` preserve their exact characters.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FeatureId(String);

impl FeatureId {
    /// Parses a JSON feature ID or `promoteId` property value (`string` or finite `number`).
    pub fn from_json(value: &Value) -> Option<Self> {
        match value {
            Value::String(text) => Some(Self(text.clone())),
            Value::Number(number) => number
                .as_u64()
                .map(|n| Self(n.to_string()))
                .or_else(|| number.as_i64().map(|n| Self(n.to_string())))
                .or_else(|| {
                    number
                        .as_f64()
                        .filter(|n| n.is_finite())
                        .map(|n| Self(js_number(n)))
                }),
            _ => None,
        }
    }

    /// Parses an evaluated expression property value (`string` or finite `number`).
    pub fn from_expr_value(value: &ExprValue) -> Option<Self> {
        match value {
            ExprValue::String(text) => Some(Self(text.clone())),
            ExprValue::Number(number) if number.is_finite() => Some(Self(js_number(*number))),
            _ => None,
        }
    }

    /// Canonical string slice of this feature identifier.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consumes the identifier into its canonical `String`.
    pub fn into_string(self) -> String {
        self.0
    }
}

impl From<u64> for FeatureId {
    fn from(value: u64) -> Self {
        Self(value.to_string())
    }
}

impl From<i64> for FeatureId {
    fn from(value: i64) -> Self {
        Self(value.to_string())
    }
}

impl From<u32> for FeatureId {
    fn from(value: u32) -> Self {
        Self(value.to_string())
    }
}

impl From<i32> for FeatureId {
    fn from(value: i32) -> Self {
        Self(value.to_string())
    }
}

impl From<usize> for FeatureId {
    fn from(value: usize) -> Self {
        Self(value.to_string())
    }
}

impl From<&str> for FeatureId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl From<String> for FeatureId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

/// Names the source (and optional vector `source-layer`) addressed by a feature-state operation.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct FeatureStateTarget {
    /// Style source identifier.
    pub source: String,
    /// Vector source layer identifier; `None` for GeoJSON sources.
    #[serde(
        rename = "sourceLayer",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub source_layer: Option<String>,
}

impl FeatureStateTarget {
    /// Targets a GeoJSON source (which has no `source-layer`).
    pub fn geojson(source: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            source_layer: None,
        }
    }

    /// Targets a specific `source-layer` inside a vector tile source.
    pub fn vector(source: impl Into<String>, source_layer: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            source_layer: Some(source_layer.into()),
        }
    }
}

/// Feature-state entries belonging to a single style source, grouped by `source-layer` (`""` for
/// source-wide / GeoJSON state).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SourceFeatureState {
    layers: BTreeMap<String, BTreeMap<String, BTreeMap<String, Value>>>,
}

impl SourceFeatureState {
    /// Whether this source holds no feature-state keys.
    pub fn is_empty(&self) -> bool {
        self.layers
            .values()
            .all(|features| features.values().all(BTreeMap::is_empty))
    }

    fn layer_key(source_layer: Option<&str>) -> &str {
        match source_layer {
            None | Some("") | Some(GEOJSON_LAYER) => "",
            Some(name) => name,
        }
    }

    /// Returns the state map for `id` in `source_layer` (falling back to source-wide `""` entries
    /// so GeoJSON layers named by style layer ID still match `source_layer: None`).
    pub fn get(&self, source_layer: Option<&str>, id: &str) -> Option<&BTreeMap<String, Value>> {
        let key = Self::layer_key(source_layer);
        self.layers
            .get(key)
            .and_then(|features| features.get(id))
            .or_else(|| {
                (!key.is_empty())
                    .then(|| self.layers.get("")?.get(id))
                    .flatten()
            })
    }

    fn set(&mut self, source_layer: Option<&str>, id: String, state: Map<String, Value>) {
        let key = Self::layer_key(source_layer).to_owned();
        let entry = self.layers.entry(key).or_default().entry(id).or_default();
        for (k, v) in state {
            entry.insert(k, v);
        }
    }

    fn remove(&mut self, source_layer: Option<&str>, id: Option<&str>, prop_key: Option<&str>) {
        let layer_key = Self::layer_key(source_layer);
        match (source_layer, id, prop_key) {
            (None, None, None) => self.layers.clear(),
            (None, None, Some(prop)) => {
                for features in self.layers.values_mut() {
                    for values in features.values_mut() {
                        values.remove(prop);
                    }
                    features.retain(|_, v| !v.is_empty());
                }
                self.layers.retain(|_, f| !f.is_empty());
            }
            (_, None, None) => {
                self.layers.remove(layer_key);
            }
            (_, None, Some(prop)) => {
                if let Some(features) = self.layers.get_mut(layer_key) {
                    for values in features.values_mut() {
                        values.remove(prop);
                    }
                    features.retain(|_, v| !v.is_empty());
                    if features.is_empty() {
                        self.layers.remove(layer_key);
                    }
                }
            }
            (_, Some(feature_id), None) => {
                if let Some(features) = self.layers.get_mut(layer_key) {
                    features.remove(feature_id);
                    if features.is_empty() {
                        self.layers.remove(layer_key);
                    }
                }
            }
            (_, Some(feature_id), Some(prop)) => {
                if let Some(features) = self.layers.get_mut(layer_key) {
                    if let Some(values) = features.get_mut(feature_id) {
                        values.remove(prop);
                        if values.is_empty() {
                            features.remove(feature_id);
                        }
                    }
                    if features.is_empty() {
                        self.layers.remove(layer_key);
                    }
                }
            }
        }
    }
}

/// Runtime feature-state store across all sources of a [`Style`].
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FeatureStateStore {
    sources: BTreeMap<String, SourceFeatureState>,
}

impl FeatureStateStore {
    /// Whether no feature state is stored for any source.
    pub fn is_empty(&self) -> bool {
        self.sources.values().all(SourceFeatureState::is_empty)
    }

    /// Borrows the feature state stored for `source`.
    pub fn source(&self, source: &str) -> Option<&SourceFeatureState> {
        self.sources.get(source).filter(|s| !s.is_empty())
    }

    /// Merges `state` into the state of `(target, id)`.
    pub fn set(&mut self, target: FeatureStateTarget, id: FeatureId, state: Map<String, Value>) {
        self.sources.entry(target.source).or_default().set(
            target.source_layer.as_deref(),
            id.0,
            state,
        );
    }

    /// Reads the state map of `(target, id)`, if any.
    pub fn get(
        &self,
        target: &FeatureStateTarget,
        id: &FeatureId,
    ) -> Option<&BTreeMap<String, Value>> {
        self.sources
            .get(&target.source)?
            .get(target.source_layer.as_deref(), id.as_str())
    }

    /// Removes a single `key`, an entire feature (`key == None`), or all features of `target`
    /// (`id == None`).
    pub fn remove(
        &mut self,
        target: &FeatureStateTarget,
        id: Option<&FeatureId>,
        key: Option<&str>,
    ) {
        let Some(source) = self.sources.get_mut(&target.source) else {
            return;
        };
        source.remove(
            target.source_layer.as_deref(),
            id.map(FeatureId::as_str),
            key,
        );
        if source.is_empty() {
            self.sources.remove(&target.source);
        }
    }
}

impl Style {
    /// Merges `state` into the runtime state of feature `id` in `target`.
    pub fn set_feature_state(
        &mut self,
        target: FeatureStateTarget,
        id: impl Into<FeatureId>,
        state: Map<String, Value>,
    ) {
        self.feature_state.set(target, id.into(), state);
    }

    /// Returns the runtime state map of feature `id` in `target`, if any.
    pub fn feature_state(
        &self,
        target: &FeatureStateTarget,
        id: impl Into<FeatureId>,
    ) -> Option<&BTreeMap<String, Value>> {
        let id = id.into();
        self.feature_state.get(target, &id)
    }

    /// Removes `key` (or all keys when `None`) from feature `id` (or all features of `target` when
    /// `id` is `None`).
    pub fn remove_feature_state(
        &mut self,
        target: &FeatureStateTarget,
        id: Option<FeatureId>,
        key: Option<&str>,
    ) {
        self.feature_state.remove(target, id.as_ref(), key);
    }

    /// Returns the `promoteId` configuration declared on `source_name`, if any.
    pub fn source_promote_id(&self, source_name: &str) -> Option<&PromoteId> {
        match self.sources.get(source_name)? {
            Source::GeoJson(geojson) => geojson.promote_id.as_ref(),
            Source::Vector(vector) => vector.promote_id.as_ref(),
            _ => None,
        }
    }
}
