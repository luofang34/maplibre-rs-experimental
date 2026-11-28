//! Error types for map initialization and frame operations.

#![deny(missing_docs)]

use thiserror::Error;

#[cfg(feature = "geojson")]
use crate::geojson::{query::SourceQueryError, update::SourceUpdateError};
use crate::{
    plugin::PluginError,
    render::{error::RenderError, view_state::NavigationError},
    schedule::StageError,
    sdf::query::QueryError,
    style::mutation::StyleMutationError,
    window::WindowCreateError,
};

/// A window, renderer or scheduled update failed, or the map is in the wrong lifecycle state.
#[derive(Error, Debug)]
pub enum MapError {
    /// Renderer initialization was requested while the map is already ready.
    #[error("renderer was already set for this map")]
    RendererAlreadySet,
    /// The requested operation requires an initialized renderer.
    #[error("renderer is not fully initialized")]
    RendererNotReady,
    /// A plugin could not install its systems, render passes or GPU resources.
    #[error("installing a plugin failed")]
    Plugin(#[source] PluginError),
    /// Renderer initialization or presentation surface recovery failed.
    #[error("initializing device failed")]
    DeviceInit(#[source] RenderError),
    /// The host could not create the map window or its event loop.
    #[error("creating window failed")]
    Window(#[from] WindowCreateError),
    /// A scheduled update failed; preceding state changes are not rolled back.
    #[error("executing map stage failed")]
    StageError(#[from] StageError),
    /// A source's data could not be replaced or edited; the source is unchanged.
    #[cfg(feature = "geojson")]
    #[error("updating source data failed")]
    SourceUpdate(#[from] SourceUpdateError),
    /// A source's features could not be read.
    #[cfg(feature = "geojson")]
    #[error("querying source features failed")]
    SourceQuery(#[from] SourceQueryError),
    /// A rendered-feature query was rejected.
    #[error("querying rendered features failed")]
    Query(#[from] QueryError),
    /// A style change was refused; the style is unchanged.
    #[error("changing the style failed")]
    StyleChange(#[from] StyleMutationError),
    /// A camera move was refused; the camera is unchanged.
    #[error("moving the camera failed")]
    Navigation(#[from] NavigationError),
}
