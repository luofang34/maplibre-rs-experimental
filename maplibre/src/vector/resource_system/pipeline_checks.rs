use super::{
    extrusion_setup,
    prepared_state::{ExtrusionDetectionKey, VectorPreparedState},
    setup,
};
use crate::{
    render::eventually::{Eventually, Eventually::Initialized},
    tcs::world::World,
    vector::{CirclePipeline, ExtrusionPipeline, LinePipeline, VectorBufferPool, VectorPipeline},
};

#[derive(Clone, Copy)]
pub(super) struct PreparedFlags {
    pub(super) update_dashes: bool,
    pub(super) update_patterns: bool,
    pub(super) needs_fill: bool,
    pub(super) needs_fill_opaque: bool,
    pub(super) needs_fill_blended: bool,
    pub(super) needs_flat_lines: bool,
    pub(super) needs_terrain_lines: bool,
    pub(super) needs_elevated_lines: bool,
    pub(super) needs_circle: bool,
    pub(super) needs_circle_feature_stroke: bool,
    pub(super) needs_circle_dynamic_radius: bool,
    pub(super) needs_extrusion: bool,
    pub(super) needs_extrusion_opaque: bool,
    pub(super) needs_extrusion_pattern: bool,
    pub(super) needs_extrusion_translucent: bool,
    pub(super) has_pattern_paint: bool,
    pub(super) has_dash_or_pattern_line: bool,
    pub(super) extrusion_vis_mask: u64,
    pub(super) cached_extrusion: Option<(ExtrusionDetectionKey, (bool, bool))>,
}

pub(super) struct PipelineNeedsCheck {
    pub(super) need_vector_pipeline: bool,
    pub(super) need_line_pipeline: bool,
    pub(super) need_circle_pipeline: bool,
    pub(super) need_extrusion_pipeline: bool,
    pub(super) needs_pattern: bool,
    pub(super) needs_pattern_resources: bool,
    pub(super) extrusion_needs: extrusion_setup::ExtrusionPipelineNeeds,
    pub(super) setup_needs: setup::VectorPipelineNeeds,
}

pub(super) fn collect_prepared_flags(
    world: &mut World,
    style: &crate::style::Style,
    style_zoom: f64,
    uses_globe: bool,
    terrain_active: bool,
) -> PreparedFlags {
    let prepared = world.resources.get_or_init_mut::<VectorPreparedState>();
    prepared.sync_style(style, terrain_active);
    let update_dashes = prepared.should_update_dashes(style);
    let update_patterns = prepared.should_update_patterns(style);
    let needs_circle = prepared.needs_circle;
    let needs_extrusion = prepared.needs_extrusion_at_zoom(style_zoom, uses_globe);
    PreparedFlags {
        update_dashes,
        update_patterns,
        needs_fill: prepared.needs_fill,
        needs_fill_opaque: prepared.needs_fill_opaque,
        needs_fill_blended: prepared.needs_fill_blended,
        needs_flat_lines: prepared.needs_flat_lines,
        needs_terrain_lines: prepared.needs_terrain_lines,
        needs_elevated_lines: prepared.needs_elevated_lines,
        needs_circle,
        needs_circle_feature_stroke: needs_circle && prepared.needs_circle_feature_stroke,
        needs_circle_dynamic_radius: needs_circle && prepared.needs_circle_dynamic_radius,
        needs_extrusion,
        needs_extrusion_opaque: needs_extrusion && prepared.needs_extrusion_opaque,
        needs_extrusion_pattern: needs_extrusion && prepared.needs_extrusion_pattern,
        needs_extrusion_translucent: needs_extrusion && prepared.needs_extrusion_translucent,
        has_pattern_paint: prepared.has_pattern_paint,
        has_dash_or_pattern_line: prepared.has_dash_or_pattern_line,
        extrusion_vis_mask: prepared.extrusion_visibility_mask(style_zoom),
        cached_extrusion: prepared.cached_extrusion,
    }
}

fn is_extrusion_complete(world: &World, flags: &PreparedFlags, uses_globe: bool) -> bool {
    flags.needs_extrusion
        && matches!(
            world.resources.get::<Eventually<ExtrusionPipeline>>(),
            Some(Initialized(ep))
                if (!flags.needs_extrusion_opaque || ep.clear_mark.is_some())
                    && ep.for_globe(uses_globe).is_some_and(|mode| {
                        (!flags.needs_extrusion_opaque
                            || (mode.depth.is_some()
                                && mode.color.is_some()
                                && mode.compact_depth.is_some()
                                && mode.compact_color.is_some()))
                            && (!flags.needs_extrusion_pattern || mode.pattern.is_some())
                            && (!flags.needs_extrusion_translucent
                                || (mode.translucent_back.is_some()
                                    && mode.translucent_front.is_some()
                                    && mode.compact_translucent_back.is_some()
                                    && mode.compact_translucent_front.is_some()))
                    })
        )
}

