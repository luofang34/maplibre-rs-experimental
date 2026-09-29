//! Typed failures from GPU initialization and frame acquisition.

#![deny(missing_docs)]

use thiserror::Error;

use crate::render::graph::RenderGraphError;

#[derive(Error, Debug)]
/// Renderer initialization, graph construction or surface acquisition failed.
pub enum RenderError {
    /// Acquiring a presentation image failed after the supported recovery attempt.
    #[error("error in surface")]
    Surface(#[from] super::resource::SurfaceAcquireError),
    /// The host could not provide a raw window or display handle.
    #[error("error while getting window handle")]
    Handle(#[from] wgpu::rwh::HandleError),
    /// A presentation surface could not be created from the host handles.
    #[error("error during surface creation")]
    CreateSurfaceError(#[from] wgpu::CreateSurfaceError),
    /// A graph node or connection did not satisfy the graph's contract.
    #[error("error in render graph")]
    Graph(#[from] RenderGraphError),
    /// The adapter could not create a device with the requested features and limits.
    #[error("error while requesting device")]
    RequestDevice(#[from] wgpu::RequestDeviceError),
    /// No adapter could be selected for the requested backend, surface and preferences.
    #[error("error while requesting adaptor")]
    RequestAdaptor(#[from] wgpu::RequestAdapterError),
}
