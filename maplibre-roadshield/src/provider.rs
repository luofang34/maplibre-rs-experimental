//! Answers `roadshield:` image names with shields drawn by the roadshield engine.

use std::sync::Arc;

use maplibre::sdf::assets::{
    ImageProviderError, ImageRequest, ImageResolution, ProvideFuture, StyleImageProvider,
};
use roadshield::{
    DisplayContext, Engine, PackError, Rendering, ResourcePack, ShieldError, ENGINE_OUTPUT_VERSION,
};

use crate::{raster::rasterize, RouteRequest, ShieldRenderError};

/// How shields are drawn, besides the display's pixel ratio.
#[derive(Clone, Debug, PartialEq)]
pub struct ShieldDisplay {
    /// Layout pixels per Americana pixel.
    pub scale: f64,
    /// The pack theme; `None` is the pack's default.
    pub theme: Option<String>,
    /// The BCP 47 language shield text is shaped for.
    pub language: Option<String>,
}

impl Default for ShieldDisplay {
    fn default() -> Self {
        Self {
            scale: 1.0,
            theme: None,
            language: None,
        }
    }
}

/// Draws the shields of `roadshield:` names from one resource pack.
#[derive(Clone, Debug)]
pub struct RoadShieldProvider {
    engine: Arc<Engine>,
    display: ShieldDisplay,
    generation: String,
}

impl RoadShieldProvider {
    /// Prepares the pack's fonts and blanks to draw shields as `display` says.
    pub fn new(pack: ResourcePack, display: ShieldDisplay) -> Result<Self, PackError> {
        let engine = Engine::new(pack)?;
        // Answers made from another pack, engine or display are not reused.
        let generation = format!(
            "{ENGINE_OUTPUT_VERSION}|{}|{}|{:?}|{:?}",
            engine.manifest().content_hash,
            display.scale,
            display.theme,
            display.language
        );
        Ok(Self {
            engine: Arc::new(engine),
            display,
            generation,
        })
    }

    /// The engine, for hosts that list the pack's licences or dependencies.
    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    /// How roadshield draws the route of image id `id` at `pixel_ratio`, before rasterising:
    /// its rule, provenance and the resources it depends on, as an offline download records.
    pub fn render_symbol(
        &self,
        id: &str,
        pixel_ratio: f32,
    ) -> Result<Rendering, ShieldRenderError> {
        let route = RouteRequest::parse(id)?;
        self.engine
            .render(&route.descriptor(), &self.context(pixel_ratio))
            .map_err(|source| ShieldRenderError::Shield {
                name: id.to_owned(),
                source,
            })
    }

    fn context(&self, pixel_ratio: f32) -> DisplayContext {
        DisplayContext {
            scale: self.display.scale,
            // MapLibre draws 2x sprites on dense displays; upstream rounds its geometry on
            // that grid.
            pixel_grid: if pixel_ratio >= 1.5 { 2 } else { 1 },
            theme: self.display.theme.clone(),
            language: self.display.language.clone(),
            ..DisplayContext::default()
        }
    }

    fn answer(&self, request: &ImageRequest) -> Result<ImageResolution, ImageProviderError> {
        let failed = |error: ShieldRenderError| ImageProviderError::Failed(error.to_string());
        match self.render_symbol(&request.id, request.pixel_ratio) {
            Ok(Rendering::Symbol(symbol)) => rasterize(&symbol, request.pixel_ratio)
                .map(ImageResolution::Image)
                .map_err(failed),
            Ok(Rendering::NoShield { reason, .. }) => {
                tracing::debug!(name = %request.name, ?reason, "the rules draw no shield");
                Ok(ImageResolution::Absent)
            }
            Err(ShieldRenderError::Shield {
                source: ShieldError::ResourceNotReady { detail },
                ..
            }) => Err(ImageProviderError::Unavailable(detail)),
            Err(error) => Err(failed(error)),
        }
    }
}

impl StyleImageProvider for RoadShieldProvider {
    fn generation(&self) -> String {
        self.generation.clone()
    }

    fn provide(&self, request: ImageRequest) -> ProvideFuture<'_> {
        Box::pin(async move { self.answer(&request) })
    }
}
