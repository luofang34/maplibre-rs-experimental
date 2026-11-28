//! Style-revision cache tracking active background layers and alpha/pattern requirements.

#[derive(Default)]
pub(super) struct BackgroundStyleCache {
    layers_rev: Option<u64>,
    bg_layer_indices: Vec<usize>,
    key: Option<(u64, u64, u64)>,
    needs_bg_pattern: bool,
    has_opaque_bg: bool,
    has_blended_bg: bool,
}

impl BackgroundStyleCache {
    pub(super) fn evaluate(
        &mut self,
        style: &crate::style::Style,
        zoom: f64,
        style_zoom: f64,
    ) -> (bool, bool, bool) {
        let rev = style.revision();
        if self.layers_rev != Some(rev) {
            self.layers_rev = Some(rev);
            self.bg_layer_indices.clear();
            for (idx, layer) in style.layers.iter().enumerate() {
                if layer.type_ == "background" {
                    self.bg_layer_indices.push(idx);
                }
            }
        }
        let key = (rev, zoom.to_bits(), style_zoom.to_bits());
        if self.key == Some(key) {
            return (
                self.needs_bg_pattern,
                self.has_opaque_bg,
                self.has_blended_bg,
            );
        }
        let mut needs_bg_pattern = false;
        let (mut has_opaque_bg, mut has_blended_bg) = (false, false);
        for &idx in &self.bg_layer_indices {
            let Some(layer) = style.layers.get(idx) else {
                continue;
            };
            let patterned = layer
                .paint
                .as_ref()
                .is_some_and(|p| crate::vector::pattern::pattern_name(p, style_zoom).is_some());
            needs_bg_pattern |= patterned;
            if patterned || !layer.is_visible_at(zoom) {
                continue;
            }
            if crate::background::buffers::background_color(layer, zoom)[3] >= 1.0 {
                has_opaque_bg = true;
            } else {
                has_blended_bg = true;
            }
        }
        self.key = Some(key);
        self.needs_bg_pattern = needs_bg_pattern;
        self.has_opaque_bg = has_opaque_bg;
        self.has_blended_bg = has_blended_bg;
        (needs_bg_pattern, has_opaque_bg, has_blended_bg)
    }
}
