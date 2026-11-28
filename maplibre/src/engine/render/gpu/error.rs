//! Typed failures from GPU initialization and frame acquisition.

#![deny(missing_docs)]

use std::borrow::Cow;

use thiserror::Error;

use crate::projection::globe::{camera::GlobeCameraError, covering_tiles::GlobeCoveringError};

/// A pass could not be added, ordered or resolved.
#[derive(Error, Debug, Clone, Eq, PartialEq)]
pub enum RenderGraphError {
    /// No pass has this name.
    #[error("render pass {0:?} does not exist")]
    InvalidNode(Cow<'static, str>),
    /// A pass with this name already exists.
    #[error("render pass {0:?} already exists")]
    DuplicateNode(Cow<'static, str>),
    /// The order constraints form a cycle through these passes, so none of them can run first.
    #[error("render passes {0:?} require each other to run first")]
    Cycle(Vec<Cow<'static, str>>),
}

/// Failure while selecting visible Mercator tiles.
#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum MercatorCoveringError {
    /// The current camera cannot produce finite frustum coordinates.
    #[error("cannot unproject the Mercator covering frustum")]
    ViewProjection {
        /// Invalid projection supplied by the view.
        #[from]
        source: crate::projection::ViewProjectionError,
    },
    /// The coordinate model only supports canonical zoom levels through 31.
    #[error("mercator covering zoom {zoom} exceeds the supported maximum")]
    UnsupportedZoom {
        /// Unsupported zoom level.
        zoom: u8,
    },
}

/// Failure while deriving projection state from the current map view.
#[derive(Debug, Error)]
pub enum ProjectionStateError {
    /// Mercator tile selection failed.
    #[error("failed to select mercator tiles")]
    MercatorCovering {
        /// Underlying covering error.
        #[source]
        source: MercatorCoveringError,
    },
    /// Globe camera state could not be constructed.
    #[error("failed to construct globe camera state: {source}")]
    GlobeCamera {
        /// Underlying camera error.
        #[source]
        source: GlobeCameraError,
    },
    /// A globe matrix or clipping plane cannot be represented as 32-bit floats.
    #[error("globe projection state cannot be represented as f32")]
    FloatConversion,
    /// Globe tile traversal failed.
    #[error("failed to select visible globe tiles")]
    GlobeCovering {
        /// Underlying covering error.
        #[source]
        source: GlobeCoveringError,
    },
}

/// Why a window cannot supply a frame.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceAcquireError {
    /// The compositor did not supply an image before its deadline.
    #[error("surface acquisition timed out")]
    Timeout,
    /// The compositor cannot display the window.
    #[error("surface is occluded")]
    Occluded,
    /// The surface still needs configuration after a retry.
    #[error("surface configuration remains outdated")]
    Outdated,
    /// The host must recreate the surface.
    #[error("surface was lost")]
    Lost,
    /// The device reported a validation error during acquisition.
    #[error("surface acquisition failed validation")]
    Validation,
}

#[derive(Error, Debug)]
/// Renderer initialization, graph construction or surface acquisition failed.
pub enum RenderError {
    /// Acquiring a presentation image failed after the supported recovery attempt.
    #[error("error in surface")]
    Surface(#[from] SurfaceAcquireError),
    /// The host could not provide a raw window or display handle.
    #[error("error while getting window handle")]
    Handle(#[from] wgpu::rwh::HandleError),
    /// A presentation surface could not be created from the host handles.
    #[error("error during surface creation")]
    CreateSurfaceError(#[from] wgpu::CreateSurfaceError),
    /// The render passes could not be added, ordered or resolved.
    #[error("error in render graph")]
    Graph(#[from] RenderGraphError),
    /// The adapter could not create a device with the requested features and limits.
    #[error("error while requesting device")]
    RequestDevice(#[from] wgpu::RequestDeviceError),
    /// No adapter could be selected for the requested backend, surface and preferences.
    #[error("error while requesting adaptor")]
    RequestAdaptor(#[from] wgpu::RequestAdapterError),
}
