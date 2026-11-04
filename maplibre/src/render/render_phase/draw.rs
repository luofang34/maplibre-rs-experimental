use std::{any::TypeId, collections::HashMap, marker::PhantomData, num::NonZeroU32, ops::Range};

use crate::tcs::world::World;

/// Encodes a phase item's draw commands using resources borrowed from the frame's world.
pub trait Draw<P: ?Sized>: 'static {
    /// Draws `item` by issuing state bindings and draw calls via [`TrackedRenderPass`](crate::render::tracked_pass::TrackedRenderPass).
    fn draw<'w>(
        &self,
        pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
        world: &'w World,
        item: &P,
    );
}

/// An item which will be drawn to the screen. A phase item should be queued up for rendering
/// during the [`RenderStageLabel::Queue`](crate::render::RenderStageLabel::Queue) stage.
/// Afterwards it will be sorted and rendered automatically in the
/// [`RenderStageLabel::PhaseSort`](crate::render::RenderStageLabel::PhaseSort) stage and
/// [`RenderStageLabel::Render`](crate::render::RenderStageLabel::Render) stage, respectively.
pub trait PhaseItem {
    /// The type used for ordering the items. The smallest values are drawn first.
    type SortKey: Ord;
    /// Determines the order in which the items are drawn during the corresponding [`RenderPhase`](super::RenderPhase).
    fn sort_key(&self) -> Self::SortKey;

    /// Command implementation responsible for this item; it may skip missing resources.
    fn draw_function(&self) -> &dyn Draw<Self>;

    /// Projection uniform bound at group zero when drawing this item.
    fn projection_binding(&self) -> super::ProjectionBinding {
        super::ProjectionBinding::View
    }
}

/// [`RenderCommand`] is a trait that runs a world query and produces one or more
/// [`TrackedRenderPass`](crate::render::tracked_pass::TrackedRenderPass) calls.
/// Types implementing this trait can be composed as tuples.
///
/// Works uniformly for sorted [`PhaseItem`]s and binned [`BinnedPhaseItem`](super::BinnedPhaseItem)s.
/// [`DrawState`] adapts a command or command tuple into a [`Draw`] implementation.
pub trait RenderCommand<P> {
    /// Renders `item` by issuing bindings and draw calls via [`TrackedRenderPass`](crate::render::tracked_pass::TrackedRenderPass).
    fn render<'w>(
        world: &'w World,
        item: &P,
        pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    ) -> RenderCommandResult;
}

/// Whether a command tuple may continue; commands run left to right and stop on failure.
/// Commands already encoded before a failure are not rolled back.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RenderCommandResult {
    /// Continue with the next command in the tuple.
    Success,
    /// Skip the remaining commands for this item, commonly because resources are unavailable.
    Failure,
}

macro_rules! render_command_tuple_impl {
    ($($name: ident),*) => {
        impl<P, $($name: RenderCommand<P>),*> RenderCommand<P> for ($($name,)*) {
            #[allow(non_snake_case)]
            fn render<'w>(
                world: &'w World,
                item: &P,
                pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
            ) -> RenderCommandResult {
                $(if let RenderCommandResult::Failure = $name::render(world, item, pass) {
                    return RenderCommandResult::Failure;
                })*
                RenderCommandResult::Success
            }
        }
    };
}

render_command_tuple_impl!(C0);
render_command_tuple_impl!(C0, C1);
render_command_tuple_impl!(C0, C1, C2);
render_command_tuple_impl!(C0, C1, C2, C3);
render_command_tuple_impl!(C0, C1, C2, C3, C4);
render_command_tuple_impl!(C0, C1, C2, C3, C4, C5);
render_command_tuple_impl!(C0, C1, C2, C3, C4, C5, C6);
render_command_tuple_impl!(C0, C1, C2, C3, C4, C5, C6, C7);

/// Stateless adapter from one render command or a command tuple to a phase-item [`Draw`].
/// A failed command stops its tuple without aborting the render pass.
pub struct DrawState<P, C> {
    phantom_p: PhantomData<fn() -> P>,
    phantom_c: PhantomData<fn() -> C>,
}

impl<P, C> DrawState<P, C> {
    /// Creates a stateless [`Draw`] adapter for render command `C`.
    pub const fn new() -> Self {
        DrawState {
            phantom_p: PhantomData,
            phantom_c: PhantomData,
        }
    }
}

impl<P, C> Default for DrawState<P, C> {
    fn default() -> Self {
        Self::new()
    }
}

impl<P: 'static, C: 'static> Draw<P> for DrawState<P, C>
where
    C: RenderCommand<P>,
{
    fn draw<'w>(
        &self,
        pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
        world: &'w World,
        item: &P,
    ) {
        C::render(world, item, pass);
    }
}

/// Compact identifier of a [`Draw`] implementation registered in [`DrawFunctions`].
///
/// Storing a 32-bit `DrawFunctionId` instead of a boxed trait object per queued item eliminates
/// per-frame heap allocations in [`BinnedRenderPhase`](super::BinnedRenderPhase).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DrawFunctionId(pub u32);

/// Registry of reusable [`Draw`] functions for phase item type `P`, stored in [`World::resources`].
///
/// Mirrors Bevy's `DrawFunctions<P>`: draw implementations or [`RenderCommand`] tuples are
/// registered once at plugin setup (or lazily via idempotent [`Self::add_with`]) and looked up
/// by [`DrawFunctionId`] during pass recording with zero per-frame allocations.
pub struct DrawFunctions<P> {
    draw_functions: Vec<Box<dyn Draw<P>>>,
    indices: HashMap<TypeId, DrawFunctionId>,
}

impl<P> Default for DrawFunctions<P> {
    fn default() -> Self {
        Self::new()
    }
}

