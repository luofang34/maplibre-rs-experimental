//! Native map startup with recoverable host configuration errors.

#![deny(missing_docs, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;

use maplibre::{
    environment::OffscreenKernelConfig,
    io::apc::SchedulerAsyncProcedureCall,
    kernel::{Kernel, KernelBuildError, KernelBuilder},
    map::{Map, MapError},
    platform::{
        http_client::ReqwestHttpClient, run_multithreaded, scheduler::TokioScheduler,
        ReqwestOffscreenKernelEnvironment,
    },
    plugin::Plugin,
    render::{builder::RendererBuilder, settings::WgpuSettings, RenderPlugin},
    style::{source::Source, Style},
};
use thiserror::Error;

use super::{HeadedMapOptions, WinitMapWindowConfig};
use crate::{WinitApplicationError, WinitEnvironment, WinitEventLoop, WinitHostError};

type NativeEnvironment = WinitEnvironment<
    TokioScheduler,
    ReqwestHttpClient,
    ReqwestOffscreenKernelEnvironment,
    SchedulerAsyncProcedureCall<ReqwestOffscreenKernelEnvironment, TokioScheduler>,
    (),
>;

/// A native map could not configure its host services, initialize, or run its event loop.
#[derive(Debug, Error)]
pub enum HeadedMapError {
    /// Worker configuration cannot represent the supplied filesystem path.
    #[error("cache path cannot be encoded as UTF-8: {path:?}")]
    InvalidCachePath {
        /// Path rejected before map or window creation.
        path: PathBuf,
    },
    /// A required kernel service was not configured.
    #[error("map kernel configuration failed")]
    Kernel(#[from] KernelBuildError),
    /// The map window or renderer could not be initialized.
    #[error("map initialization failed")]
    Map(#[from] MapError),
    /// The operating system rejected a window or event loop.
    #[error(transparent)]
    Host(#[from] WinitHostError),
    /// The window closed before renderer initialization completed.
    #[error("event loop closed before map initialization completed")]
    ClosedBeforeReady,
}

/// Opens a native window and runs its map until the event loop exits.
/// Errors retain the failed initialization step and underlying cause.
/// This blocks the calling thread and must run outside an existing Tokio runtime.
pub fn run_headed_map<P: Into<PathBuf>>(
    cache_path: Option<P>,
    window_config: WinitMapWindowConfig<()>,
    wgpu_settings: WgpuSettings,
    style: Style,
    options: HeadedMapOptions,
) -> Result<(), HeadedMapError> {
    let cache_path = cache_path.map(Into::into);
    let cache_directory = cache_directory(cache_path.as_ref())?;
    let event_loop = WinitEventLoop::new(&window_config)?;
    run_multithreaded(async {
        let result = event_loop.run_map_blocking(
            window_config,
            move |bound_config| {
                let kernel =
                    create_kernel(cache_path.clone(), cache_directory.clone(), bound_config)?;
                let renderer_builder =
                    RendererBuilder::new().with_wgpu_settings(wgpu_settings.clone());
                let plugins = map_plugins(&style, options.debug_tiles);
                let mut map = Map::new(style.clone(), kernel, renderer_builder, plugins)?;
                map.set_max_pitch(cgmath::Deg(options.max_pitch_degrees));
                Ok::<_, HeadedMapError>(map)
            },
            options.max_frames,
        );
        result.map_err(|error| match error {
            WinitApplicationError::CreateMap(error) => error,
            WinitApplicationError::Host(error) => HeadedMapError::Host(error),
            WinitApplicationError::Initialize(error) | WinitApplicationError::Frame(error) => {
                HeadedMapError::Map(error)
            }
            WinitApplicationError::ClosedBeforeReady => HeadedMapError::ClosedBeforeReady,
        })
    })
}

fn cache_directory(path: Option<&PathBuf>) -> Result<Option<String>, HeadedMapError> {
    path.map(|path| {
        path.to_str()
            .map(str::to_owned)
            .ok_or_else(|| HeadedMapError::InvalidCachePath { path: path.clone() })
    })
    .transpose()
}

fn create_kernel(
    cache_path: Option<PathBuf>,
    cache_directory: Option<String>,
    window_config: WinitMapWindowConfig<()>,
) -> Result<Kernel<NativeEnvironment>, KernelBuildError> {
    KernelBuilder::new()
        .with_map_window_config(window_config)
        .with_http_client(ReqwestHttpClient::new(cache_path))
        .with_apc(SchedulerAsyncProcedureCall::new(
            TokioScheduler::new(),
            OffscreenKernelConfig { cache_directory },
        ))
        .with_scheduler(TokioScheduler::new())
        .build()
}

fn map_plugins(style: &Style, debug_tiles: bool) -> Vec<Box<dyn Plugin<NativeEnvironment>>> {
    // Only declared sources participate in the requirement that every tile plugin be ready.
    let has_vector_sources = style.sources.is_empty()
        || style
            .sources
            .values()
            .any(|source| matches!(source, Source::Vector(_) | Source::GeoJson(_)));
    let has_raster_sources = style
        .sources
        .values()
        .any(|source| matches!(source, Source::Raster(_) | Source::RasterDem(_)));
    let mut plugins: Vec<Box<dyn Plugin<NativeEnvironment>>> = vec![
        Box::new(RenderPlugin),
        Box::new(maplibre::background::BackgroundPlugin),
    ];
    if has_vector_sources {
        plugins.push(Box::<
            maplibre::vector::VectorPlugin<maplibre::vector::DefaultVectorTransferables>,
        >::default());
        plugins.push(Box::<
            maplibre::sdf::SdfPlugin<maplibre::vector::DefaultVectorTransferables>,
        >::default());
    }
    if has_raster_sources {
        plugins.push(Box::<
            maplibre::raster::RasterPlugin<maplibre::raster::DefaultRasterTransferables>,
        >::default());
        plugins.push(Box::new(maplibre::hillshade::HillshadePlugin));
    }
    if debug_tiles {
        plugins.push(Box::new(maplibre::debug::DebugPlugin));
    }
    if style.terrain.is_some() {
        plugins.push(Box::<
            maplibre::terrain::TerrainPlugin<maplibre::terrain::DefaultDemTransferables>,
        >::default());
    }
    plugins
}

#[cfg(test)]
mod tests;
