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
    /// Whether `main_pass` should inline `translucent_pass` draws directly into its own render pass
    /// because no intermediate pass or symbol depth copy runs between them this frame.
    pub(crate) inline_translucent_pass: bool,
    /// Whether any pass scheduled between `main_pass` and `translucent_pass` has active work and
    /// therefore reads the main pass's multisampled colour or depth/stencil attachments.
    pub(crate) between_main_and_translucent_has_work: bool,
}

impl<'d> RenderContext<'d> {
    /// Creates a frame render context around `device` and `command_encoder`.
    pub fn new(device: &'d wgpu::Device, command_encoder: wgpu::CommandEncoder) -> Self {
        Self {
            device,
            command_encoder,
            inline_translucent_pass: false,
            between_main_and_translucent_has_work: false,
        }
    }
}

/// One render pass of the frame.
pub trait Node: Send + Sync + 'static {
    /// Records the pass's commands. A pass whose inputs are not ready records nothing.
    fn run(&self, render_context: &mut RenderContext, resources: &RenderResources, world: &World);

    /// Whether this pass may write 3D occluders into the shared scene depth attachment outside
    /// `main_pass` (defaults to `true` so external custom 3D scene passes preserve symbol depth
    /// occlusion automatically).
    fn writes_custom_scene_depth(&self) -> bool {
        true
    }

    /// Whether this pass will record GPU commands on the current frame.
    ///
    /// Defaults to `true` so external host passes are never bypassed when fusing built-in passes.
    fn has_active_work(&self, _resources: &RenderResources, _world: &World) -> bool {
        true
    }
}

/// A pass that records nothing, used as a named anchor other passes order themselves around.
pub struct EmptyNode;

impl Node for EmptyNode {
    fn run(&self, _: &mut RenderContext, _: &RenderResources, _: &World) {}

    fn writes_custom_scene_depth(&self) -> bool {
        false
    }

    fn has_active_work(&self, _: &RenderResources, _: &World) -> bool {
        false
    }
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

struct ResolvedOrder {
    order: Vec<usize>,
    has_main_before_translucent: bool,
    trans_pass_index: Option<usize>,
    between_indices: Vec<usize>,
}

/// The frame's passes in the order they run.
#[derive(Default)]
pub struct RenderGraph {
    passes: Vec<Pass>,
    /// `(before, after)` pass indices.
    constraints: BTreeSet<(usize, usize)>,
    /// Resolved run order and pass-fusion indices, updated after passes or edges change.
    order: Option<ResolvedOrder>,
    has_custom_depth_writers: bool,
    has_named_depth_readers: bool,
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
        self.has_custom_depth_writers |= node.writes_custom_scene_depth();
        self.has_named_depth_readers |= matches!(
            name.as_ref(),
            "overlay_pass" | "depth_probe_pass" | "sun_depth_copy"
        );
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

    /// Whether any registered pass may write custom 3D scene depth outside `main_pass`.
    pub(crate) fn has_custom_scene_depth_writers(&self) -> bool {
        self.has_custom_depth_writers
    }

    /// Whether any built-in depth-reading pass (`overlay_pass`, `depth_probe_pass`, or
    /// `sun_depth_copy`) is present in the graph.
    pub(crate) fn has_named_depth_reader_passes(&self) -> bool {
        self.has_named_depth_readers
    }

    /// Whether `main_pass` runs before `translucent_pass`, required for inline pass fusion.
    pub(crate) fn has_main_before_translucent(&self) -> bool {
        match &self.order {
            Some(order) => order.has_main_before_translucent,
            None => self
                .resolve_metadata()
                .is_ok_and(|resolved| resolved.has_main_before_translucent),
        }
    }

    /// Resolves the run order after passes or constraints changed.
    pub fn prepare(&mut self) -> Result<(), RenderGraphError> {
        if self.order.is_none() {
            self.order = Some(self.resolve_metadata()?);
        }
        Ok(())
    }

