//! The outline GL JS draws along a fill's edges.

use csscolorparser::Color;

use super::{paint::FillPaint, StyleProperty};

impl FillPaint {
    /// The colour of the outline drawn along the polygons' edges at `zoom`.
    ///
    /// An antialiased fill outlines in its own colour unless `fill-outline-color` says otherwise,
    /// which only shows where the fill is translucent; a fill that is opaque everywhere skips
    /// the outline.
    pub fn outline_color(&self, zoom: f64) -> Option<StyleProperty<Color>> {
        if self.fill_outline_color.is_some() {
            return self.fill_outline_color.clone();
        }
        let antialiased = self
            .fill_antialias
            .as_ref()
            .is_none_or(|flag| flag.as_bool().unwrap_or(true));
        if !antialiased || self.fill_pattern.is_some() || self.is_opaque(zoom) {
            return None;
        }
        self.fill_color.clone()
    }

    fn is_opaque(&self, zoom: f64) -> bool {
        let color_opaque = self.fill_color.as_ref().is_none_or(|color| {
            color.is_feature_constant()
                && color
                    .evaluate_at_zoom(zoom)
                    .is_none_or(|color| color.a >= 1.0)
        });
        let opacity_full = self.fill_opacity.as_ref().is_none_or(|opacity| {
            opacity.is_feature_constant()
                && opacity
                    .evaluate_at_zoom(zoom)
                    .is_none_or(|opacity| opacity >= 1.0)
        });
        color_opaque && opacity_full
    }
}
