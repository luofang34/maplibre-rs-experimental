//! Ordered frame stages, with execution stopping at the first failed system.

#![deny(missing_docs)]

use std::borrow::Cow;
use std::collections::HashMap;

use downcast_rs::{impl_downcast, Downcast};
use thiserror::Error;

use crate::{
    context::MapContext,
    define_label,
    tcs::system::{heap::live_bytes, timings::FrameTimings},
    tcs::system::{stage::SystemStage, IntoSystemContainer, SystemError},
};

/// A stage that succeeds without reading or changing the frame context.
pub struct NopStage;

impl Stage for NopStage {
    fn run(&mut self, _context: &mut MapContext) -> StageResult {
        Ok(())
    }
}

/// Defines a sequence of default-constructible stages, run in declaration order.
/// The first error stops execution and is returned without rolling back earlier stages.
///
/// ```
/// #![deny(missing_docs)]
/// //! An application frame composed from two schedules.
/// maplibre::multi_stage!(FrameStages,
///     first: maplibre::schedule::Schedule,
///     second: maplibre::schedule::Schedule
/// );
/// # fn main() { let _stages = FrameStages::default(); }
/// ```
#[macro_export]
macro_rules! multi_stage {
    ($multi_stage:ident, $($stage:ident: $stage_ty:ty),*) => {
        /// Runs its stages in declaration order and returns the first error.
        pub struct $multi_stage {
            $($stage: $stage_ty),*
        }

        impl $crate::schedule::Stage for $multi_stage {
            fn run(
                &mut self,
                context: &mut $crate::context::MapContext,
            ) -> $crate::schedule::StageResult {
                $($crate::schedule::Stage::run(&mut self.$stage, context)?;)*
                Ok(())
            }
        }

        impl ::core::default::Default for $multi_stage {
            fn default() -> Self {
                $multi_stage {
                     $($stage: <$stage_ty as ::core::default::Default>::default()),*
                }
            }
        }
    };
}

/// Runs a fixed-size array of stages in order, stopping at the first error.
pub struct MultiStage<const I: usize, S>
where
    S: Stage,
{
    stages: [S; I],
}

impl<const I: usize, S> MultiStage<I, S>
where
    S: Stage,
{
    /// Takes ownership of stages in their execution order.
    pub fn new(stages: [S; I]) -> Self {
        Self { stages }
    }
}

impl<const I: usize, S> Stage for MultiStage<I, S>
where
    S: Stage,
{
    fn run(&mut self, context: &mut MapContext) -> StageResult {
        for stage in self.stages.iter_mut() {
            stage.run(context)?
        }

        Ok(())
    }
}

define_label!(StageLabel);
pub(crate) type BoxedStageLabel = Box<dyn StageLabel>;

/// Failure that stops execution of a stage or its containing schedule.
#[derive(Error, Debug)]
pub enum StageError {
    /// A scheduled system failed; its typed cause is preserved.
    #[error("system errored")]
    System(#[from] SystemError),
}

/// Completion status of a stage; an error prevents subsequent stages from running.
pub type StageResult = Result<(), StageError>;

/// A mutable unit of frame work that can be stored and downcast inside a schedule.
pub trait Stage: Downcast {
    /// Runs the stage; this happens once per update.
    /// Implementors must initialize all of their state before running the first time.
    fn run(&mut self, context: &mut MapContext) -> StageResult;
}

impl_downcast!(Stage);

/// Owns labeled stages and runs them in insertion order against one mutable frame context.
/// A stage error stops that run without rolling back earlier stages. Schedules can be nested
/// because they also implement [`Stage`].
#[derive(Default)]
pub struct Schedule {
    stages: HashMap<BoxedStageLabel, Box<dyn Stage>>,
    stage_order: Vec<BoxedStageLabel>,
}

impl Schedule {
    /// Adds the given `stage` at the last position of the schedule.
    ///
    /// # Example
    ///
    /// ```
    /// # use maplibre::schedule::{Schedule, NopStage};
    /// #
    /// # let mut schedule = Schedule::default();
    /// schedule.add_stage("my_stage", NopStage);
    /// ```
    ///
    /// # Panics
    /// Panics if the label is already registered.
    pub fn add_stage<S: Stage>(&mut self, label: impl StageLabel, stage: S) -> &mut Self {
        let label: Box<dyn StageLabel> = Box::new(label);
        self.stage_order.push(label.clone());
        let prev = self.stages.insert(label.clone(), Box::new(stage));
        assert!(prev.is_none(), "Stage already exists: {label:?}.");
        self
    }