    /// The pass names in the order they run.
    pub fn node_order(&self) -> Result<Vec<&str>, RenderGraphError> {
        let order = match &self.order {
            Some(resolved) => Cow::Borrowed(&resolved.order),
            None => Cow::Owned(self.resolve()?),
        };
        Ok(order
            .iter()
            .filter_map(|&index| self.passes.get(index).map(|pass| pass.name.as_ref()))
            .collect())
    }

    /// Records every pass in run order, resolving the order first if it is stale.
    pub(crate) fn run(
        &self,
        render_context: &mut RenderContext,
        resources: &RenderResources,
        world: &World,
    ) -> Result<(), RenderGraphError> {
        let owned;
        let resolved = match &self.order {
            Some(resolved) => resolved,
            None => {
                owned = self.resolve_metadata()?;
                &owned
            }
        };
        let between_has_work = resolved.between_indices.iter().any(|&idx| {
            self.passes
                .get(idx)
                .is_some_and(|p| p.node.has_active_work(resources, world))
        });
        render_context.between_main_and_translucent_has_work = between_has_work;
        render_context.inline_translucent_pass = resolved.has_main_before_translucent
            && !between_has_work
            && !crate::render::translucent_pass::needs_symbol_depth_copy(world);
        for &index in &resolved.order {
            if render_context.inline_translucent_pass && resolved.trans_pass_index == Some(index) {
                continue;
            }
            let Some(pass) = self.passes.get(index) else {
                continue;
            };
            #[cfg(feature = "trace")]
            let _span = tracing::info_span!("render_pass", name = pass.name.as_ref()).entered();
            pass.node.run(render_context, resources, world);
        }
        Ok(())
    }

    fn resolve_metadata(&self) -> Result<ResolvedOrder, RenderGraphError> {
        let order = self.resolve()?;
        let main_pos = order.iter().position(|&idx| {
            self.passes
                .get(idx)
                .is_some_and(|p| p.name.as_ref() == crate::render::draw_graph::node::MAIN_PASS)
        });
        let trans_pos = order.iter().position(|&idx| {
            self.passes.get(idx).is_some_and(|p| {
                p.name.as_ref() == crate::render::draw_graph::node::TRANSLUCENT_PASS
            })
        });
        let trans_pass_index = trans_pos.and_then(|pos| order.get(pos).copied());
        let (has_main_before_translucent, between_indices) = match (main_pos, trans_pos) {
            (Some(m), Some(t)) if m < t => (true, order.get(m + 1..t).unwrap_or(&[]).to_vec()),
            _ => (false, Vec::new()),
        };
        Ok(ResolvedOrder {
            order,
            has_main_before_translucent,
            trans_pass_index,
            between_indices,
        })
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
            if let Some(slot) = waiting_on.get_mut(after) {
                *slot += 1;
            }
        }
        let mut ready: BTreeSet<usize> = waiting_on
            .iter()
            .enumerate()
            .filter_map(|(index, &count)| (count == 0).then_some(index))
            .collect();
        let mut order = Vec::with_capacity(self.passes.len());
        while let Some(next) = ready.pop_first() {
            order.push(next);
            for &(_, after) in self.constraints.range((next, 0)..=(next, usize::MAX)) {
                if let Some(slot) = waiting_on.get_mut(after) {
                    *slot -= 1;
                    if *slot == 0 {
                        ready.insert(after);
                    }
                }
            }
        }
        if order.len() < self.passes.len() {
            let cycle = waiting_on
                .iter()
                .enumerate()
                .filter_map(|(index, &count)| {
                    (count > 0)
                        .then(|| self.passes.get(index).map(|p| p.name.clone()))
                        .flatten()
                })
                .collect();
            return Err(RenderGraphError::Cycle(cycle));
        }
        Ok(order)
    }
}

#[cfg(test)]
mod tests;
