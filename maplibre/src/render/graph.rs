//! The frame's render passes and the order constraints between them.
//!
//! Plugins add passes under names and declare which pass must run before which. The order is
//! resolved once after the passes change: every constraint holds, and passes the constraints
//! leave unordered run in the order they were added, so a frame records the same commands on
//! every run and every platform.

#![deny(missing_docs)]

use std::{borrow::Cow, collections::BTreeSet};

use thiserror::Error;

use crate::{render::RenderResources, tcs::world::World};

/// The device and the frame's command encoder, which every pass records into.
pub struct RenderContext<'d> {
    /// Device that owns the resources the passes use.
    pub device: &'d wgpu::Device,
    /// The frame's single command stream; passes append to it and the runner submits it.
    pub command_encoder: wgpu::CommandEncoder,
}

/// One render pass of the frame.
pub trait Node: Send + Sync + 'static {
    /// Records the pass's commands. A pass whose inputs are not ready records nothing.
    fn run(&self, render_context: &mut RenderContext, resources: &RenderResources, world: &World);
}

/// A pass that records nothing, used as a named anchor other passes order themselves around.
pub struct EmptyNode;

impl Node for EmptyNode {
    fn run(&self, _: &mut RenderContext, _: &RenderResources, _: &World) {}
}

/// A pass could not be added, ordered or resolved.
#[derive(Error, Debug, Clone, Eq, PartialEq)]
pub enum RenderGraphError {
    /// No pass has this name.
    #[error("render pass {0:?} does not exist")]
    InvalidNode(Cow<'static, str>),
    /// A pass with this name already exists.
    #[error("render pass {0:?} already exists")]
    DuplicateNode(Cow<'static, str>),
    /// The order constraints form a cycle through these passes, so none of them can run first.
    #[error("render passes {0:?} require each other to run first")]
    Cycle(Vec<Cow<'static, str>>),
}

struct Pass {
    name: Cow<'static, str>,
    node: Box<dyn Node>,
}

/// The frame's passes in the order they run.
#[derive(Default)]
pub struct RenderGraph {
    passes: Vec<Pass>,
    /// `(before, after)` pass indices.
    constraints: BTreeSet<(usize, usize)>,
    /// Pass indices in run order, resolved after the last change.
    order: Option<Vec<usize>>,
}

impl RenderGraph {
    /// Adds a pass that runs after the passes added before it, unless constraints say
    /// otherwise.
    pub fn add_node(
        &mut self,
        name: impl Into<Cow<'static, str>>,
        node: impl Node,
    ) -> Result<(), RenderGraphError> {
        let name = name.into();
        if self.index(&name).is_some() {
            return Err(RenderGraphError::DuplicateNode(name));
        }
        self.passes.push(Pass {
            name,
            node: Box::new(node),
        });
        self.order = None;
        Ok(())
    }

    /// Requires the pass `before` to run before the pass `after`.
    pub fn add_node_edge(
        &mut self,
        before: impl Into<Cow<'static, str>>,
        after: impl Into<Cow<'static, str>>,
    ) -> Result<(), RenderGraphError> {
        let before = self.existing(before.into())?;
        let after = self.existing(after.into())?;
        self.constraints.insert((before, after));
        self.order = None;
        Ok(())
    }

    /// Whether a pass with this name exists.
    pub fn contains(&self, name: &str) -> bool {
        self.index(name).is_some()
    }

    /// Resolves the run order after passes or constraints changed.
    pub fn prepare(&mut self) -> Result<(), RenderGraphError> {
        if self.order.is_none() {
            self.order = Some(self.resolve()?);
        }
        Ok(())
    }

    /// The pass names in the order they run.
    pub fn node_order(&self) -> Result<Vec<&str>, RenderGraphError> {
        let order = match &self.order {
            Some(order) => order.clone(),
            None => self.resolve()?,
        };
        Ok(order
            .into_iter()
            .map(|index| self.passes[index].name.as_ref())
            .collect())
    }

    /// Records every pass in run order, resolving the order first if it is stale.
    pub(crate) fn run(
        &self,
        render_context: &mut RenderContext,
        resources: &RenderResources,
        world: &World,
    ) -> Result<(), RenderGraphError> {
        let order = match &self.order {
            Some(order) => Cow::Borrowed(order),
            None => Cow::Owned(self.resolve()?),
        };
        for &index in order.iter() {
            let pass = &self.passes[index];
            #[cfg(feature = "trace")]
            let _span = tracing::info_span!("render_pass", name = pass.name.as_ref()).entered();
            pass.node.run(render_context, resources, world);
        }
        Ok(())
    }

    fn index(&self, name: &str) -> Option<usize> {
        self.passes.iter().position(|pass| pass.name == name)
    }

    fn existing(&self, name: Cow<'static, str>) -> Result<usize, RenderGraphError> {
        self.index(&name).ok_or(RenderGraphError::InvalidNode(name))
    }

    /// Kahn's algorithm, always taking the earliest-added pass that is ready.
    fn resolve(&self) -> Result<Vec<usize>, RenderGraphError> {
        let mut waiting_on = vec![0usize; self.passes.len()];
        for &(_, after) in &self.constraints {
            waiting_on[after] += 1;
        }
        let mut ready: BTreeSet<usize> = (0..self.passes.len())
            .filter(|&index| waiting_on[index] == 0)
            .collect();
        let mut order = Vec::with_capacity(self.passes.len());
        while let Some(next) = ready.pop_first() {
            order.push(next);
            for &(_, after) in self.constraints.range((next, 0)..=(next, usize::MAX)) {
                waiting_on[after] -= 1;
                if waiting_on[after] == 0 {
                    ready.insert(after);
                }
            }
        }
        if order.len() < self.passes.len() {
            let cycle = (0..self.passes.len())
                .filter(|&index| waiting_on[index] > 0)
                .map(|index| self.passes[index].name.clone())
                .collect();
            return Err(RenderGraphError::Cycle(cycle));
        }
        Ok(order)
    }
}

#[cfg(test)]
mod tests;
