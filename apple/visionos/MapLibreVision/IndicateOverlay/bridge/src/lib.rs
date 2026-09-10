//! Pointer-free Apple ABI for an independent Indicate synthetic-vision overlay.

mod export;
mod telemetry;

pub use export::{OverlayScene, indicate_svs_directions, indicate_svs_glyph, indicate_svs_render};
pub use telemetry::{ReplayTelemetry, resolve_replay};
