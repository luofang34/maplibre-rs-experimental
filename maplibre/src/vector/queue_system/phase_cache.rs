//! Per-style compiled layer predicates and sorted draw-phase snapshot caching for
//! [`super::queue_system`].

use std::hash::{Hash, Hasher};

use rustc_hash::{FxHashSet, FxHasher};

use crate::{
    render::render_phase::{LayerItem, TileMaskItem},
    style::{
        layer::{LayerPaint, PatternName, StyleLayer},
        property::StyleProperty,
        Style, StyleLayerId,
    },
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct VectorPhaseCacheKey {
    pub(super) topology_gen: u64,
    pub(super) buffer_pool_rev: u64,
    pub(super) style_rev: u64,
    pub(super) layer_state_digest: u64,
    pub(super) uses_globe: bool,
    pub(super) skip_draped: bool,
}

pub(super) struct CachedVectorPhases {
    pub(super) key: VectorPhaseCacheKey,
    pub(super) masks: Vec<TileMaskItem>,
    pub(super) layers: Vec<LayerItem>,
}

struct CompiledLayerRule {
    id: StyleLayerId,
    index: u32,
    minzoom: Option<f64>,
    maxzoom: Option<f64>,
    is_translucent_extrusion: bool,
    is_unclipped_3d: bool,
    is_drapeable: bool,
    is_heatmap: bool,
    pattern_prop: Option<StyleProperty<PatternName>>,
    missing_pattern_prop: Option<StyleProperty<PatternName>>,
    opacity_prop: Option<StyleProperty<f32>>,
    width_prop: Option<StyleProperty<f32>>,
}

impl CompiledLayerRule {
    fn from_layer(layer: &StyleLayer) -> Self {
        let is_translucent_extrusion = matches!(
            &layer.paint,
            Some(LayerPaint::FillExtrusion(paint)) if paint.fill_extrusion_translucent == Some(true)
        );
        let is_unclipped_3d = is_translucent_extrusion
            || matches!(
                crate::vector::structures::kind(layer),
                Some(crate::vector::structures::StructureKind::ElevatedLine)
            );
        let is_drapeable = crate::terrain::is_drapeable(&layer.type_)
            && crate::vector::structures::kind(layer).is_none();
        let is_heatmap = layer.type_ == "heatmap";
        let raw_pattern = match &layer.paint {
            Some(LayerPaint::Fill(fill)) => fill.fill_pattern.as_ref(),
            Some(LayerPaint::Line(line)) => line.line_pattern.as_ref(),
            Some(LayerPaint::FillExtrusion(extrusion)) => extrusion.fill_extrusion_pattern.as_ref(),
            Some(LayerPaint::Background(background)) => background.background_pattern.as_ref(),
            _ => None,
        };
        let pattern_prop = raw_pattern.map(StyleProperty::<PatternName>::parse);
        let missing_pattern_prop = pattern_prop
            .as_ref()
            .filter(|prop| prop.is_feature_constant())
            .cloned();

        let (raw_opacity, raw_width) = if layer.transitions.is_empty() {
            match &layer.paint {
                Some(LayerPaint::Fill(p)) => (p.fill_opacity.as_ref(), None),
                Some(LayerPaint::Line(p)) => (p.line_opacity.as_ref(), p.line_width.as_ref()),
                Some(LayerPaint::FillExtrusion(p)) => (p.fill_extrusion_opacity.as_ref(), None),
                _ => (None, None),
            }
        } else {
            (None, None)
        };
        let opacity_prop = raw_opacity.filter(|p| p.is_feature_constant()).cloned();
        let width_prop = raw_width.filter(|p| p.is_feature_constant()).cloned();

        Self {
            id: layer.layer_id(),
            index: layer.index,
            minzoom: layer.minzoom,
            maxzoom: layer.maxzoom,
            is_translucent_extrusion,
            is_unclipped_3d,
            is_drapeable,
            is_heatmap,
            pattern_prop,
            missing_pattern_prop,
            opacity_prop,
            width_prop,
        }
    }

    #[inline]
    fn is_visible_at(&self, zoom: f64) -> bool {
        self.minzoom.is_none_or(|min| zoom >= min) && self.maxzoom.is_none_or(|max| zoom < max)
    }

    #[inline]
    fn is_zero_opacity(&self, zoom: f64) -> bool {
        let zero = |prop: Option<&StyleProperty<f32>>| {
            prop.is_some_and(|p| p.evaluate_at_zoom(zoom).is_some_and(|v| v <= 0.0))
        };
        zero(self.opacity_prop.as_ref()) || zero(self.width_prop.as_ref())
    }

    #[inline]
    fn has_pattern(&self, has_per_feature: bool, zoom: f64) -> bool {
        has_per_feature
            || self.pattern_prop.as_ref().is_some_and(|prop| {
                prop.evaluate_at_zoom(zoom)
                    .is_some_and(|name| !name.0.is_empty())
            })
    }

    #[inline]
    fn names_missing_image(&self, style: &Style, zoom: f64) -> bool {
        self.missing_pattern_prop.as_ref().is_some_and(|prop| {
            prop.evaluate_at_zoom(zoom)
                .is_none_or(|name| !style.images.contains_key(&name.0))
        })
    }
}

#[derive(Default)]
pub(super) struct CompiledStyleRules {
    style_rev: Option<u64>,
    rules: Vec<(CompiledLayerRule, bool)>,
}

impl CompiledStyleRules {
    fn sync(&mut self, style: &Style) -> bool {
        let rev = style.revision();
        if self.style_rev == Some(rev) {
            return false;
        }
        self.style_rev = Some(rev);
        self.rules.clear();
        self.rules.reserve(style.layers.len());
        for layer in &style.layers {
            let has_per_feature = layer
                .paint
                .as_ref()
                .and_then(crate::vector::pattern::per_feature_pattern)
                .is_some();
            self.rules
                .push((CompiledLayerRule::from_layer(layer), has_per_feature));
        }
        true
    }
}

#[derive(Default)]
pub(super) struct VectorQueueScratch {
    compiled: CompiledStyleRules,
    last_digest: Option<(u64, u64)>,
    last_style_zoom_bits: Option<u64>,
    pub(super) layer_state_digest: u64,
    pub(super) pattern_layers: FxHashSet<StyleLayerId>,
    pub(super) missing_image_layers: FxHashSet<StyleLayerId>,
    pub(super) translucent_extrusion_layers: FxHashSet<StyleLayerId>,
    pub(super) unclipped_3d_layers: FxHashSet<StyleLayerId>,
    pub(super) zero_opacity_layers: FxHashSet<StyleLayerId>,
    pub(super) drapeable_layers: FxHashSet<StyleLayerId>,
    pub(super) has_visible_heatmap: bool,
    pub(super) active_unclipped_3d_shapes: FxHashSet<(i32, crate::coords::WorldTileCoords, u32)>,
    pub(super) queued_unclipped_3d: FxHashSet<(i32, crate::coords::WorldTileCoords, u32)>,
    pub(super) extrusion_colors: Vec<LayerItem>,
    pub(super) extrusion_clears: Vec<LayerItem>,
    pub(super) translucent_fronts: Vec<LayerItem>,
    pub(super) cached_phases: Option<CachedVectorPhases>,
}

impl VectorQueueScratch {
    /// Evaluates zoom-dependent layer flags without reallocating or reparsing expressions; returns
    /// cleanly with the exact `layer_state_digest` and only rebuilds hash sets when flags change.
    pub(super) fn sync_style_sets(&mut self, style: &Style, style_zoom: f64) -> u64 {
        let style_changed = self.compiled.sync(style);
        let style_rev = style.revision();
        let zoom_bits = style_zoom.to_bits();
        if style_changed {
            self.last_style_zoom_bits = None;
        } else if self.last_style_zoom_bits == Some(zoom_bits) {
            return self.layer_state_digest;
        }
        self.last_style_zoom_bits = Some(zoom_bits);

        let mut hasher = FxHasher::default();
        self.compiled.rules.len().hash(&mut hasher);
        for (rule, has_per_feature) in &self.compiled.rules {
            let bits = u8::from(rule.is_visible_at(style_zoom))
                | (u8::from(rule.has_pattern(*has_per_feature, style_zoom)) << 1)
                | (u8::from(rule.names_missing_image(style, style_zoom)) << 2)
                | (u8::from(rule.is_zero_opacity(style_zoom)) << 3)
                | (u8::from(rule.is_translucent_extrusion) << 4)
                | (u8::from(rule.is_unclipped_3d) << 5)
                | (u8::from(rule.is_drapeable) << 6)
                | (u8::from(rule.is_heatmap) << 7);
            (rule.id, rule.index, bits).hash(&mut hasher);
        }
        let digest = hasher.finish();
        self.layer_state_digest = digest;

        if !style_changed && self.last_digest == Some((style_rev, digest)) {
            return digest;
        }
        self.last_digest = Some((style_rev, digest));
        self.pattern_layers.clear();
        self.missing_image_layers.clear();
        self.translucent_extrusion_layers.clear();
        self.unclipped_3d_layers.clear();
        self.zero_opacity_layers.clear();
        self.drapeable_layers.clear();
        self.has_visible_heatmap = false;

        for (rule, has_per_feature) in &self.compiled.rules {
            if rule.has_pattern(*has_per_feature, style_zoom) {
                self.pattern_layers.insert(rule.id);
            }
            if rule.names_missing_image(style, style_zoom) {
                self.missing_image_layers.insert(rule.id);
            }
            if rule.is_translucent_extrusion {
                self.translucent_extrusion_layers.insert(rule.id);
            }
            if rule.is_unclipped_3d {
                self.unclipped_3d_layers.insert(rule.id);
            }
            if rule.is_zero_opacity(style_zoom) {
                self.zero_opacity_layers.insert(rule.id);
            }
            if rule.is_drapeable {
                self.drapeable_layers.insert(rule.id);
            }
            if rule.is_heatmap && rule.is_visible_at(style_zoom) {
                self.has_visible_heatmap = true;
            }
        }
        digest
    }
}
