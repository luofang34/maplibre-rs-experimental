//! Resumable feature-level slicing over a single symbol style layer's sorted features.

use super::{
    anchors::{ordered_features, tile_culled_on_flat_mercator},
    empty_metadata, rules, temporal, write_feature_metadata, LayerFrame,
};
use crate::{
    render::shaders::SDFShaderFeatureMetadata,
    sdf::{
        collision_grid::CollisionGrid, paint::SymbolUniforms, placement::LinePoses,
        query::PlacedSymbols,
    },
};

#[cfg(target_arch = "wasm32")]
const FEATURE_BATCH_CHECK: usize = 16;
#[cfg(not(target_arch = "wasm32"))]
const FEATURE_BATCH_CHECK: usize = 32;

/// In-progress placement state across a style layer's visible tiles.
pub(in crate::sdf::collision_system) struct LayerSliceProgress {
    ordered: Vec<(usize, usize, bool)>,
    cursor: usize,
    pub(in crate::sdf::collision_system) metadata: Vec<Vec<SDFShaderFeatureMetadata>>,
    pub(in crate::sdf::collision_system) outcomes: super::super::family::Outcomes,
}

impl LayerSliceProgress {
    pub(in crate::sdf::collision_system) fn start(
        world: &crate::tcs::world::World,
        view_state: &crate::render::view_state::ViewState,
        projection: &crate::render::projection::ShaderProjectionData,
        layers: &[&crate::sdf::SymbolLayerData],
        paint: &crate::style::layer::SymbolPaint,
        history: &temporal::PlacementHistory,
    ) -> Self {
        let metadata: Vec<_> = layers.iter().map(|layer| empty_metadata(layer)).collect();
        let outcomes: super::super::family::Outcomes =
            (0..layers.len()).map(|_| Vec::new()).collect();
        let zoom = view_state.style_zoom().value();
        let uniforms = SymbolUniforms::new(paint, zoom, [1, 1]);
        let has_terrain_or_height = world
            .resources
            .get::<crate::terrain::coverage::TerrainCoverageIndex>()
            .is_some()
            || paint.uses_shared_height();
        let culled: Vec<bool> = layers
            .iter()
            .map(|layer| {
                tile_culled_on_flat_mercator(
                    layer,
                    view_state,
                    projection,
                    &uniforms,
                    has_terrain_or_height,
                    history,
                )
            })
            .collect();
        if culled.iter().all(|&c| c) {
            return Self {
                ordered: Vec::new(),
                cursor: 0,
                metadata,
                outcomes,
            };
        }
        let ordered = ordered_features(layers, &culled, history)
            .into_iter()
            .filter_map(|(pos, feat_idx, feature, was_visible)| {
                (!feature.fallback).then_some((pos, feat_idx, was_visible))
            })
            .collect();
        Self {
            ordered,
            cursor: 0,
            metadata,
            outcomes,
        }
    }

