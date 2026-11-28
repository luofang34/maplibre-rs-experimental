//! Tile container formats, static tile bundles, resource loaders, and MVT/MLT codecs.

#[cfg(all(not(target_arch = "wasm32"), feature = "native-io"))]
pub mod mbtiles;
pub mod offline;
pub mod pmtiles;
pub mod resource_loader;
#[cfg(feature = "embed-static-tiles")]
pub mod static_tile_fetcher;
pub mod tile_json;
pub(crate) mod vector_tile;
