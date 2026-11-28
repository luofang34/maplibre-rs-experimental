//! Resumable feature-level slicing over a single symbol style layer's sorted features.

use super::{
    super::{policy::rules, temporal},
    anchors::{ordered_features, tile_culled_on_flat_mercator},
    empty_metadata, write_feature_metadata, LayerFrame,
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
pub(in crate::layers::sdf::collision_system) struct LayerSliceProgress {
    ordered: Vec<(usize, usize, bool)>,
    cursor: usize,
    pub(in crate::layers::sdf::collision_system) metadata: Vec<Vec<SDFShaderFeatureMetadata>>,
    pub(in crate::layers::sdf::collision_system) outcomes: super::family::Outcomes,
}

/// Shared read-only frame and layer inputs for stepping a symbol slice.
pub(in crate::layers::sdf::collision_system) struct SliceStepContext<'a> {
    pub(in crate::layers::sdf::collision_system) world: &'a crate::tcs::world::World,
    pub(in crate::layers::sdf::collision_system) view_state:
        &'a crate::render::view_state::ViewState,
    pub(in crate::layers::sdf::collision_system) projection:
        &'a crate::render::projection::ShaderProjectionData,
    pub(in crate::layers::sdf::collision_system) layers:
        &'a [Option<&'a crate::sdf::SymbolLayerData>],
    pub(in crate::layers::sdf::collision_system) paint: &'a crate::style::layer::SymbolPaint,
    pub(in crate::layers::sdf::collision_system) zoom_limits: [f64; 2],
}

/// Mutable placement outputs updated across sliced symbol steps.
pub(in crate::layers::sdf::collision_system) struct SlicePlacementState<'a> {
    pub(in crate::layers::sdf::collision_system) boxes: &'a mut CollisionGrid,
    pub(in crate::layers::sdf::collision_system) placed: &'a mut PlacedSymbols,
    pub(in crate::layers::sdf::collision_system) history: &'a mut temporal::PlacementHistory,
    pub(in crate::layers::sdf::collision_system) scratch:
        &'a mut super::super::upload_diff::MetadataScratchPool,
}

