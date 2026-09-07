use crate::{
    context::MapContext,
    schedule::{Stage, StageResult},
    tcs::system::{timings::FrameTimings, IntoSystemContainer, SystemContainer},
};

#[derive(Default)]
pub struct SystemStage {
    systems: Vec<SystemContainer>,
}

impl SystemStage {
    #[must_use]
    pub fn with_system(mut self, system: impl IntoSystemContainer) -> Self {
        self.add_system(system);
        self
    }

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
            container.system.run(context)?;
            let spent = started.elapsed();
            context
                .world
                .resources
                .get_or_init_mut::<FrameTimings>()
                .record(container.system.name(), spent);
        }
        Ok(())
    }
}
