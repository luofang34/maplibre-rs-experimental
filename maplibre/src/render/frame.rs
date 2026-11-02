//! One frame of the schedule, inside the renderer's per-frame scope.

use crate::{
    context::MapContext,
    schedule::{Schedule, StageLabel, StageResult},
    tcs::system::timings::FrameTimings,
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
    let _pipeline_scope =
        crate::render::resource::SharedPipelines::enter(context.renderer.shared_pipelines.clone());
    schedule.run_stages(context, include)
}

/// Runs the stages `include` selects as one frame: pipelines compile into the renderer's
/// shared cache, and the frame's signals and timings are finished even when a stage fails.
pub(crate) fn run_stages(
    schedule: &mut Schedule,
    context: &mut MapContext,
    include: impl Fn(&dyn StageLabel) -> bool,
) -> StageResult {
    let result = run_stages_only(schedule, context, include);
    crate::render::frame_signals::finish_frame(context);
    context
        .world
        .resources
        .get_or_init_mut::<FrameTimings>()
        .end_frame();
    result
}
