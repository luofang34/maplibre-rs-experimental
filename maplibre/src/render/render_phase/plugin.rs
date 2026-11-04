//! Schedule plugin wiring [`BinnedRenderPhase`](super::BinnedRenderPhase) into `Prepare`, `PhaseSort`, and `Cleanup`.

use std::{marker::PhantomData, rc::Rc};

use super::{
    batching::{
        batch_and_prepare_binned_render_phase, BatchedInstanceBuffer, FallbackInstanceBufferMode,
        GetBatchData,
    },
    binned::{BinnedPhaseItem, BinnedRenderPhase, DEFAULT_MAX_INACTIVE_GENERATIONS},
    draw::DrawFunctions,
    indirect::IndirectParametersBuffers,
};
use crate::{
    context::MapContext,
    environment::Environment,
    kernel::Kernel,
    plugin::{Plugin, PluginError},
    render::{eye_covering::EyeInFrame, RenderStageLabel, Renderer},
    schedule::Schedule,
    tcs::{
        system::{SystemError, SystemResult},
        world::World,
    },
};

/// Installs [`BinnedRenderPhase<BPI>`], [`DrawFunctions<BPI>`], [`BatchedInstanceBuffer<GBD::BufferData>`],
/// shared [`IndirectParametersBuffers`], and their `Prepare` / `PhaseSort` / `Cleanup` systems.
///
/// Install after [`RenderPlugin`](crate::render::RenderPlugin) so the standard render stages exist.
pub struct BinnedRenderPhasePlugin<BPI, GBD> {
    fallback_mode: FallbackInstanceBufferMode,
    max_inactive_generations: u64,
    _marker: PhantomData<fn() -> (BPI, GBD)>,
}

impl<BPI, GBD> Default for BinnedRenderPhasePlugin<BPI, GBD> {
    fn default() -> Self {
        Self::new()
    }
}

impl<BPI, GBD> BinnedRenderPhasePlugin<BPI, GBD> {
    /// Creates a [`BinnedRenderPhasePlugin`] with [`FallbackInstanceBufferMode::DynamicUniform`]
    /// and [`DEFAULT_MAX_INACTIVE_GENERATIONS`].
    pub const fn new() -> Self {
        Self {
            fallback_mode: FallbackInstanceBufferMode::DynamicUniform,
            max_inactive_generations: DEFAULT_MAX_INACTIVE_GENERATIONS,
            _marker: PhantomData,
        }
    }

    /// Configures the fallback instance buffer strategy used on backends without vertex storage buffers.
    pub const fn with_fallback_mode(mut self, mode: FallbackInstanceBufferMode) -> Self {
        self.fallback_mode = mode;
        self
    }

    /// Configures how many inactive sweep generations empty bins remain cached before eviction.
    pub const fn with_max_inactive_generations(mut self, max_inactive_generations: u64) -> Self {
        self.max_inactive_generations = max_inactive_generations;
        self
    }

    /// Initializes the phase's resources in `world` and registers its `Prepare`, `PhaseSort`, and
    /// `Cleanup` systems onto `schedule`.
    pub fn install(&self, schedule: &mut Schedule, world: &mut World) -> Result<(), PluginError>
    where
        BPI: BinnedPhaseItem,
        GBD: GetBatchData<BPI>,
    {
        world.resources.get_or_init_mut::<DrawFunctions<BPI>>();
        {
            let phase = world.resources.get_or_init_mut::<BinnedRenderPhase<BPI>>();
            phase.set_max_inactive_generations(self.max_inactive_generations);
            phase.set_fallback_mode(self.fallback_mode);
        }
        if !world
            .resources
            .exists::<BatchedInstanceBuffer<GBD::BufferData>>()
        {
            world.resources.insert(
                BatchedInstanceBuffer::<GBD::BufferData>::new()
                    .with_fallback_mode(self.fallback_mode),
            );
        }
        world
            .resources
            .get_or_init_mut::<IndirectParametersBuffers>();

        schedule.add_system_to_stage(
            RenderStageLabel::Prepare,
            prepare_binned_render_phase_system::<BPI, GBD>,
        )?;
        schedule.add_system_to_stage(
            RenderStageLabel::PhaseSort,
            batch_and_prepare_binned_render_phase::<BPI, GBD>,
        )?;
        schedule.add_system_to_stage(
            RenderStageLabel::Cleanup,
            cleanup_binned_render_phase_system::<BPI>,
        )?;
        Ok(())
    }
}

