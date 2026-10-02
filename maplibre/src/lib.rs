//! # Maplibre-rs
//!
//! A multi-platform library for rendering vector tile maps with WebGPU.
//!
//! Maplibre-rs is a map renderer that can run natively on MacOS, Linux, Windows, Android, iOS and the web.
//! It takes advantage of Lyon to tessellate vector tiles and WebGPU to display them efficiently.
//! The `headless` feature renders supplied tiles into offscreen textures and raster images.
//!
//! The official guide book can be found [here](https://maplibre.org/maplibre-rs/docs/book/).
//!
#![deny(dead_code, unused_imports)]
// The uncounted wgpu upload methods listed in clippy.toml are errors, not warnings.
#![deny(clippy::disallowed_methods)]

extern crate core;

/// Vector tile protocol-buffer types shared by native and worker transports.
pub use geozero::mvt::tile;

/// Geometry types compatible with the tessellator's coordinate units.
pub mod euclid {
    pub use lyon::geom::euclid::*;
}

pub mod context;
pub mod coords;
#[cfg(feature = "headless")]
pub mod headless;
pub mod io;
pub mod platform;
pub mod projection;
pub mod query;
pub mod render;
pub mod style;
pub mod util;

pub mod schedule;
pub mod window;

pub mod environment;

pub mod benchmarking;

pub mod background;
pub mod event_loop;
pub mod kernel;
pub mod map;
pub mod plugin;
pub mod tcs;

pub mod debug;
pub mod geojson;
pub mod heatmap;
pub mod hillshade;
pub mod raster;
pub mod vector;

pub mod sdf;
pub mod terrain;
