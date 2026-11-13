//! Shared configuration, camera, error, and style-parsing types for `maplibre-ffi`.

use cgmath::Deg;
use maplibre::{
    coords::LatLon,
    headless::map::{readback::ReadbackError, HeadlessMap, HeadlessMapOperationError},
    map::handle::{HeadlessRuntimeError, MapClosed},
    render::{
        settings::{BufferPoolSizes, RendererSettings},
        view_state::{CameraOptions, NavigationError},
    },
    sdf::query::QueryError,
    style::{
        feature_state::{FeatureId, FeatureStateTarget},
        mutation::{StyleChange, StyleMutationError},
        source::Source,
        Style,
    },
};
use serde_json::{Map, Value};
use thiserror::Error;

/// Errors returned by [`crate::MapEngine`] and mapped to thread-local messages across FFI/JNI.
#[derive(Debug, Error)]
pub enum FfiError {
    /// A required pointer parameter was null.
    #[error("null pointer passed for `{0}`")]
    NullPointer(&'static str),
    /// A C string parameter was not valid UTF-8.
    #[error("invalid UTF-8 in `{0}`")]
    InvalidUtf8(&'static str),
    /// Dimensions must be non-zero.
    #[error("invalid dimensions {width}x{height}")]
    InvalidSize {
        /// Requested width.
        width: u32,
        /// Requested height.
        height: u32,
    },
    /// A floating-point argument was NaN, infinite, or out of range.
    #[error("invalid numeric parameter `{0}`")]
    InvalidNumber(&'static str),
    /// Buffer length did not match required pixel byte count.
    #[error("buffer requires {expected} bytes, got {actual}")]
    InvalidBufferLength {
        /// Expected byte length.
        expected: usize,
        /// Provided byte length.
        actual: usize,
    },
    /// JSON parsing failed.
    #[error("invalid JSON in `{context}`: {source}")]
    Json {
        /// Argument or field context.
        context: &'static str,
        /// Underlying JSON error.
        #[source]
        source: serde_json::Error,
    },
    /// Expected a JSON object (`{ ... }`) for `feature-state`.
    #[error("`state_json` must be a JSON object")]
    ExpectedJsonObject,
    /// Returned JSON contained an interior NUL byte.
    #[error("serialized output contained an interior NUL byte")]
    InteriorNul,
    /// Map worker thread has already shut down.
    #[error(transparent)]
    Closed(#[from] MapClosed),
    /// Headless runtime startup failed.
    #[error(transparent)]
    Runtime(#[from] HeadlessRuntimeError),
    /// Camera update failed.
    #[error(transparent)]
    Navigation(#[from] NavigationError),
    /// Style mutation failed.
    #[error(transparent)]
    StyleMutation(#[from] StyleMutationError),
    /// GeoJSON source update failed.
    #[error(transparent)]
    GeoJsonUpdate(#[from] maplibre::geojson::update::SourceUpdateError),
    /// Frame execution failed.
    #[error("{0}")]
    Frame(String),
    /// GPU pixel readback failed.
    #[error(transparent)]
    Readback(#[from] ReadbackError),
    /// Rendered feature query failed.
    #[error(transparent)]
    Query(#[from] QueryError),
    /// Offscreen render target had no color texture.
    #[error("headless map has no color target texture")]
    MissingTargetTexture,
}

impl From<HeadlessMapOperationError> for FfiError {
    fn from(error: HeadlessMapOperationError) -> Self {
        Self::Frame(error.to_string())
    }
}

const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<FfiError>();
};

/// Safe Rust configuration for [`crate::MapEngine::create`].
#[derive(Clone, Debug)]
pub struct MapEngineConfig {
    /// Physical render target width in pixels.
    pub width: u32,
    /// Physical render target height in pixels.
    pub height: u32,
    /// Physical-to-logical pixel ratio (`<= 0.0` defaults to `1.0`).
    pub pixel_ratio: f64,
    /// Optional HTTP tile cache directory.
    pub cache_dir: Option<String>,
    /// Maximum camera pitch in degrees (`<= 0.0` uses default `60.0`).
    pub max_pitch_deg: f64,
}

/// Safe Rust camera representation matching [`crate::MaplibreCamera`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraState {
    /// Center latitude in degrees.
    pub latitude: f64,
    /// Center longitude in degrees.
    pub longitude: f64,
    /// Continuous MapLibre zoom level.
    pub zoom: f64,
    /// Bearing clockwise from north in degrees.
    pub bearing_deg: f64,
    /// Pitch away from nadir in degrees.
    pub pitch_deg: f64,
}

impl CameraState {
    /// Validates that every camera component is finite and converts to [`CameraOptions`].
    pub fn to_options(self) -> Result<CameraOptions, FfiError> {
        if !self.latitude.is_finite()
            || !self.longitude.is_finite()
            || !self.zoom.is_finite()
            || !self.bearing_deg.is_finite()
            || !self.pitch_deg.is_finite()
        {
            return Err(FfiError::InvalidNumber("camera"));
        }
        Ok(CameraOptions {
            center: Some(LatLon::new(self.latitude, self.longitude)),
            zoom: Some(self.zoom),
            bearing: Some(Deg(self.bearing_deg)),
            pitch: Some(Deg(self.pitch_deg)),
            ..CameraOptions::default()
        })
    }
}

pub(crate) fn compact_renderer_settings() -> RendererSettings {
    RendererSettings {
        texture_format: Some(wgpu::TextureFormat::Rgba8Unorm),
        buffer_pools: BufferPoolSizes {
            vertices: 2_000_000,
            indices: 6_000_000,
            feature_metadata: 2_000_000,
            layer_metadata: 10 * 1024,
        },
        symbol_pools: BufferPoolSizes {
            vertices: 500_000,
            indices: 1_000_000,
            feature_metadata: 500_000,
            layer_metadata: 10 * 1024,
        },
        ..RendererSettings::default()
    }
}

/// Parses a MapLibre Style JSON string and resolves global-state templates.
pub fn parse_style_json(style_json: &str) -> Result<Style, FfiError> {
    let mut style: Style = serde_json::from_str(style_json).map_err(|source| FfiError::Json {
        context: "style_json",
        source,
    })?;
    style.resolve_global_state();
    Ok(style)
}

/// Parses a `feature-state` target, canonical ID, and JSON object payload.
pub fn parse_feature_state_args(
    source: &str,
    source_layer: Option<&str>,
    feature_id: &str,
    state_json: &str,
) -> Result<(FeatureStateTarget, FeatureId, Map<String, Value>), FfiError> {
    let parsed: Value = serde_json::from_str(state_json).map_err(|err| FfiError::Json {
        context: "state_json",
        source: err,
    })?;
    let Value::Object(state_map) = parsed else {
        return Err(FfiError::ExpectedJsonObject);
    };
    let target = build_feature_state_target(source, source_layer);
    Ok((target, FeatureId::from(feature_id), state_map))
}

/// Builds a [`FeatureStateTarget`] treating empty or `None` `source_layer` as a GeoJSON target.
pub fn build_feature_state_target(source: &str, source_layer: Option<&str>) -> FeatureStateTarget {
    match source_layer.filter(|layer| !layer.is_empty()) {
        Some(layer) => FeatureStateTarget::vector(source, layer),
        None => FeatureStateTarget::geojson(source),
    }
}

/// Replaces `map`'s entire [`Style`] in-place, purging stale tile layers and triggering tile reload.
pub fn apply_whole_style(map: &mut HeadlessMap, next_style: Style) -> Result<(), FfiError> {
    map.mutate_style(|style| {
        let removed_layers: Vec<String> =
            style.layers.iter().map(|layer| layer.id.clone()).collect();
        let layers: Vec<String> = next_style
            .layers
            .iter()
            .map(|layer| layer.id.clone())
            .collect();
        let removed_raster_sources: Vec<String> = style
            .sources
            .iter()
            .filter(|(_, src)| {
                matches!(
                    src,
                    Source::Raster(_) | Source::RasterDem(_) | Source::Image(_)
                )
            })
            .map(|(name, _)| name.clone())
            .collect();
        let reloaded_image_sources: Vec<String> = next_style
            .sources
            .iter()
            .filter(|(_, src)| matches!(src, Source::Image(_)))
            .map(|(name, _)| name.clone())
            .collect();
        *style = next_style;
        Ok(StyleChange {
            layers,
            redraw_tiles: true,
            removed_layers,
            reloaded_image_sources,
            removed_raster_sources,
        })
    })?;
    Ok(())
}
