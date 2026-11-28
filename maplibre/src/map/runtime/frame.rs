//! Frame execution loop and per-frame host/view/timings orchestration.

use crate::{
    context::MapContext,
    projection::ProjectionType,
    render::{
        eye_covering,
        frame_input::{apply_frame_input, FrameInput, ViewSource},
        frame_signals, frame_trace,
        memory_budget::FrameUploads,
        resolved_view::ResolvedFrameView,
        tracked_pass::RenderStats,
    },
    schedule::{
        system::{timings::FrameTimings, SystemResult},
        Schedule, StageLabel, StageResult,
    },
};

/// Runs every stage of `schedule` as one frame inside the shared pipeline cache, closing its
/// signals and timings.
pub fn run_frame(schedule: &mut Schedule, context: &mut MapContext) -> StageResult {
    run_stages(schedule, context, |_| true)
}

/// Runs the stages `include` selects inside the shared pipeline scope without closing the
/// frame's signals or timings, for composite multi-view bookend passes.
pub(crate) fn run_stages_only(
    schedule: &mut Schedule,
    context: &mut MapContext,
    include: impl Fn(&dyn StageLabel) -> bool,
) -> StageResult {
    let _pipeline_scope = crate::render::resource::PipelineCacheScope::enter(
        context.renderer.shared_pipelines.clone(),
    );
    let result = schedule.run_stages(context, include);
    if let Some(cache) = &context.renderer.shared_pipelines {
        cache.clear_transient_shaders();
    }
    result
}

/// Runs the stages `include` selects as one frame: pipelines compile into the renderer's
/// shared cache, and the frame's signals and timings are finished even when a stage fails.
pub(crate) fn run_stages(
    schedule: &mut Schedule,
    context: &mut MapContext,
    include: impl Fn(&dyn StageLabel) -> bool,
) -> StageResult {
    let result = run_stages_only(schedule, context, include);
    finish_frame(context);
    #[cfg(not(target_arch = "wasm32"))]
    if let Err(error) = context.renderer.device.poll(wgpu::PollType::Poll) {
        tracing::debug!(%error, "device poll failed");
    }
    result
}

/// Closes the frame: its uploads and draws are counted, its statistics become the last frame's,
/// and the camera it drew is remembered.
pub(crate) fn finish_frame(context: &mut MapContext) {
    let upload_bytes = context.renderer.queue.take_written_bytes();
    let stages = context
        .world
        .resources
        .get_or_init_mut::<FrameTimings>()
        .finish_frame();
    let draws = context
        .world
        .resources
        .get::<RenderStats>()
        .map_or(0, |stats| stats.take_frame_draws());
    frame_trace::record_finished_frame(
        &mut context.world,
        &context.view_state,
        upload_bytes,
        stages,
        draws,
    );
}

/// Applies the [`FrameInput`] a host wrote; a host that writes none keeps the map view.
///
/// A rejected external view is logged and retains the last valid external view, so a
/// tracking failure cannot switch projection or start an unrelated tile covering.
pub fn frame_input_system(
    MapContext {
        world,
        view_state,
        style,
        ..
    }: &mut MapContext,
) -> SystemResult {
    eye_covering::snapshot_lod_history(world);
    let reuses_content = eye_covering::EyeInFrame::reuses_content(world);
    let Some(input) = world.resources.get::<FrameInput>().copied() else {
        if !reuses_content {
            if let Some(uploads) = world.resources.get_mut::<FrameUploads>() {
                uploads.begin_frame(false);
            }
        }
        return Ok(());
    };
    let projection = style
        .projection
        .as_ref()
        .map_or_else(ProjectionType::default, |specification| {
            specification.projection_type.clone()
        });
    if let Err(error) = apply_frame_input(&input, view_state, &projection) {
        tracing::error!(%error, "host view rejected; preserving the last valid view");
    }
    if !reuses_content {
        let moving = !input.timestamp.is_zero()
            && matches!(input.view, ViewSource::MapView)
            && frame_signals::camera_moved(world, view_state);
        world
            .resources
            .get_or_init_mut::<FrameUploads>()
            .begin_frame(moving);
    }
    Ok(())
}

/// Publishes or refreshes [`ResolvedFrameView`] in `world.resources` when camera or projection
/// inputs have changed.
pub fn resolve_frame_view_system(
    MapContext {
        world,
        style,
        view_state,
        ..
    }: &mut MapContext,
) -> SystemResult {
    if world
        .resources
        .get::<ResolvedFrameView>()
        .is_some_and(|cached| cached.matches(style, view_state))
    {
        return Ok(());
    }
    match ResolvedFrameView::resolve(style, view_state) {
        Ok(resolved) => {
            world.resources.insert(resolved);
        }
        Err(_) => {
            world.resources.remove::<ResolvedFrameView>();
        }
    }
    Ok(())
}
