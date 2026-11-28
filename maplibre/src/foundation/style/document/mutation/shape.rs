//! What a layer added to a style must satisfy: a source of a kind its type draws from, as
//! GL JS checks, and an ordered zoom range.

use super::StyleMutationError;
use crate::style::{layer::StyleLayer, source::Source, Style};

impl Style {
    /// Checks what GL JS checks of a layer: that its source exists, is of a kind its type can draw
    /// from and, for vector sources, is read through a source layer, and that the zoom range is
    /// ordered.
    pub(super) fn ensure_shape(&self, layer: &StyleLayer) -> Result<(), StyleMutationError> {
        let id = layer.id.clone();
        ensure_zoom_range(layer)?;
        if layer.type_ == "background" || layer.type_ == "custom" {
            return Ok(());
        }
        let Some(source_name) = &layer.source else {
            return Err(StyleMutationError::SourceRequired { layer: id });
        };
        let Some(source) = self.sources.get(source_name) else {
            return Err(StyleMutationError::MissingSource {
                layer: id,
                source_name: source_name.clone(),
            });
        };
        let fits = match layer.type_.as_str() {
            "raster" => matches!(
                source,
                Source::Raster(_)
                    | Source::RasterDem(_)
                    | Source::Image(_)
                    | Source::Video(_)
                    | Source::Canvas(_)
            ),
            "hillshade" | "color-relief" => matches!(source, Source::RasterDem(_)),
            _ => matches!(source, Source::Vector(_) | Source::GeoJson(_)),
        };
        if !fits {
            return Err(StyleMutationError::WrongSourceKind {
                layer: id,
                kind: layer.type_.clone(),
                source_name: source_name.clone(),
            });
        }
        if matches!(source, Source::Vector(_)) && layer.source_layer.is_none() {
            return Err(StyleMutationError::SourceLayerRequired {
                layer: id,
                source_name: source_name.clone(),
            });
        }
        Ok(())
    }
}

/// Refuses a layer whose minimum zoom lies above its maximum.
pub(super) fn ensure_zoom_range(layer: &StyleLayer) -> Result<(), StyleMutationError> {
    match (layer.minzoom, layer.maxzoom) {
        (Some(minzoom), Some(maxzoom)) if minzoom > maxzoom => {
            Err(StyleMutationError::InvalidZoomRange {
                layer: layer.id.clone(),
                minzoom,
                maxzoom,
            })
        }
        _ => Ok(()),
    }
}
