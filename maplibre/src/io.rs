//! Source loading, tile geometry indexing, and asynchronous worker transport.

#![deny(missing_docs)]

pub use geozero::mvt::tile::Layer as RawLayer;

pub mod apc;
pub mod cpu_work;
pub mod geometry_index;
#[cfg(feature = "mlt")]
pub mod mlt;
pub mod pmtiles;
pub mod resource_loader;
pub mod scheduler;
pub mod source_client;
pub mod source_type;
#[cfg(feature = "embed-static-tiles")]
pub mod static_tile_fetcher;
pub mod tile_backpressure;
pub mod tile_json;
pub mod tile_retry;
pub mod tile_sources;
