//! Source loading, tile geometry indexing, and asynchronous worker transport.

#![deny(missing_docs)]

pub use geozero::mvt::tile::Layer as RawLayer;

pub mod apc;
pub mod cpu_work;
mod formats;
#[cfg(all(not(target_arch = "wasm32"), feature = "native-io"))]
pub use formats::mbtiles;
#[cfg(feature = "embed-static-tiles")]
pub use formats::static_tile_fetcher;
pub(crate) use formats::vector_tile;
pub use formats::{offline, pmtiles, resource_loader, tile_json};

pub use crate::tcs::geometry_index;
pub mod scheduler;
pub mod source_client;
pub mod source_type;
mod tile_queue;
pub(crate) use tile_queue::{frame_signals, frame_style, tile_load};
pub use tile_queue::{tile_backpressure, tile_retry};
pub mod tile_sources;
