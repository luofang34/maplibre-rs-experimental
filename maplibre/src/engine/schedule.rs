//! Ordered frame stages, with execution stopping at the first failed system.

#![deny(missing_docs)]

use std::sync::Arc;

use downcast_rs::{impl_downcast, Downcast};
use thiserror::Error;

use crate::{
    context::MapContext,
    define_label,
    schedule::system::{stage::SystemStage, timings, IntoSystemContainer, SystemError},
};

pub mod system;

/// A stage that succeeds without reading or changing the frame context.
pub struct NopStage;

impl Stage for NopStage {
    fn run(&mut self, _context: &mut MapContext) -> StageResult {
        Ok(())
    }
}

define_label!(StageLabel);
pub(crate) type BoxedStageLabel = Box<dyn StageLabel>;

/// The labels of the default App rendering stages.
#[derive(Debug, Hash, PartialEq, Eq, Clone)]
pub enum RenderStageLabel {
    /// Applies the host's frame input and advances camera animation, so every later system
    /// sees the frame's view.
    Input,

    /// Requests the tiles the view needs and applies the worker results that arrived. A map
    /// that is handed its tiles, such as a headless one, has no request stage.
    Request,

    /// Creates or resizes GPU resources, such as pipelines and frame attachments, and writes
    /// the frame's uniforms.
    Prepare,

    /// Uploads tile data and queues the phase items the [`Render`](RenderStageLabel::Render)
    /// stage draws.
    Queue,

    /// Sorts the render phases and reads GPU timings that came back from earlier frames.
    PhaseSort,

    /// Records, submits and presents the frame.
    Render,

    /// Clears the frame's render phases and evicts tiles that left the view; it runs even when
    /// an earlier stage failed.
    Cleanup,
}

impl StageLabel for RenderStageLabel {
    fn dyn_clone(&self) -> Box<dyn StageLabel> {
        Box::new(self.clone())
    }

    fn label_name(&self) -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed(match self {
            Self::Input => "Input",
            Self::Request => "Request",
            Self::Prepare => "Prepare",
            Self::Queue => "Queue",
            Self::PhaseSort => "PhaseSort",
            Self::Render => "Render",
            Self::Cleanup => "Cleanup",
        })
    }
}