pub(super) fn resolve_extrusion_needs(
    world: &mut World,
    style_rev: u64,
    style_zoom: f64,
    terrain_needed: bool,
    uses_globe: bool,
    flags: &PreparedFlags,
) -> extrusion_setup::ExtrusionPipelineNeeds {
    let complete = is_extrusion_complete(world, flags, uses_globe);
    let (has_compact, has_standard) = if flags.needs_extrusion && !complete {
        let (pool_rev, pool_eviction_gen) =
            match world.resources.get::<Eventually<VectorBufferPool>>() {
                Some(Initialized(pool)) => (pool.revision(), pool.eviction_generation()),
                _ => (0, 0),
            };
        let key = ExtrusionDetectionKey {
            style_rev,
            arrival_epoch: crate::vector::arrival_epoch(world),
            pool_rev,
            pool_eviction_gen,
            terrain_needed,
            vis_mask: flags.extrusion_vis_mask,
        };
        if let Some((cached_key, encodings)) = flags.cached_extrusion
            && cached_key == key
        {
            encodings
        } else if let Some(prepared) = world.resources.get::<VectorPreparedState>() {
            let encodings =
                super::detect_extrusion_encodings(world, prepared, style_zoom, terrain_needed);
            if let Some(state_mut) = world.resources.get_mut::<VectorPreparedState>() {
                state_mut.cached_extrusion = Some((key, encodings));
            }
            encodings
        } else {
            (false, false)
        }
    } else {
        (false, false)
    };
    extrusion_setup::ExtrusionPipelineNeeds {
        globe: uses_globe,
        standard_opaque: flags.needs_extrusion_opaque && has_standard,
        compact_opaque: flags.needs_extrusion_opaque && has_compact,
        pattern: flags.needs_extrusion_opaque && flags.needs_extrusion_pattern && has_standard,
        standard_translucent: flags.needs_extrusion_translucent && has_standard,
        compact_translucent: flags.needs_extrusion_translucent && has_compact,
        clear_mark: flags.needs_extrusion_opaque && (has_compact || has_standard),
    }
}

fn check_extrusion_pipeline_needed(
    world: &World,
    uses_globe: bool,
    flags: &PreparedFlags,
    extrusion_needs: extrusion_setup::ExtrusionPipelineNeeds,
) -> bool {
    let has_ext = extrusion_needs.standard_opaque
        || extrusion_needs.compact_opaque
        || extrusion_needs.pattern
        || extrusion_needs.standard_translucent
        || extrusion_needs.compact_translucent;
    flags.needs_extrusion
        && (extrusion_needs.clear_mark || has_ext)
        && !matches!(
            world.resources.get::<Eventually<ExtrusionPipeline>>(),
            Some(Initialized(ep)) if
                (!extrusion_needs.clear_mark || ep.clear_mark.is_some())
                && (!has_ext
                    || ep.for_globe(uses_globe).is_some_and(|mode| {
                        (!extrusion_needs.standard_opaque
                            || (mode.depth.is_some() && mode.color.is_some()))
                            && (!extrusion_needs.compact_opaque
                                || (mode.compact_depth.is_some() && mode.compact_color.is_some()))
                            && (!extrusion_needs.pattern || mode.pattern.is_some())
                            && (!extrusion_needs.standard_translucent
                                || (mode.translucent_back.is_some()
                                    && mode.translucent_front.is_some()))
                            && (!extrusion_needs.compact_translucent
                                || (mode.compact_translucent_back.is_some()
                                    && mode.compact_translucent_front.is_some()))
                    }))
        )
}

