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
#![forbid(unsafe_code)]
#![deny(dead_code, unused_imports)]
// The uncounted wgpu upload methods listed in clippy.toml are errors, not warnings.
#![deny(clippy::disallowed_methods)]
#![cfg_attr(
    not(test),
    forbid(
        clippy::unwrap_used,
        clippy::panic,
        clippy::todo,
        clippy::unimplemented
    )
)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]

extern crate core;

/// Vector tile protocol-buffer types shared by native and worker transports.
pub use geozero::mvt::tile;

/// Geometry types compatible with the tessellator's coordinate units.
pub mod euclid {
    pub use lyon::geom::euclid::*;
}

mod engine;
mod features;
mod foundation;
#[cfg(feature = "headless")]
pub mod headless;
mod layers;
pub mod map;
mod world;

pub use engine::{context, custom_layer, environment, io, kernel, platform, render, schedule};
#[cfg(feature = "geojson")]
pub use features::geojson;
#[cfg(feature = "experimental")]
pub use features::starfield;
pub use features::{background, benchmarking, heatmap, input};
pub use foundation::{coords, event_loop, font, projection, style, tcs, util, window};
#[cfg(feature = "raster")]
pub use layers::raster;
pub use layers::{hillshade, navigation, query, sdf, vector};
pub use map::{BinnedRenderPhasePlugin, RenderPlugin};
#[cfg(feature = "experimental")]
pub use world::{celestial, overlay};
pub use world::{debug, event, hud, plugin, terrain};
