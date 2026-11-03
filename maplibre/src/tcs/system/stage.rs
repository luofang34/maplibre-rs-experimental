//! Ordered execution of map systems, with timing and heap-growth measurements.

use crate::{
    context::MapContext,
    schedule::{Stage, StageResult},
    tcs::system::{timings, IntoSystemContainer, SystemContainer},
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
            let name = container.system.name();
            let system = &mut container.system;
            timings::measure(context, &name, |context| system.run(context)).0?;
        }
        Ok(())
    }
}