fn check_pattern_needs(
    world: &World,
    style: &crate::style::Style,
    style_zoom: f64,
    has_pattern_paint: bool,
) -> (bool, bool) {
    let pattern_res = world
        .resources
        .get::<super::super::pattern::PatternResources>();
    let missing_resources = pattern_res.is_none();
    let missing_pipeline = pattern_res.is_none_or(|patterns| patterns.pipeline().is_none());
    let needs_pattern = missing_pipeline
        && has_pattern_paint
        && style.layers.iter().any(|layer| {
            layer.paint.as_ref().is_some_and(|paint| {
                super::super::pattern::pattern_name(paint, style_zoom).is_some()
                    || super::super::pattern::per_feature_pattern(paint).is_some()
            })
        });
    (needs_pattern, missing_resources && has_pattern_paint)
}

fn vector_pipeline_needed(
    world: &World,
    flags: &PreparedFlags,
    setup_needs: &setup::VectorPipelineNeeds,
) -> bool {
    flags.needs_fill
        && !matches!(
            world.resources.get::<Eventually<VectorPipeline>>(),
            Some(Initialized(vp)) if
                (!setup_needs.flat_fill_opaque || vp.flat_opaque.is_some())
                && (!setup_needs.flat_fill_blended || vp.flat_blended.is_some())
                && (!setup_needs.globe_fill_opaque || vp.globe_opaque.is_some())
                && (!setup_needs.globe_fill_blended || vp.globe_blended.is_some())
        )
}

fn line_pipeline_needed(
    world: &World,
    needs_any_lines: bool,
    setup_needs: &setup::VectorPipelineNeeds,
) -> bool {
    needs_any_lines
        && !matches!(
            world.resources.get::<Eventually<LinePipeline>>(),
            Some(Initialized(lp)) if
                (!setup_needs.plain_flat_lines || lp.flat_plain.is_some())
                && (!setup_needs.plain_globe_lines || lp.globe_plain.is_some())
                && (!setup_needs.flat_lines || lp.flat.is_some())
                && (!setup_needs.terrain_lines || (lp.structure.is_some() && lp.terrain.is_some()))
                && (!setup_needs.elevated_lines || lp.elevated.is_some())
        )
}

pub(super) fn check_pipeline_needs(
    world: &World,
    style: &crate::style::Style,
    style_zoom: f64,
    uses_globe: bool,
    terrain_needed: bool,
    flags: &PreparedFlags,
    extrusion_needs: extrusion_setup::ExtrusionPipelineNeeds,
) -> PipelineNeedsCheck {
    let flat_or_terrain = !uses_globe || terrain_needed;
    let setup_needs = setup::VectorPipelineNeeds {
        flat_fill_opaque: flags.needs_fill && flags.needs_fill_opaque && flat_or_terrain,
        flat_fill_blended: flags.needs_fill && flags.needs_fill_blended && flat_or_terrain,
        globe_fill_opaque: flags.needs_fill && flags.needs_fill_opaque && uses_globe,
        globe_fill_blended: flags.needs_fill && flags.needs_fill_blended && uses_globe,
        plain_flat_lines: flags.needs_flat_lines && flat_or_terrain,
        plain_globe_lines: flags.needs_flat_lines && uses_globe,
        flat_lines: flags.needs_flat_lines && flags.has_dash_or_pattern_line,
        terrain_lines: flags.needs_terrain_lines,
        elevated_lines: flags.needs_elevated_lines,
        circle: flags.needs_circle,
        circle_feature_stroke: flags.needs_circle_feature_stroke,
        circle_dynamic_radius: flags.needs_circle_dynamic_radius,
    };
    let needs_any_lines =
        flags.needs_flat_lines || flags.needs_terrain_lines || flags.needs_elevated_lines;
    let need_vector_pipeline = vector_pipeline_needed(world, flags, &setup_needs);
    let need_line_pipeline = line_pipeline_needed(world, needs_any_lines, &setup_needs);
    let need_circle_pipeline = flags.needs_circle
        && !matches!(
            world.resources.get::<Eventually<CirclePipeline>>(),
            Some(Initialized(cp)) if
                (!flags.needs_circle_feature_stroke || cp.feature_stroke.is_some())
                && (!flags.needs_circle_dynamic_radius || cp.dynamic_radius.is_some())
        );
    let need_extrusion_pipeline =
        check_extrusion_pipeline_needed(world, uses_globe, flags, extrusion_needs);
    let (needs_pattern, needs_pattern_resources) =
        check_pattern_needs(world, style, style_zoom, flags.has_pattern_paint);

    PipelineNeedsCheck {
        need_vector_pipeline,
        need_line_pipeline,
        need_circle_pipeline,
        need_extrusion_pipeline,
        needs_pattern,
        needs_pattern_resources,
        extrusion_needs,
        setup_needs,
    }
}
