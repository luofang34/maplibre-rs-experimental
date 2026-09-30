//! Feature state on vector sources: the state a fixture sets is written into the tags of the
//! matching features of each decoded tile, where `feature-state` expressions read it.

use std::collections::BTreeMap;

use geozero::mvt::{tile, Message, Tile};
use maplibre::style::expression::FEATURE_STATE_PREFIX;
use serde_json::Value;

struct Entry {
    source: String,
    layer: Option<String>,
    id: u64,
    values: BTreeMap<String, Value>,
}

/// The state set on features of vector sources so far.
#[derive(Default)]
pub(super) struct VectorFeatureStates {
    entries: Vec<Entry>,
}

fn target_of(target: &Value) -> Result<(String, Option<String>, u64), String> {
    let source = target
        .get("source")
        .and_then(Value::as_str)
        .ok_or("feature state needs a source")?;
    let layer = target
        .get("sourceLayer")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let id = match target.get("id") {
        Some(Value::String(text)) => text.parse().ok(),
        Some(Value::Number(number)) => number.as_u64(),
        _ => None,
    }
    .ok_or("feature state on a vector source needs a numeric id")?;
    Ok((source.to_owned(), layer, id))
}

impl VectorFeatureStates {
    fn entry(&mut self, target: &Value) -> Result<&mut Entry, String> {
        let (source, layer, id) = target_of(target)?;
        let position = self
            .entries
            .iter()
            .position(|e| e.source == source && e.layer == layer && e.id == id);
        Ok(match position {
            Some(position) => &mut self.entries[position],
            None => {
                self.entries.push(Entry {
                    source,
                    layer,
                    id,
                    values: BTreeMap::new(),
                });
                let last = self.entries.len() - 1;
                &mut self.entries[last]
            }
        })
    }

    /// Merges `states` into the state of the feature `target` names.
    pub(super) fn set(&mut self, target: &Value, states: &Value) -> Result<(), String> {
        let Value::Object(values) = states else {
            return Err("setFeatureState needs an object of states".to_owned());
        };
        let entry = self.entry(target)?;
        entry.values.extend(
            values
                .iter()
                .map(|(key, value)| (key.clone(), value.clone())),
        );
        Ok(())
    }

    /// Removes the named state of the feature, or all of it for `null`.
    pub(super) fn remove(&mut self, target: &Value, keys: &Value) -> Result<(), String> {
        let entry = self.entry(target)?;
        match keys {
            Value::String(key) => {
                entry.values.remove(key);
            }
            _ => entry.values.clear(),
        }
        Ok(())
    }

    /// Whether any state was set on features of `source`.
    pub(super) fn has(&self, source: &str) -> bool {
        self.entries.iter().any(|entry| entry.source == source)
    }

    /// `tile` with the state of `source`'s features written into their tags.
    pub(super) fn apply(&self, source: &str, tile: &[u8]) -> Result<Vec<u8>, String> {
        let mut decoded =
            Tile::decode(tile).map_err(|error| format!("Cannot decode tile for state: {error}"))?;
        for entry in self.entries.iter().filter(|entry| entry.source == source) {
            for layer in &mut decoded.layers {
                if entry
                    .layer
                    .as_deref()
                    .is_some_and(|name| name != layer.name)
                {
                    continue;
                }
                write_state(layer, entry);
            }
        }
        Ok(decoded.encode_to_vec())
    }
}

fn write_state(layer: &mut tile::Layer, entry: &Entry) {
    let tags: Vec<(u32, u32)> = entry
        .values
        .iter()
        .filter_map(|(key, value)| {
            let value = tile_value(value)?;
            let name = format!("{FEATURE_STATE_PREFIX}{key}");
            let key_index = layer
                .keys
                .iter()
                .position(|k| *k == name)
                .unwrap_or_else(|| {
                    layer.keys.push(name);
                    layer.keys.len() - 1
                });
            layer.values.push(value);
            Some((key_index as u32, (layer.values.len() - 1) as u32))
        })
        .collect();
    for feature in layer
        .features
        .iter_mut()
        .filter(|feature| feature.id == Some(entry.id))
    {
        for (key, value) in &tags {
            feature.tags.extend([*key, *value]);
        }
    }
}

fn tile_value(value: &Value) -> Option<tile::Value> {
    let mut out = tile::Value::default();
    match value {
        Value::String(text) => out.string_value = Some(text.clone()),
        Value::Bool(flag) => out.bool_value = Some(*flag),
        Value::Number(number) => out.double_value = Some(number.as_f64()?),
        _ => return None,
    }
    Some(out)
}
