//! Memoized per-revision style requirements for [`resource_system`](super::resource_system).

use crate::style::{
    layer::{LayerPaint, StyleLayer},
    Style,
};

#[derive(Default)]
pub(super) struct VectorPreparedState {
    revision: Option<u64>,
    terrain_active: bool,
    pub(super) needs_terrain_lines: bool,
    pub(super) needs_elevated_lines: bool,
    pub(super) needs_circle: bool,
    pub(super) needs_extrusion: bool,
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
            self.needs_terrain_lines = terrain_needed;
            return false;
        }
        let mut needs_elevated_lines = false;
        let mut needs_circle = false;
        let mut needs_extrusion = false;
        let mut has_pattern_paint = false;
        let mut has_dash_or_pattern_line = false;
        for layer in &style.layers {
            if !needs_elevated_lines
                && matches!(
                    crate::vector::structures::kind(layer),
                    Some(crate::vector::structures::StructureKind::ElevatedLine)
                )
            {
                needs_elevated_lines = true;
            }
            if layer.type_ == "circle" {
                needs_circle = true;
            } else if layer.type_ == "fill-extrusion" {
                needs_extrusion = true;
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
        self.needs_terrain_lines = terrain_needed;
        self.needs_elevated_lines = needs_elevated_lines;
        self.needs_circle = needs_circle;
        self.needs_extrusion = needs_extrusion;
        self.has_pattern_paint = has_pattern_paint;
        self.has_dash_or_pattern_line = has_dash_or_pattern_line;
        true
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