    /// Drops the stage with `label` and removes it from the execution order.
    ///
    /// # Panics
    /// Panics if the label is absent.
    pub fn remove_stage(&mut self, label: impl StageLabel) -> &mut Self {
        let remove: Box<dyn StageLabel> = Box::new(label);
        self.stages.remove(&remove).expect("stage not found");
        self.stage_order.retain(|label| label != &remove);
        self
    }

    /// Adds the given `stage` immediately after the `target` stage.
    ///
    /// # Example
    ///
    /// ```
    /// # use maplibre::schedule::{Schedule, NopStage};
    /// #
    /// # let mut schedule = Schedule::default();
    /// # schedule.add_stage("target_stage", NopStage);
    /// schedule.add_stage_after("target_stage", "my_stage", NopStage);
    /// ```
    ///
    /// # Panics
    /// Panics if the target is absent or the new label is already registered.
    pub fn add_stage_after<S: Stage>(
        &mut self,
        target: impl StageLabel,
        label: impl StageLabel,
        stage: S,
    ) -> &mut Self {
        let label: Box<dyn StageLabel> = Box::new(label);
        let target = &target as &dyn StageLabel;
        let target_index = self
            .stage_order
            .iter()
            .enumerate()
            .find(|(_i, stage_label)| &***stage_label == target)
            .map(|(i, _)| i)
            .unwrap_or_else(|| panic!("Target stage does not exist: {target:?}."));

        self.stage_order.insert(target_index + 1, label.clone());
        let prev = self.stages.insert(label.clone(), Box::new(stage));
        assert!(prev.is_none(), "Stage already exists: {label:?}.");
        self
    }

    /// Adds the given `stage` immediately before the `target` stage.
    ///
    /// # Example
    ///
    /// ```
    /// # use maplibre::schedule::{Schedule, NopStage};
    /// #
    /// # let mut schedule = Schedule::default();
    /// # schedule.add_stage("target_stage", NopStage);
    /// #
    /// schedule.add_stage_before("target_stage", "my_stage", NopStage);
    /// ```
    ///
    /// # Panics
    /// Panics if the target is absent or the new label is already registered.
    pub fn add_stage_before<S: Stage>(
        &mut self,
        target: impl StageLabel,
        label: impl StageLabel,
        stage: S,
    ) -> &mut Self {
        let label: Box<dyn StageLabel> = Box::new(label);
        let target = &target as &dyn StageLabel;
        let target_index = self
            .stage_order
            .iter()
            .enumerate()
            .find(|(_i, stage_label)| &***stage_label == target)
            .map(|(i, _)| i)
            .unwrap_or_else(|| panic!("Target stage does not exist: {target:?}."));

        self.stage_order.insert(target_index, label.clone());
        let prev = self.stages.insert(label.clone(), Box::new(stage));
        assert!(prev.is_none(), "Stage already exists: {label:?}.");
        self
    }

    /// Fetches the [`Stage`] of type `T` marked with `label`, then executes the provided
    /// `func` passing the fetched stage to it as an argument.
    ///
    /// The `func` argument should be a function or a closure that accepts a mutable reference
    /// to a struct implementing `Stage` and returns the same type. That means that it should
    /// also assume that the stage has already been fetched successfully.
    ///
    /// # Example
    ///
    /// ```
    /// # use maplibre::schedule::{Schedule, NopStage};
    /// # let mut schedule = Schedule::default();
    ///
    /// # schedule.add_stage("my_stage", NopStage);
    /// #
    /// schedule.stage("my_stage", |stage: &mut NopStage| {
    ///     // modify stage
    ///     stage
    /// });
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if `label` refers to a non-existing stage, or if it's not of type `T`.
    pub fn stage<T: Stage, F: FnOnce(&mut T) -> &mut T>(
        &mut self,
        label: impl StageLabel,
        func: F,
    ) -> &mut Self {
        let stage = self.get_stage_mut::<T>(&label).unwrap_or_else(move || {
            panic!("stage '{label:?}' does not exist or is the wrong type")
        });
        func(stage);
        self
    }

