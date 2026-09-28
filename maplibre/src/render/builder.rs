use crate::{
    render::{
        error::RenderError,
        settings::{RendererSettings, WgpuSettings},
        Renderer,
    },
    window::{HeadedMapWindow, MapWindowConfig},
};

#[derive(Clone)]
pub struct RendererBuilder {
    wgpu_settings: Option<WgpuSettings>,
    renderer_settings: Option<RendererSettings>,
}

impl RendererBuilder {
    pub fn new() -> Self {
        Self {
            wgpu_settings: None,
            renderer_settings: None,
        }
    }

    pub fn with_renderer_settings(mut self, renderer_settings: RendererSettings) -> Self {
        self.renderer_settings = Some(renderer_settings);
        self
    }

    pub fn with_wgpu_settings(mut self, wgpu_settings: WgpuSettings) -> Self {
        self.wgpu_settings = Some(wgpu_settings);
        self
    }

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

pub struct UninitializedRenderer {
    pub wgpu_settings: WgpuSettings,
    pub renderer_settings: RendererSettings,
}

impl UninitializedRenderer {
    /// Initializes the whole rendering pipeline for the given configuration.
    /// Returns the initialized map, ready to be run.
    pub async fn initialize_renderer<MWC>(
        self,
        existing_window: &MWC::MapWindow,
    ) -> Result<Renderer, RenderError>
    where
        MWC: MapWindowConfig,
        <MWC as MapWindowConfig>::MapWindow: HeadedMapWindow,
    {
        Renderer::initialize(
            existing_window,
            self.wgpu_settings.clone(),
            self.renderer_settings,
        )
        .await
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
        Renderer::initialize_headless(
            existing_window,
            self.wgpu_settings.clone(),
            self.renderer_settings,
        )
        .await
    }
}
