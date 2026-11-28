//! Extension hooks executed during render passes.

use std::sync::Arc;

use crate::{render::tracked_pass::TrackedRenderPass, tcs::world::World};

/// Identifies the execution point within a render pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PassHookPoint {
    /// Beginning of MAIN_PASS, before layer drawing.
    MainPassBegin,
    /// Beginning of TRANSLUCENT_PASS, before translucent items.
    TranslucentPassBegin,
    /// End of TRANSLUCENT_PASS, after all map layers and symbols have drawn.
    TranslucentPassEnd,
}

impl PassHookPoint {
    #[inline]
    const fn index(self) -> usize {
        match self {
            Self::MainPassBegin => 0,
            Self::TranslucentPassBegin => 1,
            Self::TranslucentPassEnd => 2,
        }
    }
}

/// A draw hook callable during render pass execution.
pub trait PassDrawHook: 'static + Send + Sync {
    /// Whether this hook has active draw work to record in the current frame.
    ///
    /// Defaults to `true` so custom closures without an explicit predicate always run.
    fn has_work(&self, _world: &World) -> bool {
        true
    }

    /// Records draw commands into the given tracked render pass.
    fn draw<'w>(&self, pass: &mut TrackedRenderPass<'w>, world: &'w World);
}

impl<F> PassDrawHook for F
where
    F: for<'w> Fn(&mut TrackedRenderPass<'w>, &'w World) + Send + Sync + 'static,
{
    fn draw<'w>(&self, pass: &mut TrackedRenderPass<'w>, world: &'w World) {
        (self)(pass, world);
    }
}

struct ConditionalHook<P, D> {
    has_work: P,
    draw: D,
}

impl<P, D> PassDrawHook for ConditionalHook<P, D>
where
    P: Fn(&World) -> bool + Send + Sync + 'static,
    D: for<'w> Fn(&mut TrackedRenderPass<'w>, &'w World) + Send + Sync + 'static,
{
    fn has_work(&self, world: &World) -> bool {
        (self.has_work)(world)
    }

    fn draw<'w>(&self, pass: &mut TrackedRenderPass<'w>, world: &'w World) {
        (self.draw)(pass, world);
    }
}

/// Container for pass draw hooks registered in the World resources.
#[derive(Default, Clone)]
pub struct PassDrawHooks {
    hooks: [Vec<Arc<dyn PassDrawHook>>; 3],
}

impl PassDrawHooks {
    /// Creates a new empty hook registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a draw hook for a specific hook point.
    pub fn register(&mut self, point: PassHookPoint, hook: impl PassDrawHook) {
        if let Some(bucket) = self.hooks.get_mut(point.index()) {
            bucket.push(Arc::new(hook));
        }
    }

    /// Registers a draw hook guarded by a fast per-frame active-work predicate.
    pub fn register_conditional<P, D>(&mut self, point: PassHookPoint, has_work: P, draw: D)
    where
        P: Fn(&World) -> bool + Send + Sync + 'static,
        D: for<'w> Fn(&mut TrackedRenderPass<'w>, &'w World) + Send + Sync + 'static,
    {
        self.register(point, ConditionalHook { has_work, draw });
    }

    /// Borrows all registered hooks for `point` without heap allocation.
    pub fn hooks(&self, point: PassHookPoint) -> &[Arc<dyn PassDrawHook>] {
        self.hooks.get(point.index()).map_or(&[], Vec::as_slice)
    }

    /// Returns whether any hook registered at `point` reports active draw work for `world`.
    pub fn has_active_work(&self, point: PassHookPoint, world: &World) -> bool {
        self.hooks(point).iter().any(|hook| hook.has_work(world))
    }

    /// Retrieves cloned handles to all registered hooks for the hook point.
    pub fn get_hooks(&self, point: PassHookPoint) -> Vec<Arc<dyn PassDrawHook>> {
        self.hooks.get(point.index()).cloned().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    struct CounterHook(Arc<AtomicUsize>);
    impl PassDrawHook for CounterHook {
        fn draw<'w>(&self, _pass: &mut TrackedRenderPass<'w>, _world: &'w World) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn test_pass_draw_hooks_registration_and_retrieval() {
        let mut hooks = PassDrawHooks::new();
        assert!(hooks.get_hooks(PassHookPoint::MainPassBegin).is_empty());
        assert!(hooks
            .get_hooks(PassHookPoint::TranslucentPassBegin)
            .is_empty());

        let counter = Arc::new(AtomicUsize::new(0));
        hooks.register(PassHookPoint::MainPassBegin, CounterHook(counter.clone()));

        let main_hooks = hooks.get_hooks(PassHookPoint::MainPassBegin);
        assert_eq!(main_hooks.len(), 1);
        assert!(hooks
            .get_hooks(PassHookPoint::TranslucentPassBegin)
            .is_empty());
    }
}
