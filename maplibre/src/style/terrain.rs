//! Root-level terrain specification.

use serde::{Deserialize, Serialize};

/// Drapes the map over a `raster-dem` source, following the style-spec `terrain` root property.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct TerrainSpecification {
    /// Name of the `raster-dem` source supplying elevation.
    pub source: String,
    /// Multiplier applied to elevation values; one keeps true heights.
    #[serde(default = "default_exaggeration")]
    pub exaggeration: f32,
}

fn default_exaggeration() -> f32 {
    1.0
}

#[cfg(test)]
mod tests;
