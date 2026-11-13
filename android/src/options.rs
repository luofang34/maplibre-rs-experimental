//! Configurable headed Android map options and runner.

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

/// Native Android activity handle on Android targets, or unit `()` during host builds/checks.
#[cfg(target_os = "android")]
pub type PlatformAndroidApp = maplibre_winit::android_activity::AndroidApp;

/// Host placeholder for [`PlatformAndroidApp`] when checking `maplibre-android` off-device.
#[cfg(not(target_os = "android"))]
pub type PlatformAndroidApp = ();

/// Errors produced by the Android embeddable SDK and JNI bindings.
#[derive(Debug, Error)]
pub enum AndroidSdkError {
    /// Underlying map engine or argument validation failed.
    #[error(transparent)]
    Ffi(#[from] FfiError),
    /// JNI environment call failed.
    #[error(transparent)]
    Jni(#[from] jni::errors::Error),
    /// Windowed map startup failed.
    #[error(transparent)]
    Headed(#[from] HeadedMapError),
    /// A required Java string or array reference was `null`.
    #[error("required JNI argument `{0}` was null")]
    NullArgument(&'static str),
    /// A `0` or unaligned native map handle was passed across JNI.
    #[error("invalid or destroyed NativeMap handle")]
    InvalidHandle,
}

/// Configuration for launching a windowed Android map via [`run_android_map`].
#[derive(Clone, Debug)]
pub struct AndroidMapOptions {
    /// Native window title.
    pub title: String,
    /// Optional MapLibre Style JSON document; `None` uses [`Style::default()`].
    pub style_json: Option<String>,
    /// Optional directory for caching fetched tiles on device storage.
    pub cache_dir: Option<PathBuf>,
    /// Graphics backends enabled for `wgpu` surface creation.
    pub backends: Backends,
    /// Maximum camera pitch in degrees (`<= 0.0` falls back to `60.0`).
    pub max_pitch_deg: f64,
    /// Overrides the style projection to [`ProjectionType::Globe`] when `true`.
    pub globe: bool,
    /// Displays the interactive floating navigation HUD controls.
    pub show_navigation_controls: bool,
    /// Renders red tile debug borders when `true`.
    pub debug_tiles: bool,
}

impl Default for AndroidMapOptions {
    fn default() -> Self {
        Self {
            title: "maplibre".to_owned(),
            style_json: None,
            cache_dir: None,
            backends: Backends::VULKAN | Backends::GL,
            max_pitch_deg: DEFAULT_MAX_PITCH.0,
            globe: false,
            show_navigation_controls: true,
            debug_tiles: false,
        }
    }
}

impl AndroidMapOptions {
    /// Sets an inline MapLibre Style JSON string.
    pub fn with_style_json(mut self, style_json: impl Into<String>) -> Self {
        self.style_json = Some(style_json.into());
        self
    }

    /// Sets the on-disk HTTP tile cache directory.
    pub fn with_cache_dir(mut self, cache_dir: impl Into<PathBuf>) -> Self {
        self.cache_dir = Some(cache_dir.into());
        self
    }

    /// Overrides the `wgpu` graphics backends.
    pub fn with_backends(mut self, backends: Backends) -> Self {
        self.backends = backends;
        self
    }

    /// Sets the maximum camera pitch in degrees.
    pub fn with_max_pitch_deg(mut self, max_pitch_deg: f64) -> Self {
        self.max_pitch_deg = max_pitch_deg;
        self
    }

    /// Enables or disables globe projection override.
    pub fn with_globe(mut self, globe: bool) -> Self {
        self.globe = globe;
        self
    }

    /// Parses [`Self::style_json`] (or constructs [`Style::default()`]) and applies [`Self::globe`].
    pub fn resolve_style(&self) -> Result<Style, AndroidSdkError> {
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

    /// Builds [`HeadedMapOptions`] from these Android options.
    pub fn headed_options(&self) -> Result<HeadedMapOptions, AndroidSdkError> {
        if !self.max_pitch_deg.is_finite() {
            return Err(AndroidSdkError::Ffi(FfiError::InvalidNumber(
                "max_pitch_deg",
            )));
        }
        let max_pitch_degrees = if self.max_pitch_deg > 0.0 {
            self.max_pitch_deg
        } else {
            DEFAULT_MAX_PITCH.0
        };
        Ok(HeadedMapOptions {
            max_pitch_degrees,
            debug_tiles: self.debug_tiles,
            show_navigation_controls: self.show_navigation_controls,
            ..HeadedMapOptions::default()
        })
    }

    /// Builds [`WgpuSettings`] using the configured [`Self::backends`].
    pub fn wgpu_settings(&self) -> WgpuSettings {
        WgpuSettings {
            backends: Some(self.backends),
            ..WgpuSettings::default()
        }
    }
}

/// Runs a configurable headed Android map bound to `app`.
pub fn run_android_map(
    app: PlatformAndroidApp,
    options: AndroidMapOptions,
) -> Result<(), AndroidSdkError> {
    let style = options.resolve_style()?;
    let headed = options.headed_options()?;
    let wgpu_settings = options.wgpu_settings();
    #[cfg(target_os = "android")]
    let window_config = WinitMapWindowConfig::new(options.title, app);
    #[cfg(not(target_os = "android"))]
    let window_config = {
        let () = app;
        WinitMapWindowConfig::new(options.title)
    };
    run_headed_map(
        options.cache_dir,
        window_config,
        wgpu_settings,
        style,
        headed,
    )?;
    Ok(())
}
