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
    pub(super) needs_fill_opaque: bool,
    pub(super) needs_fill_blended: bool,
    pub(super) needs_flat_lines: bool,
    pub(super) needs_terrain_lines: bool,
    pub(super) needs_elevated_lines: bool,
    pub(super) needs_circle: bool,
    pub(super) needs_circle_feature_stroke: bool,
    pub(super) needs_circle_dynamic_radius: bool,
    pub(super) extrusion_minzoom: Option<f64>,
    initial_zoom_checked: bool,
    eager_extrusion: bool,
    pub(super) needs_extrusion_opaque: bool,
    pub(super) needs_extrusion_pattern: bool,
    pub(super) needs_extrusion_translucent: bool,
    pub(super) extrusion_layers: Vec<(String, Option<f64>, Option<f64>)>,
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
        let scan = scan_style_layers(style);
        self.revision = Some(rev);
        self.terrain_active = terrain_active;
        self.needs_fill = scan.needs_fill;
        self.needs_fill_opaque = scan.needs_fill_opaque;
        self.needs_fill_blended = scan.needs_fill_blended;
        self.needs_flat_lines = scan.has_flat_line;
        self.needs_terrain_lines = scan.has_flat_line && terrain_needed;
        self.needs_elevated_lines = scan.needs_elevated_lines;
        self.needs_circle = scan.needs_circle;
        self.needs_circle_feature_stroke = scan.needs_circle_feature_stroke;
        self.needs_circle_dynamic_radius = scan.needs_circle_dynamic_radius;
        self.extrusion_minzoom = scan.extrusion_minzoom;
        self.initial_zoom_checked = false;
        self.eager_extrusion = false;
        self.needs_extrusion_opaque = scan.needs_extrusion_opaque;
        self.needs_extrusion_pattern = scan.needs_extrusion_pattern;
        self.needs_extrusion_translucent = scan.needs_extrusion_translucent;
        self.extrusion_layers = scan.extrusion_layers;
        self.has_pattern_paint = scan.has_pattern_paint;
        self.has_dash_or_pattern_line = scan.has_dash_or_pattern_line;
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

    pub(super) fn is_visible_extrusion_layer(&self, id: &str, style_zoom: f64) -> bool {
        self.extrusion_layers.iter().any(|(layer_id, minz, maxz)| {
            layer_id == id
                && minz.is_none_or(|min| style_zoom >= min)
                && maxz.is_none_or(|max| style_zoom < max)
        })
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

#[derive(Default)]
struct StyleLayerScan {
    needs_fill: bool,
    needs_fill_opaque: bool,
    needs_fill_blended: bool,
    has_flat_line: bool,
    needs_elevated_lines: bool,
    needs_circle: bool,
    needs_circle_feature_stroke: bool,
    needs_circle_dynamic_radius: bool,
    extrusion_minzoom: Option<f64>,
    needs_extrusion_opaque: bool,
    needs_extrusion_pattern: bool,
    needs_extrusion_translucent: bool,
    extrusion_layers: Vec<(String, Option<f64>, Option<f64>)>,
    has_pattern_paint: bool,
    has_dash_or_pattern_line: bool,
}

fn scan_style_layers(style: &Style) -> StyleLayerScan {
    let mut scan = StyleLayerScan::default();
    for layer in &style.layers {
        scan.visit_layer(style, layer);
    }
    scan
}

impl StyleLayerScan {
    fn visit_layer(&mut self, _style: &Style, layer: &StyleLayer) {
        match layer.type_.as_str() {
            "fill" => {
                self.needs_fill = true;
                let (op, bl) = classify_fill_opacity(layer);
                self.needs_fill_opaque |= op;
                self.needs_fill_blended |= bl;
            }
            "line" => {
                if matches!(
                    crate::vector::structures::kind(layer),
                    Some(crate::vector::structures::StructureKind::ElevatedLine)
                ) {
                    self.needs_elevated_lines = true;
                } else {
                    self.has_flat_line = true;
                }
            }
            "circle" => {
                self.needs_circle = true;
                if !layer.is_hidden() {
                    if let Some(LayerPaint::Circle(circle)) = layer.paint.as_ref()
                        && circle.stroke_color_per_feature().is_some()
                    {
                        self.needs_circle_feature_stroke = true;
                    }
                    #[cfg(feature = "geojson")]
                    if crate::geojson::update::dynamic_points::uses_dynamic_circle_metadata(
                        _style, layer,
                    ) {
                        self.needs_circle_dynamic_radius = true;
                    }
                }
            }
            "fill-extrusion" if !layer.is_hidden() => self.visit_extrusion_layer(layer),
            _ => {}
        }
        if let Some(paint) = layer.paint.as_ref() {
            self.has_pattern_paint |= layer_uses_pattern(paint);
            self.has_dash_or_pattern_line |= line_uses_dash_or_pattern(layer, paint);
        }
    }

    fn visit_extrusion_layer(&mut self, layer: &StyleLayer) {
        let layer_minzoom = layer.minzoom.unwrap_or(0.0);
        self.extrusion_minzoom = Some(
            self.extrusion_minzoom
                .map_or(layer_minzoom, |m| m.min(layer_minzoom)),
        );
        let mut is_translucent = false;
        if let Some(LayerPaint::FillExtrusion(extrusion)) = layer.paint.as_ref() {
            if extrusion.fill_extrusion_pattern.is_some()
                || crate::vector::pattern::per_feature_pattern(
                    layer
                        .paint
                        .as_ref()
                        .unwrap_or(&LayerPaint::Background(Default::default())),
                )
                .is_some()
            {
                self.needs_extrusion_pattern = true;
            }
            if extrusion.fill_extrusion_translucent == Some(true) {
                self.needs_extrusion_translucent = true;
                is_translucent = true;
            }
        }
        if !is_translucent {
            self.needs_extrusion_opaque = true;
        }
        self.extrusion_layers
            .push((layer.id.clone(), layer.minzoom, layer.maxzoom));
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

fn classify_fill_opacity(layer: &StyleLayer) -> (bool, bool) {
    let Some(LayerPaint::Fill(fill)) = layer.paint.as_ref() else {
        return (true, false);
    };
    if fill.fill_pattern.is_some() {
        return (false, true);
    }
    let constant = fill
        .fill_color
        .as_ref()
        .is_none_or(|p| p.is_zoom_constant() && p.is_feature_constant())
        && fill
            .fill_opacity
            .as_ref()
            .is_none_or(|p| p.is_zoom_constant() && p.is_feature_constant())
        && fill
            .fill_outline_color
            .as_ref()
            .is_none_or(|p| p.is_zoom_constant() && p.is_feature_constant());
    if !constant {
        return (true, true);
    }
    let opacity = fill
        .fill_opacity
        .as_ref()
        .and_then(|p| p.evaluate_at_zoom(0.0))
        .unwrap_or(1.0)
        .clamp(0.0, 1.0);
    let fill_alpha = fill
        .fill_color
        .as_ref()
        .and_then(|p| p.evaluate_at_zoom(0.0))
        .unwrap_or_default()
        .a
        * opacity;
    let outline_alpha = fill
        .fill_outline_color
        .as_ref()
        .and_then(|p| p.evaluate_at_zoom(0.0))
        .map_or(1.0, |c| c.a * opacity);
    if fill_alpha >= 1.0 && outline_alpha >= 1.0 {
        (true, false)
    } else {
        (false, true)
    }
}

fn line_uses_dash_or_pattern(layer: &StyleLayer, paint: &LayerPaint) -> bool {
    let LayerPaint::Line(line) = paint else {
        return false;
    };
    line.line_dasharray.is_some()
        || line.line_pattern.is_some()
        || line.line_gradient.is_some()
        || (layer.type_ == "line" && crate::vector::pattern::per_feature_pattern(paint).is_some())
}
