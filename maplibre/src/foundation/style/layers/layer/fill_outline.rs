//! The outline GL JS draws along a fill's edges.

use csscolorparser::Color;

use super::{paint::FillPaint, StyleProperty};

impl FillPaint {
    /// The colour of the outline drawn along the polygons' edges.
    ///
    /// An antialiased fill outlines in its own colour unless `fill-outline-color` says otherwise;
    /// the outline softens the edges of an opaque fill and darkens those of a translucent one.
    pub fn outline_color(&self) -> Option<StyleProperty<Color>> {
        if self.fill_outline_color.is_some() {
            return self.fill_outline_color.clone();
        }
        let antialiased = self
            .fill_antialias
            .as_ref()
            .is_none_or(|flag| flag.as_bool().unwrap_or(true));
        if !antialiased || self.fill_pattern.is_some() {
            return None;
        }
        // The default fill is opaque black.
        Some(
            self.fill_color
                .clone()
                .unwrap_or(StyleProperty::Constant(Color::new(0.0, 0.0, 0.0, 1.0))),
        )
    }
}
