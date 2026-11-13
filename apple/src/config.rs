//! Configurable iOS & macOS headed map startup options and validation.

use std::path::PathBuf;

use maplibre::{
    projection::{ProjectionSpecification, ProjectionType},
    render::{
        camera::DEFAULT_MAX_PITCH,
        settings::{Backends, WgpuSettings},
    },
    style::Style,
};
use maplibre_ffi::{parse_style_json, FfiError};
use maplibre_winit::{run_headed_map, HeadedMapError, HeadedMapOptions, WinitMapWindowConfig};
use thiserror::Error;

/// Errors returned by [`AppleMapConfig`] validation and [`run_apple_map`].
#[derive(Debug, Error)]
pub enum AppleSdkError {
    /// Style JSON or numeric configuration validation failed.
    #[error(transparent)]
    Ffi(#[from] FfiError),
    /// Native window or renderer startup failed.
    #[error(transparent)]
    Headed(#[from] HeadedMapError),
}

/// Configuration for launching a windowed iOS or macOS map via [`run_apple_map`].
#[derive(Clone, Debug)]
pub struct AppleMapConfig {
    /// Window title on macOS (ignored by full-screen iOS views).
    pub title: String,
    /// Optional MapLibre Style JSON document; `None` uses [`Style::default()`].
    pub style_json: Option<String>,
    /// Optional HTTP tile cache directory on device storage.
    pub cache_dir: Option<PathBuf>,
    /// Enabled `wgpu` graphics backends (defaults to Metal / `Backends::all()`).
    pub backends: Backends,
    /// Maximum camera pitch in degrees (`<= 0.0` falls back to `60.0`).
    pub max_pitch_deg: f64,
    /// Overrides the style projection to [`ProjectionType::Globe`] when `true`.
    pub globe: bool,
    /// Shows the floating navigation HUD controls.
    pub show_navigation_controls: bool,
    /// Draws red debug tile outlines when `true`.
    pub debug_tiles: bool,
    /// Optional frame limit for automated smoke runs.
    pub max_frames: Option<u64>,
}

impl Default for AppleMapConfig {
    fn default() -> Self {
        Self {
            title: "maplibre".to_owned(),
            style_json: None,
            cache_dir: None,
            backends: Backends::all(),
            max_pitch_deg: DEFAULT_MAX_PITCH.0,
            globe: false,
            show_navigation_controls: true,
            debug_tiles: false,
            max_frames: None,
        }
    }
}

impl AppleMapConfig {
    /// Sets an inline MapLibre Style JSON document.
    pub fn with_style_json(mut self, style_json: impl Into<String>) -> Self {
        self.style_json = Some(style_json.into());
        self
    }

    /// Sets the on-disk HTTP tile cache directory.
    pub fn with_cache_dir(mut self, cache_dir: impl Into<PathBuf>) -> Self {
        self.cache_dir = Some(cache_dir.into());
        self
    }

    /// Enables or disables globe projection override.
    pub fn with_globe(mut self, globe: bool) -> Self {
        self.globe = globe;
        self
    }

    /// Sets the maximum camera pitch in degrees.
    pub fn with_max_pitch_deg(mut self, max_pitch_deg: f64) -> Self {
        self.max_pitch_deg = max_pitch_deg;
        self
    }

    /// Overrides the `wgpu` graphics backends.
    pub fn with_backends(mut self, backends: Backends) -> Self {
        self.backends = backends;
        self
    }

    /// Parses [`Self::style_json`] (or constructs [`Style::default()`]) and applies [`Self::globe`].
    pub fn resolve_style(&self) -> Result<Style, AppleSdkError> {
        let mut style = match self.style_json.as_deref() {
            Some(raw) => parse_style_json(raw)?,
            None => {
                let mut default_style = Style::default();
                default_style.resolve_global_state();
                default_style
            }
        };
        if self.globe {
            style.projection = Some(ProjectionSpecification {
                projection_type: ProjectionType::Globe,
            });
        }
        Ok(style)
    }

    /// Validates numeric fields and builds [`HeadedMapOptions`].
    pub fn headed_options(&self) -> Result<HeadedMapOptions, AppleSdkError> {
        if !self.max_pitch_deg.is_finite() {
            return Err(AppleSdkError::Ffi(FfiError::InvalidNumber("max_pitch_deg")));
        }
        let max_pitch_degrees = if self.max_pitch_deg > 0.0 {
            self.max_pitch_deg
        } else {
            DEFAULT_MAX_PITCH.0
        };
        Ok(HeadedMapOptions {
            max_frames: self.max_frames,
            max_pitch_degrees,
            debug_tiles: self.debug_tiles,
            show_navigation_controls: self.show_navigation_controls,
            ..HeadedMapOptions::default()
        })
    }

    /// Builds [`WgpuSettings`] using [`Self::backends`].
    pub fn wgpu_settings(&self) -> WgpuSettings {
        WgpuSettings {
            backends: Some(self.backends),
            ..WgpuSettings::default()
        }
    }
}

/// Initializes `env_logger` idempotently so repeated SDK calls never panic.
pub fn init_apple_logging() {
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .try_init();
}

/// Validates `config` and runs a headed Apple (iOS / macOS) map window until exit.
pub fn run_apple_map(config: AppleMapConfig) -> Result<(), AppleSdkError> {
    let style = config.resolve_style()?;
    let options = config.headed_options()?;
    let wgpu_settings = config.wgpu_settings();
    run_headed_map(
        config.cache_dir,
        WinitMapWindowConfig::new(config.title),
        wgpu_settings,
        style,
        options,
    )?;
    Ok(())
}
