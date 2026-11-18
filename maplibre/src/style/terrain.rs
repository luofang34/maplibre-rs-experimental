//! Root-level terrain specification.

use serde::{Deserialize, Serialize};

/// Drapes the map over a `raster-dem` source, following the style-spec `terrain` root property.
#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct TerrainSpecification {
    /// Name of the `raster-dem` source supplying elevation.
    pub source: String,
    /// Multiplier applied to elevation values; one keeps true heights.
    pub exaggeration: f32,
}

impl TerrainSpecification {
    /// Parses a `terrain` object directly from JSON.
    pub(crate) fn from_json_value(value: &serde_json::Value) -> Result<Self, serde_json::Error> {
        let Some(map) = value.as_object() else {
            return Err(serde::de::Error::custom("terrain must be a JSON object"));
        };
        let source = crate::util::json::extract::json_req_string(map, "source")?;
        let exaggeration =
            crate::util::json::extract::json_f64_or(map, "exaggeration", 1.0)? as f32;
        Ok(Self {
            source,
            exaggeration,
        })
    }
}

impl<'de> Deserialize<'de> for TerrainSpecification {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        Self::from_json_value(&value).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests;
