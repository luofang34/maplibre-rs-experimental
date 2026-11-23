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
    prepend_count: usize,
}

impl SystemStage {
    /// Appends a system and returns the stage for builder-style construction.
    #[must_use]
    pub fn with_system(mut self, system: impl IntoSystemContainer) -> Self {
        self.add_system(system);
        self
    }

    /// Inserts a system into the stage's leading group, preserving registration order among
    /// early systems so worker completions drain before new requests are dispatched.
    pub fn add_early_system(&mut self, system: impl IntoSystemContainer) -> &mut Self {
        let index = self.prepend_count.min(self.systems.len());
        self.systems.insert(index, system.into_container());
        self.prepend_count += 1;
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
        let measure_systems = cfg!(test)
            || tracing::enabled!(tracing::Level::INFO)
            || context
                .world
                .resources
                .get::<super::heap::HeapSampler>()
                .is_some();
        for container in &mut self.systems {
            #[cfg(feature = "trace")]
            let _span =
                tracing::info_span!("system", name = container.system.name().as_ref()).entered();
            let system = &mut container.system;
            if measure_systems {
                let name = system.name();
                timings::measure(context, &name, |context| system.run(context)).0?;
            } else {
                system.run(context)?;
            }
        }
        Ok(())
    }
}
