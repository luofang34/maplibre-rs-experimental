//! Schedule plugin wiring [`BinnedRenderPhase`] into `Prepare`, `PhaseSort`, and `Cleanup`.

use std::{marker::PhantomData, rc::Rc};

use crate::{
    context::MapContext,
    environment::Environment,
    kernel::Kernel,
    plugin::{Plugin, PluginError},
    render::{
        eye_covering::{EyeInFrame, EyePassCount},
        render_phase::{
            batch_and_prepare_binned_render_phase_cpu, BatchedInstanceBuffer, BinnedPhaseItem,
            BinnedRenderPhase, DrawFunctions, FallbackInstanceBufferMode, GetBatchData,
            IndirectParametersBuffers, DEFAULT_MAX_INACTIVE_GENERATIONS,
        },
        tracked_pass::DrawCapabilities,
        Renderer,
    },
    schedule::{
        system::{SystemError, SystemResult},
        RenderStageLabel, Schedule,
    },
    tcs::world::World,
};

/// Scheduled `PhaseSort` system that batches instances on the CPU and uploads instance and
/// indirect parameter buffers to the GPU via [`UploadQueue`](crate::render::upload_queue::UploadQueue).
pub fn batch_and_prepare_binned_render_phase<BPI, GBD>(
    MapContext {
        world, renderer, ..
    }: &mut MapContext,
) -> SystemResult
where
    BPI: BinnedPhaseItem,
    GBD: GetBatchData<BPI>,
{
    if !world.resources.exists::<BinnedRenderPhase<BPI>>() {
        return Err(SystemError::Dependencies);
    }
    let capabilities = world
        .resources
        .get::<DrawCapabilities>()
        .copied()
        .unwrap_or_else(|| {
            DrawCapabilities::from_adapter_and_device(&renderer.adapter, &renderer.device)
        });
    let min_uniform_alignment = renderer.device.limits().min_uniform_buffer_offset_alignment;

    batch_and_prepare_binned_render_phase_cpu::<BPI, GBD>(
        world,
        capabilities,
        min_uniform_alignment,
    );

    if let Some(instance_buffer) = world
        .resources
        .get_mut::<BatchedInstanceBuffer<GBD::BufferData>>()
    {
        instance_buffer.upload(&renderer.device, &renderer.queue, capabilities);
    }
    if let Some(indirect_buffers) = world.resources.get_mut::<IndirectParametersBuffers>() {
        indirect_buffers.upload(&renderer.device, &renderer.queue, capabilities);
    }
    Ok(())
}

/// Installs [`BinnedRenderPhase<BPI>`], [`DrawFunctions<BPI>`], [`BatchedInstanceBuffer<GBD::BufferData>`],
/// shared [`IndirectParametersBuffers`], and their `Prepare` / `PhaseSort` / `Cleanup` systems.
///
/// Install after [`RenderPlugin`](crate::RenderPlugin) so the standard render stages exist.
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
        if !world.resources.exists::<BinnedRenderPhase<BPI>>() {
            world
                .resources
                .insert(BinnedRenderPhase::<BPI>::with_config(
                    self.fallback_mode,
                    self.max_inactive_generations,
                ));
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

pub(crate) fn prepare_binned_phase_world<BPI, GBD>(world: &mut World) -> SystemResult
where
    BPI: BinnedPhaseItem,
    GBD: GetBatchData<BPI>,
{
    let eye = world.resources.get::<EyeInFrame>().copied();
    let eye_count = world.resources.get::<EyePassCount>().map_or(1, |c| c.0);
    let Some(phase) = world.resources.get_mut::<BinnedRenderPhase<BPI>>() else {
        return Err(SystemError::Dependencies);
    };
    phase.begin_frame(eye, eye_count);

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

pub(crate) fn cleanup_binned_phase_world<BPI>(world: &mut World) -> SystemResult
where
    BPI: BinnedPhaseItem,
{
    let eye = world.resources.get::<EyeInFrame>().copied();
    let eye_count = world.resources.get::<EyePassCount>().map_or(1, |c| c.0);
    let Some(phase) = world.resources.get_mut::<BinnedRenderPhase<BPI>>() else {
        return Err(SystemError::Dependencies);
    };

    phase.end_frame(eye, eye_count);
    Ok(())
}
