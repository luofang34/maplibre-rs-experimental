//! Scene features and host interactions: background, heatmap, GeoJSON, input, starfield, and benchmarks.

pub mod background;
pub mod benchmarking;
#[cfg(feature = "geojson")]
pub mod geojson;
pub mod heatmap;
pub mod input;
#[cfg(feature = "experimental")]
pub mod starfield;
