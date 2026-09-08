//! Preserves the style document's painter order across asynchronous tile processing.
use super::layer::StyleLayer;
use serde::{Deserialize, Deserializer};

pub(super) fn deserialize_layers<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<StyleLayer>, D::Error> {
    let mut layers = Vec::<StyleLayer>::deserialize(deserializer)?;
    for (index, layer) in layers.iter_mut().enumerate() {
        layer.index = u32::try_from(index).map_err(serde::de::Error::custom)?;
    }
    Ok(layers)
}

#[cfg(test)]
mod tests;
