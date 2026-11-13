//! Source loading, tile geometry indexing, and asynchronous worker transport.

#![deny(missing_docs)]

pub use geozero::mvt::tile::Layer as RawLayer;

pub mod apc;
pub mod cpu_work;
pub(crate) mod frame_style;
pub mod geometry_index;
#[cfg(all(not(target_arch = "wasm32"), feature = "native-io"))]
pub mod mbtiles;
#[cfg(feature = "mlt")]
pub mod mlt;
pub mod offline;
pub mod pmtiles;
pub mod resource_loader;
pub mod scheduler;
pub mod source_client;
pub mod source_type;
#[cfg(feature = "embed-static-tiles")]
pub mod static_tile_fetcher;
pub mod tile_backpressure;
pub mod tile_json;
pub(crate) mod tile_load;
pub mod tile_retry;
pub mod tile_sources;