impl<P> DrawFunctions<P> {
    /// Creates an empty draw-function registry.
    pub fn new() -> Self {
        Self {
            draw_functions: Vec::new(),
            indices: HashMap::new(),
        }
    }

    /// Number of unique draw functions registered so far.
    #[inline]
    pub fn len(&self) -> usize {
        self.draw_functions.len()
    }

    /// Whether no draw functions have been registered yet.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.draw_functions.is_empty()
    }

    /// Looks up the [`DrawFunctionId`] previously registered under key type `T`.
    #[inline]
    pub fn id<T: 'static>(&self) -> Option<DrawFunctionId> {
        self.indices.get(&TypeId::of::<T>()).copied()
    }

    /// Borrows the [`Draw`] implementation associated with `id`.
    #[inline]
    pub fn get(&self, id: DrawFunctionId) -> Option<&dyn Draw<P>> {
        self.draw_functions.get(id.0 as usize).map(AsRef::as_ref)
    }
}

impl<P: 'static> DrawFunctions<P> {
    /// Registers `draw_function` keyed by its concrete type `D`, returning its [`DrawFunctionId`].
    ///
    /// Re-registering the same concrete type `D` is idempotent and returns the existing ID without
    /// duplicating the entry.
    pub fn add<D: Draw<P> + 'static>(&mut self, draw_function: D) -> DrawFunctionId {
        self.add_mapped::<D, D>(draw_function)
    }

    /// Registers `draw_function` under an explicit lookup key type `K`, returning its [`DrawFunctionId`].
    ///
    /// Re-registering the same key type `K` is idempotent and returns the existing ID.
    pub fn add_mapped<K: 'static, D: Draw<P> + 'static>(
        &mut self,
        draw_function: D,
    ) -> DrawFunctionId {
        let key_type = TypeId::of::<K>();
        if let Some(&existing) = self.indices.get(&key_type) {
            return existing;
        }
        let id = DrawFunctionId(u32::try_from(self.draw_functions.len()).unwrap_or(u32::MAX));
        self.draw_functions.push(Box::new(draw_function));
        self.indices.insert(key_type, id);
        self.indices.insert(TypeId::of::<D>(), id);
        id
    }

    /// Wraps [`RenderCommand`] (or command tuple) `C` in [`DrawState`] and registers it idempotently.
    ///
    /// Repeated calls for the same command type `C` return the same [`DrawFunctionId`] without
    /// allocating another boxed [`Draw`] object.
    pub fn add_with<C: RenderCommand<P> + 'static>(&mut self) -> DrawFunctionId {
        let cmd_type = TypeId::of::<C>();
        if let Some(&existing) = self.indices.get(&cmd_type) {
            return existing;
        }
        self.add_mapped::<C, DrawState<P, C>>(DrawState::<P, C>::new())
    }
}

/// Supplementary dynamic offset or indirect parameter slot index attached to a phase item.
///
/// Mirrors Bevy's `PhaseItemExtraIndex` to unify direct hardware instancing, WebGL2/downlevel
/// dynamic uniform offsets, single indirect draws, and multi-draw indirect batch sets.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum PhaseItemExtraIndex {
    /// No dynamic uniform offset or indirect parameter slot; draws directly via `batch_range`.
    #[default]
    None,
    /// Instance data is bound as a per-batch `VertexStepMode::Instance` sub-slice via
    /// [`BatchedInstanceBuffer::instance_slice`](super::BatchedInstanceBuffer::instance_slice),
    /// so [`draw_binned_mesh`](super::draw_binned_mesh) dispatches `0..count` (`first_instance == 0`).
    VertexInstance,
    /// Byte offset into a dynamic uniform buffer (used on WebGL2/downlevel backends or unbatchable items).
    DynamicOffset(u32),
    /// Contiguous slot range within [`IndirectParametersBuffers`](super::IndirectParametersBuffers).
    IndirectParametersIndex {
        /// Slot range (`start..end`) in the indexed or non-indexed indirect parameter buffer.
        range: Range<u32>,
        /// Optional non-zero batch-set slot when part of a multi-draw indirect batch set.
        batch_set_index: Option<NonZeroU32>,
    },
}

impl PhaseItemExtraIndex {
    /// Constant alias for [`PhaseItemExtraIndex::None`], matching Bevy's `PhaseItemExtraIndex::NONE`.
    pub const NONE: Self = Self::None;

    /// Converts an optional dynamic uniform offset into [`PhaseItemExtraIndex`].
    #[inline]
    pub const fn maybe_dynamic_offset(offset: Option<u32>) -> Self {
        match offset {
            Some(offset) => Self::DynamicOffset(offset),
            None => Self::None,
        }
    }

    /// Whether this batch uses a sub-sliced `VertexStepMode::Instance` buffer (`0..count`).
    #[inline]
    pub const fn is_vertex_instance(&self) -> bool {
        matches!(self, Self::VertexInstance)
    }

    /// Returns the dynamic uniform byte offset when this is [`Self::DynamicOffset`].
    #[inline]
    pub const fn dynamic_offset(&self) -> Option<u32> {
        match self {
            Self::DynamicOffset(offset) => Some(*offset),
            Self::None | Self::VertexInstance | Self::IndirectParametersIndex { .. } => None,
        }
    }

    /// Returns the indirect parameter slot range when this is [`Self::IndirectParametersIndex`].
    #[inline]
    pub const fn indirect_parameters_range(&self) -> Option<&Range<u32>> {
        match self {
            Self::IndirectParametersIndex { range, .. } => Some(range),
            Self::None | Self::VertexInstance | Self::DynamicOffset(_) => None,
        }
    }
}
