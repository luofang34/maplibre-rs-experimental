//! Which layers GL JS draws in its opaque pass, ahead of every translucent layer.
//!
//! An opaque background or fill below the first fill-extrusion layer writes depth in that pass.
//! Layers that test depth are hidden behind it exactly as painter order hides them; a heatmap
//! ignores depth and so shows over the opaque layers above it.

use super::{layer::LayerPaint, layer::StyleLayer, Style};

impl Style {
    /// The highest painter index of an opaque-pass layer drawn after `layer`, when `layer`
    /// draws over such layers regardless of its place in the order.
    pub fn opaque_layer_above(&self, layer: &StyleLayer, zoom: f64) -> Option<u32> {
        if self.terrain.is_some() {
            return None;
        }
        let cutoff = self
            .layers
            .iter()
            .find(|candidate| candidate.type_ == "fill-extrusion")
            .map_or(u32::MAX, |candidate| candidate.index);
        if layer.index >= cutoff {
            return None;
        }
        self.layers
            .iter()
            .filter(|candidate| {
                candidate.index > layer.index
                    && candidate.index < cutoff
                    && candidate.is_visible_at(zoom)
                    && candidate.is_opaque(zoom)
            })
            .map(|candidate| candidate.index)
            .max()
    }
}

impl StyleLayer {
    /// Whether the layer paints every pixel it covers with one opaque colour.
    fn is_opaque(&self, zoom: f64) -> bool {
        let opaque = |color: Option<f64>, opacity: Option<f64>| {
            color == Some(1.0) && opacity.unwrap_or(1.0) == 1.0
        };
        match self.paint.as_ref() {
            Some(LayerPaint::Background(paint)) => {
                paint.background_pattern.is_none()
                    && opaque(
                        paint
                            .background_color
                            .as_ref()
                            .and_then(|color| color.evaluate_at_zoom(zoom))
                            .map(|color| color.a),
                        paint
                            .background_opacity
                            .as_ref()
                            .and_then(|opacity| opacity.evaluate_at_zoom(zoom))
                            .map(f64::from),
                    )
            }
            Some(LayerPaint::Fill(paint)) => {
                paint.fill_pattern.is_none()
                    && paint.fill_color.as_ref().is_some_and(|color| {
                        color.is_feature_constant()
                            && opaque(
                                color.evaluate_at_zoom(zoom).map(|color| color.a),
                                paint
                                    .fill_opacity
                                    .as_ref()
                                    .filter(|opacity| opacity.is_feature_constant())
                                    .and_then(|opacity| opacity.evaluate_at_zoom(zoom))
                                    .map(f64::from)
                                    .or(Some(1.0)),
                            )
                    })
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests;
