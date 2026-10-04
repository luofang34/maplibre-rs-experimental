//! Road shields for maplibre-rs labels, made on request from route attributes by the
//! [roadshield](https://crates.io/crates/roadshield) engine and its Americana-compatible rules.
//!
//! The renderer knows nothing about routes. A style names a shield image with an expression,
//! such as [`openmaptiles_shield_image`], and [`RoadShieldProvider`] answers names in the
//! `roadshield` namespace from a resource pack the host supplies. A route without a shield
//! falls back to whatever the style's expression falls back to.
//!
//! ```no_run
//! # fn demo(map: &maplibre::headless::map::HeadlessMap) -> Result<(), Box<dyn std::error::Error>> {
//! use std::sync::Arc;
//!
//! use maplibre_roadshield::{load_pack_dir_blocking, RoadShieldProvider, ShieldDisplay, NAMESPACE};
//!
//! let pack = load_pack_dir_blocking("packs/americana".as_ref())?;
//! let provider = RoadShieldProvider::new(pack, ShieldDisplay::default())?;
//! if let Some(providers) = map.image_providers() {
//!     providers.register(NAMESPACE, Arc::new(provider));
//! }
//! # Ok(()) }
//! ```
//!
//! Then give a symbol layer `"icon-image": openmaptiles_shield_image()` (or an expression that
//! builds [`RouteRequest`] names from the data's full route relations) and leave its
//! `text-field` empty: the shield already carries the route number.

mod error;
mod pack;
mod provider;
mod raster;
mod route;
mod style;

pub use error::{PackLoadError, ShieldRenderError};
pub use pack::load_pack_dir_blocking;
pub use provider::{RoadShieldProvider, ShieldDisplay};
pub use route::{RouteRequest, NAMESPACE};
pub use style::{openmaptiles_route_shield_image, openmaptiles_shield_image, route_shield_image};
