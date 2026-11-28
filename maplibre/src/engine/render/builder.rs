//! Deferred renderer initialization for a host window and explicit device/render settings.

#![deny(missing_docs, clippy::redundant_clone)]

use crate::{
    render::{
        error::RenderError,
        settings::{RendererSettings, WgpuSettings},
        Renderer,
    },
    window::{HeadedMapWindow, MapWindowConfig},
};

#[derive(Clone)]
/// Collects optional configuration without creating a window, GPU device or surface.
pub struct RendererBuilder {
    wgpu_settings: Option<WgpuSettings>,
    renderer_settings: Option<RendererSettings>,
}

impl RendererBuilder {
    /// Starts with renderer and device settings that will use their defaults unless replaced.
    pub fn new() -> Self {
        Self {
            wgpu_settings: None,
            renderer_settings: None,
        }
    }

    /// Replaces render target and pool settings; later calls replace the entire value.
    pub fn with_renderer_settings(mut self, renderer_settings: RendererSettings) -> Self {
        self.renderer_settings = Some(renderer_settings);
        self
    }

    /// Replaces the device request configuration; later calls replace the entire value.
    pub fn with_wgpu_settings(mut self, wgpu_settings: WgpuSettings) -> Self {
        self.wgpu_settings = Some(wgpu_settings);
        self
    }

    /// Resolves missing settings to defaults without acquiring any GPU resources.
    pub fn build(self) -> UninitializedRenderer {
        UninitializedRenderer {
            wgpu_settings: self.wgpu_settings.unwrap_or_default(),
            renderer_settings: self.renderer_settings.unwrap_or_default(),
        }
    }
}

impl Default for RendererBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Resolved configuration that can be consumed to acquire GPU resources for an existing window.
pub struct UninitializedRenderer {
    /// Backend selection, adapter preference, features and limits for the device request.
    pub wgpu_settings: WgpuSettings,
    /// Render target properties and tile buffer capacities.
    pub renderer_settings: RendererSettings,
}

impl UninitializedRenderer {
    /// Acquires a window-compatible adapter/device and configures its presentation surface.
    /// Returns a renderer with an empty render graph; plugins supply the rendering pipeline.
    /// Window-handle, surface and device-request failures are propagated as [`RenderError`].
    pub async fn initialize_renderer<MWC>(
        self,
        existing_window: &MWC::MapWindow,
    ) -> Result<Renderer, RenderError>
    where
        MWC: MapWindowConfig,
        <MWC as MapWindowConfig>::MapWindow: HeadedMapWindow,
    {
        Renderer::initialize(existing_window, self.wgpu_settings, self.renderer_settings).await
    }
}

#[cfg(feature = "headless")]
impl UninitializedRenderer {
    pub(crate) async fn initialize_headless<MWC>(
        self,
        existing_window: &MWC::MapWindow,
    ) -> Result<Renderer, RenderError>
    where
        MWC: MapWindowConfig,
    {
        Renderer::initialize_headless(existing_window, self.wgpu_settings, self.renderer_settings)
            .await
    }
}
