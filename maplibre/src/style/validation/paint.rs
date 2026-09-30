//! Property evaluation capabilities of each paint path.

use super::{
    Evaluation::{Constant, Density, Elevation, Feature, Zoom},
    LayerValidation,
};
use crate::style::{
    layer::LayerPaint,
    property::{NumberList, StyleProperty},
};

impl LayerValidation<'_> {
    pub(super) fn paint(&mut self, paint: &LayerPaint) {
        match paint {
            LayerPaint::Background(p) => {
                self.property("paint.background-color", p.background_color.as_ref(), Zoom);
                self.property(
                    "paint.background-opacity",
                    p.background_opacity.as_ref(),
                    Zoom,
                );
            }
            LayerPaint::Fill(p) => {
                self.property("paint.fill-color", p.fill_color.as_ref(), Feature);
                self.property("paint.fill-opacity", p.fill_opacity.as_ref(), Feature);
                // Edges are antialiased by the renderer's multisampling, which a layer cannot
                // turn off; asking for no antialiasing is the one request that is not honoured.
                if p.fill_antialias
                    .as_ref()
                    .is_some_and(|value| *value != serde_json::Value::Bool(true))
                {
                    self.unsupported(
                        "paint.fill-antialias",
                        "only antialiased fills are drawn; multisampling cannot be turned off per layer",
                    );
                }
            }
            LayerPaint::FillExtrusion(p) => {
                let color = p.fill_extrusion_color.as_ref();
                self.property("paint.fill-extrusion-color", color, Feature);
                let height = p.fill_extrusion_height.as_ref();
                self.property("paint.fill-extrusion-height", height, Feature);
                let base = p.fill_extrusion_base.as_ref();
                self.property("paint.fill-extrusion-base", base, Feature);
                let opacity = p.fill_extrusion_opacity.as_ref();
                self.property("paint.fill-extrusion-opacity", opacity, Zoom);
            }
            LayerPaint::Line(p) => {
                self.property("paint.line-color", p.line_color.as_ref(), Feature);
                self.property("paint.line-opacity", p.line_opacity.as_ref(), Feature);
                self.property("paint.line-width", p.line_width.as_ref(), Zoom);
                if let Some(value) = &p.line_dasharray {
                    self.property(
                        "paint.line-dasharray",
                        Some(&StyleProperty::<NumberList>::parse(value)),
                        Zoom,
                    );
                }
            }
            LayerPaint::Circle(p) => self.circle(p),
            LayerPaint::Hillshade(p) => self.hillshade(p),
            LayerPaint::ColorRelief(p) => {
                self.property(
                    "paint.color-relief-opacity",
                    p.color_relief_opacity.as_ref(),
                    Zoom,
                );
                self.property(
                    "paint.color-relief-color",
                    p.color_relief_color.as_ref(),
                    Elevation,
                );
            }
            LayerPaint::Heatmap(p) => {
                self.property("paint.heatmap-radius", p.heatmap_radius.as_ref(), Feature);
                self.property("paint.heatmap-weight", p.heatmap_weight.as_ref(), Feature);
                self.property(
                    "paint.heatmap-intensity",
                    p.heatmap_intensity.as_ref(),
                    Zoom,
                );
                self.property("paint.heatmap-opacity", p.heatmap_opacity.as_ref(), Zoom);
                self.property("paint.heatmap-color", p.heatmap_color.as_ref(), Density);
            }
            LayerPaint::Symbol(p) => self.symbol(p),
            LayerPaint::Raster(p) => self.raster(p),
        }
    }

    fn circle(&mut self, p: &crate::style::circle::CirclePaint) {
        self.property("paint.circle-color", p.circle_color.as_ref(), Feature);
        self.property("paint.circle-radius", p.circle_radius.as_ref(), Feature);
        self.property("paint.circle-opacity", p.circle_opacity.as_ref(), Feature);
        self.property("paint.circle-blur", p.circle_blur.as_ref(), Zoom);
        self.property(
            "paint.circle-stroke-width",
            p.circle_stroke_width.as_ref(),
            Feature,
        );
        self.property(
            "paint.circle-stroke-color",
            p.circle_stroke_color.as_ref(),
            Constant,
        );
        self.property(
            "paint.circle-stroke-opacity",
            p.circle_stroke_opacity.as_ref(),
            Zoom,
        );
    }

    fn hillshade(&mut self, p: &crate::style::hillshade::HillshadePaint) {
        self.property(
            "paint.hillshade-illumination-direction",
            p.hillshade_illumination_direction.as_ref(),
            Zoom,
        );
        self.property(
            "paint.hillshade-illumination-altitude",
            p.hillshade_illumination_altitude.as_ref(),
            Zoom,
        );
        self.property(
            "paint.hillshade-shadow-color",
            p.hillshade_shadow_color.as_ref(),
            Zoom,
        );
        self.property(
            "paint.hillshade-highlight-color",
            p.hillshade_highlight_color.as_ref(),
            Zoom,
        );
        self.property(
            "paint.hillshade-accent-color",
            p.hillshade_accent_color.as_ref(),
            Zoom,
        );
        self.property(
            "paint.hillshade-exaggeration",
            p.hillshade_exaggeration.as_ref(),
            Zoom,
        );
    }

    fn raster(&mut self, p: &crate::style::layer::RasterPaint) {
        self.property("paint.raster-opacity", p.raster_opacity.as_ref(), Zoom);
        self.property(
            "paint.raster-brightness-min",
            p.raster_brightness_min.as_ref(),
            Zoom,
        );
        self.property(
            "paint.raster-brightness-max",
            p.raster_brightness_max.as_ref(),
            Zoom,
        );
        self.property("paint.raster-contrast", p.raster_contrast.as_ref(), Zoom);
        self.property(
            "paint.raster-hue-rotate",
            p.raster_hue_rotate.as_ref(),
            Zoom,
        );
        self.property(
            "paint.raster-saturation",
            p.raster_saturation.as_ref(),
            Zoom,
        );
        // The adjustments are applied by the raster shader; fading and nearest sampling are not.
        let changed = [
            (
                "raster-fade-duration",
                p.raster_fade_duration.is_some_and(|v| v != 0),
            ),
            (
                "raster-resampling",
                matches!(
                    p.raster_resampling,
                    Some(crate::style::layer::RasterResampling::Nearest)
                ),
            ),
        ];
        for (name, changed) in changed {
            if changed {
                self.unsupported(
                    &format!("paint.{name}"),
                    "raster paint adjustment is not implemented",
                );
            }
        }
    }
}