impl<E, BPI, GBD> Plugin<E> for BinnedRenderPhasePlugin<BPI, GBD>
where
    E: Environment,
    BPI: BinnedPhaseItem,
    GBD: GetBatchData<BPI>,
{
    fn build(
        &self,
        schedule: &mut Schedule,
        _kernel: Rc<Kernel<E>>,
        world: &mut World,
        _renderer: &mut Renderer,
    ) -> Result<(), PluginError> {
        self.install(schedule, world)
    }
}

/// Resets per-pass binned phase queues, instance buffers, and indirect parameter staging at the
/// start of `Prepare`, ensuring multi-viewport slots never inherit a prior slot's queued items.
pub fn prepare_binned_render_phase_system<BPI, GBD>(
    MapContext { world, .. }: &mut MapContext,
) -> SystemResult
where
    BPI: BinnedPhaseItem,
    GBD: GetBatchData<BPI>,
{
    prepare_binned_phase_world::<BPI, GBD>(world)
}

pub(super) fn prepare_binned_phase_world<BPI, GBD>(world: &mut World) -> SystemResult
where
    BPI: BinnedPhaseItem,
    GBD: GetBatchData<BPI>,
{
    let eye = world.resources.get::<EyeInFrame>().copied();
    let Some(phase) = world.resources.get_mut::<BinnedRenderPhase<BPI>>() else {
        return Err(SystemError::Dependencies);
    };
    phase.clear_entries();
    if let Some(e) = eye.filter(|e| e.index > 0) {
        phase.active_stamp_generation = phase
            .xr_eye0_sweep
            .and_then(|(swept_frame, gen_eye0)| (swept_frame == e.frame).then_some(gen_eye0));
    } else {
        phase.xr_eye0_sweep = None;
        phase.active_stamp_generation = None;
    }

    if let Some(instance_buffer) = world
        .resources
        .get_mut::<BatchedInstanceBuffer<GBD::BufferData>>()
    {
        instance_buffer.clear();
    }
    if let Some(indirect_buffers) = world.resources.get_mut::<IndirectParametersBuffers>() {
        indirect_buffers.clear();
    }
    Ok(())
}

/// Clears current-frame binned entries and sweeps cold bins during `Cleanup` (including after
/// mid-frame stage failures via `Schedule::run_after_failure`).
pub fn cleanup_binned_render_phase_system<BPI>(
    MapContext { world, .. }: &mut MapContext,
) -> SystemResult
where
    BPI: BinnedPhaseItem,
{
    cleanup_binned_phase_world::<BPI>(world)
}

pub(super) fn cleanup_binned_phase_world<BPI>(world: &mut World) -> SystemResult
where
    BPI: BinnedPhaseItem,
{
    let eye = world.resources.get::<EyeInFrame>().copied();
    let Some(phase) = world.resources.get_mut::<BinnedRenderPhase<BPI>>() else {
        return Err(SystemError::Dependencies);
    };

    // Always empty queued entries and prepared batches immediately so failed frames or subsequent
    // viewports never re-render stale items.
    phase.clear_entries();
    phase.active_stamp_generation = None;

    if eye.is_none_or(|e| e.index == 0) {
        let swept_gen = phase.generation();
        phase.sweep();
        phase.xr_eye0_sweep = eye.map(|e| (e.frame, swept_gen));
    }
    Ok(())
}
