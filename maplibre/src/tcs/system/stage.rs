//! Ordered execution of map systems, with timing and heap-growth measurements.

use crate::{
    context::MapContext,
    schedule::{Stage, StageResult},
    tcs::system::{heap::live_bytes, timings::FrameTimings, IntoSystemContainer, SystemContainer},
};

/// Runs systems in insertion order, stopping at the first error without rollback.
#[derive(Default)]
pub struct SystemStage {
    systems: Vec<SystemContainer>,
}

impl SystemStage {
    /// Appends a system and returns the stage for builder-style construction.
    #[must_use]
    pub fn with_system(mut self, system: impl IntoSystemContainer) -> Self {
        self.add_system(system);
        self
    }

    /// Appends a system after every update already registered in this stage.
    pub fn add_system(&mut self, system: impl IntoSystemContainer) -> &mut Self {
        self.systems.push(system.into_container());
        self
    }
}

impl Stage for SystemStage {
    fn run(&mut self, context: &mut MapContext) -> StageResult {
        for container in &mut self.systems {
            #[cfg(feature = "trace")]
            let _span =
                tracing::info_span!("system", name = container.system.name().as_ref()).entered();
            let started = instant::Instant::now();
            let heap_before = live_bytes();
            container.system.run(context)?;
            let spent = started.elapsed();
            let grown = live_bytes() - heap_before;
            let timings = context.world.resources.get_or_init_mut::<FrameTimings>();
            timings.record(container.system.name(), spent);
            timings.record_growth(container.system.name(), grown);
        }
        Ok(())
    }
}