impl LayerSliceProgress {
    pub(in crate::layers::sdf::collision_system) fn start(
        world: &crate::tcs::world::World,
        view_state: &crate::render::view_state::ViewState,
        projection: &crate::render::projection::ShaderProjectionData,
        layers: &[&crate::sdf::SymbolLayerData],
        paint: &crate::style::layer::SymbolPaint,
        history: &temporal::PlacementHistory,
    ) -> Self {
        let metadata: Vec<_> = layers.iter().map(|layer| empty_metadata(layer)).collect();
        let outcomes: super::family::Outcomes = (0..layers.len()).map(|_| Vec::new()).collect();
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
    pub(in crate::layers::sdf::collision_system) fn step(
        &mut self,
        ctx: SliceStepContext<'_>,
        mut state: SlicePlacementState<'_>,
        mut should_yield: impl FnMut() -> bool,
    ) -> bool {
        if self.cursor >= self.ordered.len() {
            return true;
        }
        let frame = build_layer_frame(&ctx);
        let mut processed_in_step = 0_usize;
        while self.cursor < self.ordered.len() {
            if processed_in_step > 0
                && processed_in_step.is_multiple_of(FEATURE_BATCH_CHECK)
                && should_yield()
            {
                return false;
            }
            let Some(&(position, feature_index, was_visible)) = self.ordered.get(self.cursor)
            else {
                break;
            };
            self.cursor += 1;
            processed_in_step += 1;

            let Some(layer) = ctx.layers.get(position).copied().flatten() else {
                continue;
            };
            self.place_fallback_chain(
                &frame,
                position,
                layer,
                feature_index,
                was_visible,
                &mut state,
            );
        }
        true
    }

    fn place_fallback_chain(
        &mut self,
        frame: &LayerFrame<'_>,
        position: usize,
        layer: &crate::sdf::SymbolLayerData,
        feature_index: usize,
        was_visible: bool,
        state: &mut SlicePlacementState<'_>,
    ) {
        let zoom = frame.view_state.style_zoom().value();
        let Some(initial_feature) = layer.features.get(feature_index) else {
            return;
        };
        let mut next = Some((feature_index, initial_feature, was_visible, false));
        while let Some((index, feature, was_visible, suppressed)) = next.take() {
            let feat_frame = LayerFrame {
                uniforms: if frame.has_data_driven_sizes {
                    frame
                        .uniforms
                        .with_feature_sizes(frame.paint, &feature.data.properties, zoom)
                } else {
                    frame.uniforms
                },
                ..*frame
            };
            let outcome = feat_frame.place_feature(
                (layer, feature, index, was_visible, suppressed),
                (&mut *state.boxes, &mut *state.placed, &mut *state.history),
            );
            let poses = match &outcome.line {
                Some(LinePoses::Poses(poses)) => Some(poses.clone()),
                _ => None,
            };
            self.record_feature_outcome(
                FeatureOutcomeRecord {
                    position,
                    index,
                    layer,
                    feature,
                    zoom,
                    outcome: &outcome,
                    poses,
                    paint: frame.paint,
                },
                state.scratch,
            );
            next = layer
                .features
                .get(index + 1)
                .filter(|other| other.fallback)
                .map(|other| {
                    (
                        index + 1,
                        other,
                        state.history.was_visible(layer, other),
                        suppressed || outcome.placed,
                    )
                });
        }
    }

    fn record_feature_outcome(
        &mut self,
        record: FeatureOutcomeRecord<'_>,
        scratch: &mut super::super::upload_diff::MetadataScratchPool,
    ) {
        let FeatureOutcomeRecord {
            position,
            index,
            layer,
            feature,
            zoom,
            outcome,
            poses,
            paint,
        } = record;
        if let Some(meta) = self.metadata.get_mut(position) {
            if (!outcome.ramps[0].is_hidden() || !outcome.ramps[1].is_hidden())
                && meta.capacity() == 0
            {
                *meta = scratch.acquire();
            }
            write_feature_metadata(
                layer,
                feature,
                (outcome.ramps, outcome.text_shift, outcome.anchor),
                outcome.ground,
                poses.as_deref(),
                (paint, zoom, meta),
            );
        }
        if !outcome.ramps[0].is_hidden() || !outcome.ramps[1].is_hidden() {
            if let Some(outcomes) = self.outcomes.get_mut(position) {
                if outcomes.is_empty() {
                    outcomes.resize_with(layer.features.len(), || None);
                }
                if let Some(slot) = outcomes.get_mut(index) {
                    *slot = Some(super::family::Outcome {
                        ramps: outcome.ramps,
                        text_shift: outcome.text_shift,
                        anchor: outcome.anchor,
                        ground: outcome.ground,
                        poses,
                    });
                }
            }
        }
    }

    pub(in crate::layers::sdf::collision_system) fn recycle(
        self,
        scratch: &mut super::super::upload_diff::MetadataScratchPool,
    ) {
        for buf in self.metadata {
            scratch.recycle(buf);
        }
    }
}

struct FeatureOutcomeRecord<'a> {
    position: usize,
    index: usize,
    layer: &'a crate::sdf::SymbolLayerData,
    feature: &'a crate::sdf::Feature,
    zoom: f64,
    outcome: &'a super::FeaturePlacement,
    poses: Option<Vec<crate::sdf::line_glyphs::GlyphPose>>,
    paint: &'a crate::style::layer::SymbolPaint,
}

fn build_layer_frame<'a>(ctx: &'a SliceStepContext<'a>) -> LayerFrame<'a> {
    let zoom = ctx.view_state.style_zoom().value();
    let has_data_driven_sizes = ctx
        .paint
        .text_size
        .as_ref()
        .is_some_and(|s| !s.is_feature_constant())
        || ctx
            .paint
            .compiled_property::<f32>("icon-size")
            .is_some_and(|s| !s.is_feature_constant());
    let has_terrain_or_height = ctx
        .world
        .resources
        .get::<crate::terrain::coverage::TerrainCoverageIndex>()
        .is_some()
        || ctx.paint.uses_shared_height();
    let repeat_distance = f64::from(ctx.paint.number(
        "symbol-spacing",
        &crate::style::expression::FeatureProperties::default(),
        zoom,
        super::super::temporal::repeat_spacing::MIN_REPEAT_DISTANCE as f32,
    ))
    .clamp(
        1.0,
        super::super::temporal::repeat_spacing::MIN_REPEAT_DISTANCE,
    );
    LayerFrame {
        world: ctx.world,
        view_state: ctx.view_state,
        projection: ctx.projection,
        paint: ctx.paint,
        uniforms: SymbolUniforms::new(ctx.paint, zoom, [1, 1]),
        zoom_limits: ctx.zoom_limits,
        glyphs_stay_upright: crate::sdf::paint::glyphs_stay_upright(ctx.paint, zoom),
        has_data_driven_sizes,
        has_terrain_or_height,
        has_icon_text_fit: ctx.paint.properties.contains_key("icon-text-fit"),
        constant_rules: rules::PlacementRules::constant_for_layer(ctx.paint, zoom),
        visibility_policy: ctx
            .world
            .resources
            .get::<crate::sdf::visibility::SymbolVisibility>(),
        repeat_distance,
    }
}
