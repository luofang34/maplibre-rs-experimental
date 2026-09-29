use std::marker::PhantomData;

use crate::tcs::world::World;

/// Encodes a phase item's draw commands using resources borrowed from the frame's world.
pub trait Draw<P: PhaseItem>: 'static {
    /// Draws the [`PhaseItem`] by issuing draw calls via the [`wgpu::RenderPass`].
    fn draw<'w>(&self, pass: &mut wgpu::RenderPass<'w>, world: &'w World, item: &P);
}

/// An item which will be drawn to the screen. A phase item should be queued up for rendering
/// during the [`RenderStageLabel::Queue`](crate::render::RenderStageLabel::Queue) stage.
/// Afterwards it will be sorted and rendered automatically  in the
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

/// [`RenderCommand`] is a trait that runs an ECS query and produces one or more
/// [`wgpu::RenderPass`] calls. Types implementing this trait can be composed (as tuples).
///
/// [`DrawState`] adapts a command or command tuple into a [`Draw`] implementation.
pub trait RenderCommand<P: PhaseItem> {
    /// Renders the [`PhaseItem`] by issuing draw calls via the [`wgpu::RenderPass`].
    fn render<'w>(
        world: &'w World,
        item: &P,
        pass: &mut wgpu::RenderPass<'w>,
    ) -> RenderCommandResult;
}

/// Whether a command tuple may continue; commands run left to right and stop on failure.
/// Commands already encoded before a failure are not rolled back.
pub enum RenderCommandResult {
    /// Continue with the next command in the tuple.
    Success,
    /// Skip the remaining commands for this item, commonly because resources are unavailable.
    Failure,
}

macro_rules! render_command_tuple_impl {
    ($($name: ident),*) => {
        impl<P: PhaseItem, $($name: RenderCommand<P>),*> RenderCommand<P> for ($($name,)*) {
            #[allow(non_snake_case)]
            fn render<'w>(
                world: &'w World,
                item: &P,
                pass: &mut wgpu::RenderPass<'w>,
            ) -> RenderCommandResult{
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

/// Stateless adapter from one render command or a command tuple to a phase-item draw.
/// A failed command stops its tuple without aborting the render pass.
pub struct DrawState<P, C> {
    phantom_p: PhantomData<P>,
    phantom_c: PhantomData<C>,
}

impl<P, C> DrawState<P, C> {
    pub(crate) fn new() -> Self {
        DrawState {
            phantom_p: Default::default(),
            phantom_c: Default::default(),
        }
    }
}

impl<P: 'static, C: 'static> Draw<P> for DrawState<P, C>
where
    P: PhaseItem,
    C: RenderCommand<P>,
{
    /// Prepares data for the wrapped [`RenderCommand`] and then renders it.
    fn draw<'w>(&self, pass: &mut wgpu::RenderPass<'w>, world: &'w World, item: &P) {
        C::render(world, item, pass);
    }
}
