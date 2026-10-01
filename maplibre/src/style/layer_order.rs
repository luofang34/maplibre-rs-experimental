//! Preserves the style document's painter order across asynchronous tile processing, and resolves
//! layers that take their definition from another layer with `ref`.
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value};

use super::layer::StyleLayer;

pub(super) fn deserialize_layers<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<StyleLayer>, D::Error> {
    let documents = dereferenced(Vec::<Value>::deserialize(deserializer)?);
    let mut layers = documents
        .into_iter()
        .map(|document| StyleLayer::deserialize(document).map_err(serde::de::Error::custom))
        .collect::<Result<Vec<_>, _>>()?;
    for (index, layer) in layers.iter_mut().enumerate() {
        layer.index = u32::try_from(index).map_err(serde::de::Error::custom)?;
    }
    Ok(layers)
}

/// The properties a layer with `ref` takes from the layer it refers to.
const REF_PROPERTIES: [&str; 7] = [
    "type",
    "source",
    "source-layer",
    "minzoom",
    "maxzoom",
    "filter",
    "layout",
];

/// `layers` with each `ref` replaced by the properties of the layer it names.
fn dereferenced(layers: Vec<Value>) -> Vec<Value> {
    let parents: Vec<(String, Map<String, Value>)> = layers
        .iter()
        .filter_map(|layer| {
            let layer = layer.as_object()?;
            Some((layer.get("id")?.as_str()?.to_owned(), layer.clone()))
        })
        .collect();
    layers
        .into_iter()
        .map(|layer| {
            let Some(object) = layer.as_object() else {
                return layer;
            };
            let Some(reference) = object.get("ref").and_then(Value::as_str) else {
                return layer;
            };
            let mut result = object.clone();
            result.remove("ref");
            if let Some((_, parent)) = parents.iter().find(|(id, _)| id == reference) {
                for key in REF_PROPERTIES {
                    if let Some(value) = parent.get(key) {
                        result.insert(key.to_owned(), value.clone());
                    }
                }
            }
            Value::Object(result)
        })
        .collect()
}

#[cfg(test)]
mod tests;