/// Failure that stops execution of a stage or its containing schedule.
#[derive(Error, Debug)]
pub enum StageError {
    /// A scheduled system failed; its typed cause is preserved.
    #[error("system errored")]
    System(#[from] SystemError),
}

/// A stage could not be added, found or extended.
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum ScheduleError {
    /// A stage with this label already exists.
    #[error("stage {0} already exists")]
    DuplicateStage(String),
    /// No stage has this label.
    #[error("stage {0} does not exist")]
    MissingStage(String),
    /// Systems can only be added to a `SystemStage`, and this stage is absent or another kind.
    #[error("stage {0} does not exist or does not run systems")]
    NotASystemStage(String),
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
/// A stage error skips the later stages, except those marked with
/// [`Schedule::run_after_failure`], without rolling back earlier stages; the first error is
/// returned. Schedules can be nested because they also implement [`Stage`].
#[derive(Default)]
pub struct Schedule {
    entries: Vec<Entry>,
}

struct Entry {
    label: BoxedStageLabel,
    /// The label's name in frame timings, formatted once when the stage is added.
    name: Arc<str>,
    stage: Box<dyn Stage>,
    after_failure: bool,
}

impl Entry {
    fn new(label: BoxedStageLabel, stage: Box<dyn Stage>) -> Self {
        Self {
            name: Arc::from(label.label_name().as_ref()),
            label,
            stage,
            after_failure: false,
        }
    }
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
    /// schedule.add_stage("my_stage", NopStage)?;
    /// # Ok::<(), maplibre::schedule::ScheduleError>(())
    /// ```
    pub fn add_stage<S: Stage>(
        &mut self,
        label: impl StageLabel,
        stage: S,
    ) -> Result<&mut Self, ScheduleError> {
        let index = self.entries.len();
        self.insert(index, Box::new(label), Box::new(stage))
    }

    /// Drops the stage with `label` and removes it from the execution order.
    pub fn remove_stage(&mut self, label: impl StageLabel) -> Result<&mut Self, ScheduleError> {
        let index = self.existing(&label)?;
        self.entries.remove(index);
        Ok(self)
    }

    /// Adds the given `stage` immediately after the `target` stage.
    ///
    /// # Example
    ///
    /// ```
    /// # use maplibre::schedule::{Schedule, NopStage};
    /// #
    /// # let mut schedule = Schedule::default();
    /// # schedule.add_stage("target_stage", NopStage)?;
    /// schedule.add_stage_after("target_stage", "my_stage", NopStage)?;
    /// # Ok::<(), maplibre::schedule::ScheduleError>(())
    /// ```
    pub fn add_stage_after<S: Stage>(
        &mut self,
        target: impl StageLabel,
        label: impl StageLabel,
        stage: S,
    ) -> Result<&mut Self, ScheduleError> {
        let index = self.existing(&target)? + 1;
        self.insert(index, Box::new(label), Box::new(stage))
    }

    /// Adds the given `stage` immediately before the `target` stage.
    ///
    /// # Example
    ///
    /// ```
    /// # use maplibre::schedule::{Schedule, NopStage};
    /// #
    /// # let mut schedule = Schedule::default();
    /// # schedule.add_stage("target_stage", NopStage)?;
    /// schedule.add_stage_before("target_stage", "my_stage", NopStage)?;
    /// # Ok::<(), maplibre::schedule::ScheduleError>(())
    /// ```
    pub fn add_stage_before<S: Stage>(
        &mut self,
        target: impl StageLabel,
        label: impl StageLabel,
        stage: S,
    ) -> Result<&mut Self, ScheduleError> {
        let index = self.existing(&target)?;
        self.insert(index, Box::new(label), Box::new(stage))
    }

    fn insert(
        &mut self,
        index: usize,
        label: BoxedStageLabel,
        stage: Box<dyn Stage>,
    ) -> Result<&mut Self, ScheduleError> {
        if self.position(&*label).is_some() {
            return Err(ScheduleError::DuplicateStage(
                label.label_name().into_owned(),
            ));
        }
        self.entries.insert(index, Entry::new(label, stage));
        Ok(self)
    }

    fn existing(&self, label: &dyn StageLabel) -> Result<usize, ScheduleError> {
        self.position(label)
            .ok_or_else(|| ScheduleError::MissingStage(label.label_name().into_owned()))
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
    /// # schedule.add_stage("my_stage", NopStage).expect("new stage");
    /// #
    /// let stage = schedule.get_stage::<NopStage>(&"my_stage").unwrap();
    /// ```
    pub fn get_stage<T: Stage>(&self, label: &dyn StageLabel) -> Option<&T> {
        self.entries
            .iter()
            .find(|entry| &*entry.label == label)
            .and_then(|entry| entry.stage.downcast_ref::<T>())
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
    /// # schedule.add_stage("my_stage", NopStage).expect("new stage");
    /// #
    /// let stage = schedule.get_stage_mut::<NopStage>(&"my_stage").unwrap();
    /// ```
    pub fn get_stage_mut<T: Stage>(&mut self, label: &dyn StageLabel) -> Option<&mut T> {
        self.entries
            .iter_mut()
            .find(|entry| &*entry.label == label)
            .and_then(|entry| entry.stage.downcast_mut::<T>())
    }

    /// Executes stages in order; after a failure only stages marked with
    /// [`Self::run_after_failure`] run. Completed mutations remain in the context.
    pub fn run_once(&mut self, context: &mut MapContext) -> StageResult {
        self.run_stages(context, |_| true)
    }

    /// Runs the selected stages, preserving their declared order, and records each stage's
    /// time in the [`timings::FrameTimings`].
    pub(crate) fn run_stages(
        &mut self,
        context: &mut MapContext,
        include: impl Fn(&dyn StageLabel) -> bool,
    ) -> StageResult {
        let mut failure = None;
        let heap_sampler = context
            .world
            .resources
            .get::<crate::schedule::system::heap::HeapSampler>()
            .copied();
        let mut stage_cursor: Option<timings::Stamp> = None;
        for entry in &mut self.entries {
            if !include(&*entry.label) {
                continue;
            }
            // After a failure only the stages that release the frame's state still run, so
            // the next frame starts clean.
            if failure.is_some() && !entry.after_failure {
                continue;
            }
            #[cfg(feature = "trace")]
            let _stage_span = tracing::info_span!("stage", name = entry.name.as_ref()).entered();
            let started = stage_cursor.unwrap_or_else(timings::Stamp::now);
            let heap_before =
                heap_sampler.map_or(0, crate::schedule::system::heap::HeapSampler::sample);
            let result = entry.stage.run(context);
            let ended = timings::Stamp::now();
            stage_cursor = Some(ended);
            let spent = ended.elapsed_since(started);
            let grown = heap_sampler.map(|s| s.sample() - heap_before);
            timings::record_stage(&mut context.world, &entry.name, spent, grown);
            if let Err(error) = result {
                if failure.is_none() {
                    failure = Some(error);
                } else {
                    tracing::error!(%error, stage = entry.name.as_ref(), "stage failed after an earlier failure");
                }
            }
        }
        failure.map_or(Ok(()), Err)
    }

    fn position(&self, label: &dyn StageLabel) -> Option<usize> {
        self.entries.iter().position(|entry| &*entry.label == label)
    }

    /// Marks the stage with `label` to run even when an earlier stage of the same update
    /// failed, for stages that release per-frame state.
    pub fn run_after_failure(
        &mut self,
        label: impl StageLabel,
    ) -> Result<&mut Self, ScheduleError> {
        let index = self.existing(&label)?;
        if let Some(entry) = self.entries.get_mut(index) {
            entry.after_failure = true;
        }
        Ok(self)
    }

    /// Drops all stages and their labels without changing the frame context.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Iterates over all of schedule's stages and their labels, in execution order.
    pub fn iter_stages(&self) -> impl Iterator<Item = (&dyn StageLabel, &dyn Stage)> {
        self.entries
            .iter()
            .map(|entry| (&*entry.label, &*entry.stage))
    }

    /// Adds a system to the [`Stage`] identified by `stage_label`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use maplibre::context::MapContext;
    /// # use maplibre::schedule::system::stage::SystemStage;
    /// # use maplibre::schedule::{Schedule, NopStage};
    /// # use maplibre::schedule::system::SystemError;
    /// #
    /// # let mut schedule = Schedule::default();
    /// # schedule.add_stage("my_stage", SystemStage::default())?;
    /// # fn my_system(context: &mut MapContext) -> Result<(), SystemError> { Ok(()) }
    /// #
    /// schedule.add_system_to_stage("my_stage", my_system)?;
    /// # Ok::<(), maplibre::schedule::ScheduleError>(())
    /// ```
    ///
    pub fn add_system_to_stage(
        &mut self,
        stage_label: impl StageLabel,
        system: impl IntoSystemContainer,
    ) -> Result<&mut Self, ScheduleError> {
        let stage = self
            .get_stage_mut::<SystemStage>(&stage_label)
            .ok_or_else(|| ScheduleError::NotASystemStage(stage_label.label_name().into_owned()))?;
        stage.add_system(system);
        Ok(self)
    }

    /// Adds a system to the leading group of the [`Stage`] identified by `stage_label`, preserving
    /// registration order among early systems ahead of regular stage systems.
    pub fn add_early_system_to_stage(
        &mut self,
        stage_label: impl StageLabel,
        system: impl IntoSystemContainer,
    ) -> Result<&mut Self, ScheduleError> {
        let stage = self
            .get_stage_mut::<SystemStage>(&stage_label)
            .ok_or_else(|| ScheduleError::NotASystemStage(stage_label.label_name().into_owned()))?;
        stage.add_early_system(system);
        Ok(self)
    }
}

impl Stage for Schedule {
    fn run(&mut self, context: &mut MapContext) -> StageResult {
        self.run_once(context)?;
        Ok(())
    }
}
