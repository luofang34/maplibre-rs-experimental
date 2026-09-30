//! Preserves the style document's painter order across asynchronous tile processing.
use serde::{Deserialize, Deserializer};

use super::layer::StyleLayer;

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