    /// Advances feature placement until complete (`true`) or `should_yield()` triggers (`false`).
    #[allow(clippy::too_many_arguments)]
    pub(in crate::sdf::collision_system) fn step(
        &mut self,
        world: &crate::tcs::world::World,
        view_state: &crate::render::view_state::ViewState,
        projection: &crate::render::projection::ShaderProjectionData,
        layers: &[Option<&crate::sdf::SymbolLayerData>],
        paint: &crate::style::layer::SymbolPaint,
        zoom_limits: [f64; 2],
        placement: (
            &mut CollisionGrid,
            &mut PlacedSymbols,
            &mut temporal::PlacementHistory,
            &mut super::super::upload_diff::MetadataScratchPool,
        ),
        mut should_yield: impl FnMut() -> bool,
    ) -> bool {
        if self.cursor >= self.ordered.len() {
            return true;
        }
        let (boxes, placed, history, scratch) = placement;
        let zoom = view_state.style_zoom().value();
        let has_data_driven_sizes = paint
            .text_size
            .as_ref()
            .is_some_and(|s| !s.is_feature_constant())
            || paint
                .compiled_property::<f32>("icon-size")
                .is_some_and(|s| !s.is_feature_constant());
        let has_terrain_or_height = world
            .resources
            .get::<crate::terrain::coverage::TerrainCoverageIndex>()
            .is_some()
            || paint.uses_shared_height();
        let repeat_distance = f64::from(paint.number(
            "symbol-spacing",
            &crate::style::expression::FeatureProperties::default(),
            zoom,
            super::super::repeat_spacing::MIN_REPEAT_DISTANCE as f32,
        ))
        .clamp(1.0, super::super::repeat_spacing::MIN_REPEAT_DISTANCE);
        let frame = LayerFrame {
            world,
            view_state,
            projection,
            paint,
            uniforms: SymbolUniforms::new(paint, zoom, [1, 1]),
            zoom_limits,
            glyphs_stay_upright: crate::sdf::paint::glyphs_stay_upright(paint, zoom),
            has_data_driven_sizes,
            has_terrain_or_height,
            has_icon_text_fit: paint.properties.contains_key("icon-text-fit"),
            constant_rules: rules::PlacementRules::constant_for_layer(paint, zoom),
            visibility_policy: world
                .resources
                .get::<crate::sdf::visibility::SymbolVisibility>(),
            repeat_distance,
        };

        let mut processed_in_step = 0_usize;
        while self.cursor < self.ordered.len() {
            if processed_in_step > 0
                && processed_in_step.is_multiple_of(FEATURE_BATCH_CHECK)
                && should_yield()
            {
                return false;
            }
            let (position, feature_index, was_visible) = self.ordered[self.cursor];
            self.cursor += 1;
            processed_in_step += 1;

            let Some(layer) = layers.get(position).copied().flatten() else {
                continue;
            };
            let Some(initial_feature) = layer.features.get(feature_index) else {
                continue;
            };
            let mut next = Some((feature_index, initial_feature, was_visible, false));
            while let Some((index, feature, was_visible, suppressed)) = next.take() {
                let feat_frame = LayerFrame {
                    uniforms: if frame.has_data_driven_sizes {
                        frame
                            .uniforms
                            .with_feature_sizes(paint, &feature.data.properties, zoom)
                    } else {
                        frame.uniforms
                    },
                    ..frame
                };
                let outcome = feat_frame.place_feature(
                    (layer, feature, index, was_visible, suppressed),
                    (&mut *boxes, &mut *placed, &mut *history),
                );
                let poses = match &outcome.line {
                    Some(LinePoses::Poses(poses)) => Some(poses.clone()),
                    _ => None,
                };
                if (!outcome.ramps[0].is_hidden() || !outcome.ramps[1].is_hidden())
                    && self.metadata[position].capacity() == 0
                {
                    self.metadata[position] = scratch.acquire();
                }
                write_feature_metadata(
                    layer,
                    feature,
                    (outcome.ramps, outcome.text_shift, outcome.anchor),
                    outcome.ground,
                    poses.as_deref(),
                    (paint, zoom, &mut self.metadata[position]),
                );
                if !outcome.ramps[0].is_hidden() || !outcome.ramps[1].is_hidden() {
                    if self.outcomes[position].is_empty() {
                        self.outcomes[position].resize_with(layer.features.len(), || None);
                    }
                    if let Some(slot) = self.outcomes[position].get_mut(index) {
                        *slot = Some(super::super::family::Outcome {
                            ramps: outcome.ramps,
                            text_shift: outcome.text_shift,
                            anchor: outcome.anchor,
                            ground: outcome.ground,
                            poses,
                        });
                    }
                }
                next = layer
                    .features
                    .get(index + 1)
                    .filter(|other| other.fallback)
                    .map(|other| {
                        (
                            index + 1,
                            other,
                            history.was_visible(layer, other),
                            suppressed || outcome.placed,
                        )
                    });
            }
        }
        true
    }

    pub(in crate::sdf::collision_system) fn recycle(
        self,
        scratch: &mut super::super::upload_diff::MetadataScratchPool,
    ) {
        for buf in self.metadata {
            scratch.recycle(buf);
        }
    }
}
