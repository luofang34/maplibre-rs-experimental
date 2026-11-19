//! Memoized per-revision style requirements for [`resource_system`](super::resource_system).

use crate::style::{
    layer::{LayerPaint, StyleLayer},
    Style,
};

#[derive(Default)]
pub(super) struct VectorPreparedState {
    revision: Option<u64>,
    terrain_active: bool,
    pub(super) needs_fill: bool,
    pub(super) needs_flat_lines: bool,
    pub(super) needs_terrain_lines: bool,
    pub(super) needs_elevated_lines: bool,
    pub(super) needs_circle: bool,
    pub(super) needs_circle_feature_stroke: bool,
    pub(super) needs_circle_dynamic_radius: bool,
    pub(super) extrusion_minzoom: Option<f64>,
    initial_zoom_checked: bool,
    eager_extrusion: bool,
    pub(super) needs_extrusion_pattern: bool,
    pub(super) needs_extrusion_translucent: bool,
    pub(super) has_pattern_paint: bool,
    pub(super) has_dash_or_pattern_line: bool,
    last_dash_revision: Option<u64>,
    last_pattern_revision: Option<u64>,
}

impl VectorPreparedState {
    /// Updates cached pipeline/layer requirements in a single pass when `style.revision()` changes.
    pub(super) fn sync_style(&mut self, style: &Style, terrain_active: bool) -> bool {
        let rev = style.revision();
        let terrain_needed = style.terrain.is_some() || terrain_active;
        if self.revision == Some(rev) && self.terrain_active == terrain_active {
            self.needs_terrain_lines = self.needs_flat_lines && terrain_needed;
            return false;
        }
        let mut needs_fill = false;
        let mut has_flat_line = false;
        let mut needs_elevated_lines = false;
        let mut needs_circle = false;
        let mut needs_circle_feature_stroke = false;
        #[cfg_attr(not(feature = "geojson"), allow(unused_mut))]
        let mut needs_circle_dynamic_radius = false;
        let mut extrusion_minzoom: Option<f64> = None;
        let mut needs_extrusion_pattern = false;
        let mut needs_extrusion_translucent = false;
        let mut has_pattern_paint = false;
        let mut has_dash_or_pattern_line = false;
        for layer in &style.layers {
            if layer.type_ == "fill" {
                needs_fill = true;
            } else if layer.type_ == "line" {
                if matches!(
                    crate::vector::structures::kind(layer),
                    Some(crate::vector::structures::StructureKind::ElevatedLine)
                ) {
                    needs_elevated_lines = true;
                } else {
                    has_flat_line = true;
                }
            } else if layer.type_ == "circle" {
                needs_circle = true;
                if !layer.is_hidden() {
                    if let Some(LayerPaint::Circle(circle)) = layer.paint.as_ref() {
                        if circle.stroke_color_per_feature().is_some() {
                            needs_circle_feature_stroke = true;
                        }
                    }
                    #[cfg(feature = "geojson")]
                    if crate::geojson::update::dynamic_points::uses_dynamic_circle_metadata(
                        style, layer,
                    ) {
                        needs_circle_dynamic_radius = true;
                    }
                }
            } else if layer.type_ == "fill-extrusion" && !layer.is_hidden() {
                let layer_minzoom = layer.minzoom.unwrap_or(0.0);
                extrusion_minzoom =
                    Some(extrusion_minzoom.map_or(layer_minzoom, |m| m.min(layer_minzoom)));
                if let Some(paint) = layer.paint.as_ref() {
                    if let LayerPaint::FillExtrusion(extrusion) = paint {
                        if extrusion.fill_extrusion_pattern.is_some()
                            || crate::vector::pattern::per_feature_pattern(paint).is_some()
                        {
                            needs_extrusion_pattern = true;
                        }
                        if extrusion.fill_extrusion_translucent == Some(true) {
                            needs_extrusion_translucent = true;
                        }
                    }
                }
            }
            if let Some(paint) = layer.paint.as_ref() {
                if !has_pattern_paint && layer_uses_pattern(paint) {
                    has_pattern_paint = true;
                }
                if !has_dash_or_pattern_line && line_uses_dash_or_pattern(layer, paint) {
                    has_dash_or_pattern_line = true;
                }
            }
        }
        self.revision = Some(rev);
        self.terrain_active = terrain_active;
        self.needs_fill = needs_fill;
        self.needs_flat_lines = has_flat_line;
        self.needs_terrain_lines = has_flat_line && terrain_needed;
        self.needs_elevated_lines = needs_elevated_lines;
        self.needs_circle = needs_circle;
        self.needs_circle_feature_stroke = needs_circle_feature_stroke;
        self.needs_circle_dynamic_radius = needs_circle_dynamic_radius;
        self.extrusion_minzoom = extrusion_minzoom;
        self.initial_zoom_checked = false;
        self.eager_extrusion = false;
        self.needs_extrusion_pattern = needs_extrusion_pattern;
        self.needs_extrusion_translucent = needs_extrusion_translucent;
        self.has_pattern_paint = has_pattern_paint;
        self.has_dash_or_pattern_line = has_dash_or_pattern_line;
        true
    }

    pub(super) fn needs_extrusion_at_zoom(&mut self, style_zoom: f64, uses_globe: bool) -> bool {
        if !self.initial_zoom_checked {
            self.initial_zoom_checked = true;
            if !uses_globe
                && self
                    .extrusion_minzoom
                    .is_some_and(|m| style_zoom + 1.0 >= m)
            {
                self.eager_extrusion = true;
            }
        }
        self.eager_extrusion
            || self
                .extrusion_minzoom
                .is_some_and(|m| style_zoom + 0.5 >= m)
    }

    pub(super) fn should_update_dashes(&mut self, style: &Style) -> bool {
        let rev = style.revision();
        let changed_rev = self.last_dash_revision != Some(rev);
        self.last_dash_revision = Some(rev);
        changed_rev || self.has_dash_or_pattern_line
    }

    pub(super) fn should_update_patterns(&mut self, style: &Style) -> bool {
        let rev = style.revision();
        let changed_rev = self.last_pattern_revision != Some(rev);
        self.last_pattern_revision = Some(rev);
        changed_rev || self.has_pattern_paint
    }
}

fn layer_uses_pattern(paint: &LayerPaint) -> bool {
    if crate::vector::pattern::per_feature_pattern(paint).is_some() {
        return true;
    }
    match paint {
        LayerPaint::Fill(p) => p.fill_pattern.is_some(),
        LayerPaint::Line(p) => p.line_pattern.is_some(),
        LayerPaint::FillExtrusion(p) => p.fill_extrusion_pattern.is_some(),
        _ => false,
    }
}

fn line_uses_dash_or_pattern(layer: &StyleLayer, paint: &LayerPaint) -> bool {
    let LayerPaint::Line(line) = paint else {
        return false;
    };
    line.line_dasharray.is_some()
        || line.line_pattern.is_some()
        || (layer.type_ == "line" && crate::vector::pattern::per_feature_pattern(paint).is_some())
}