    /// Returns a shared reference to the stage identified by `label`, if it exists.
    ///
    /// Returns `None` if the label is absent or its stage has a different concrete type.
    ///
    /// # Example
    ///
    /// ```
    /// # use maplibre::schedule::{Schedule, NopStage};
    /// #
    /// # let mut schedule = Schedule::default();
    /// # schedule.add_stage("my_stage", NopStage);
    /// #
    /// let stage = schedule.get_stage::<NopStage>(&"my_stage").unwrap();
    /// ```
    pub fn get_stage<T: Stage>(&self, label: &dyn StageLabel) -> Option<&T> {
        self.stages
            .get(label)
            .and_then(|stage| stage.downcast_ref::<T>())
    }

    /// Returns a unique, mutable reference to the stage identified by `label`, if it exists.
    ///
    /// Returns `None` if the label is absent or its stage has a different concrete type.
    ///
    /// # Example
    ///
    /// ```
    /// # use maplibre::schedule::{Schedule, NopStage};
    /// #
    /// # let mut schedule = Schedule::default();
    /// # schedule.add_stage("my_stage", NopStage);
    /// #
    /// let stage = schedule.get_stage_mut::<NopStage>(&"my_stage").unwrap();
    /// ```
    pub fn get_stage_mut<T: Stage>(&mut self, label: &dyn StageLabel) -> Option<&mut T> {
        self.stages
            .get_mut(label)
            .and_then(|stage| stage.downcast_mut::<T>())
    }

    /// Executes stages in order until one fails; completed mutations remain in the context.
    pub fn run_once(&mut self, context: &mut MapContext) -> StageResult {
        self.run_stages(context, |_| true)
    }

    /// Runs the selected stages, preserving their declared order.
    pub(crate) fn run_stages(
        &mut self,
        context: &mut MapContext,
        include: impl Fn(&dyn StageLabel) -> bool,
    ) -> StageResult {
        for label in &self.stage_order {
            if !include(&**label) {
                continue;
            }
            #[cfg(feature = "trace")]
            let _stage_span = tracing::info_span!("stage", name = ?label).entered();
            let stage = self.stages.get_mut(label).unwrap(); // TODO: Remove unwrap
            let started = instant::Instant::now();
            let heap_before = live_bytes();
            stage.run(context)?;
            let spent = started.elapsed();
            let grown = live_bytes() - heap_before;
            let timings = context.world.resources.get_or_init_mut::<FrameTimings>();
            timings.record(Cow::Owned(format!("stage {label:?}")), spent);
            timings.record_growth(Cow::Owned(format!("stage {label:?}")), grown);
        }
        context
            .world
            .resources
            .get_or_init_mut::<FrameTimings>()
            .end_frame();
        Ok(())
    }

    /// Drops all stages and their labels without changing the frame context.
    pub fn clear(&mut self) {
        self.stage_order.clear();
        self.stages.clear();
    }

    /// Iterates over all of schedule's stages and their labels, in execution order.
    pub fn iter_stages(&self) -> impl Iterator<Item = (&dyn StageLabel, &dyn Stage)> {
        self.stage_order
            .iter()
            .map(move |label| (&**label, &*self.stages[label]))
    }

    /// Adds a system to the [`Stage`] identified by `stage_label`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use maplibre::context::MapContext;
    /// # use maplibre::tcs::system::stage::SystemStage;
    /// # use maplibre::schedule::{Schedule, NopStage};
    /// # use maplibre::tcs::system::SystemError;
    /// #
    /// # let mut schedule = Schedule::default();
    /// # schedule.add_stage("my_stage", SystemStage::default());
    /// # fn my_system(context: &mut MapContext) -> Result<(), SystemError> { Ok(()) }
    /// #
    /// schedule.add_system_to_stage("my_stage", my_system);
    /// ```
    ///
    /// # Panics
    /// Panics if the label is absent or names a stage other than `SystemStage`.
    pub fn add_system_to_stage(
        &mut self,
        stage_label: impl StageLabel,
        system: impl IntoSystemContainer,
    ) -> &mut Self {
        let stage = self
            .get_stage_mut::<SystemStage>(&stage_label)
            .unwrap_or_else(move || {
                panic!("Stage '{stage_label:?}' does not exist or is not a SystemStage")
            });
        stage.add_system(system);
        self
    }
}

impl Stage for Schedule {
    fn run(&mut self, context: &mut MapContext) -> StageResult {
        self.run_once(context)?;
        Ok(())
    }
}

#[cfg(all(test, feature = "headless"))]
mod tests;
