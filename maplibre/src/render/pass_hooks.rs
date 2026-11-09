//! Extension hooks executed during render passes.

use std::{collections::HashMap, sync::Arc};

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

/// A draw hook callable during render pass execution.
pub trait PassDrawHook: 'static + Send + Sync {
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

/// Container for pass draw hooks registered in the World resources.
#[derive(Default, Clone)]
pub struct PassDrawHooks {
    hooks: HashMap<PassHookPoint, Vec<Arc<dyn PassDrawHook>>>,
}

impl PassDrawHooks {
    /// Creates a new empty hook registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a draw hook for a specific hook point.
    pub fn register(&mut self, point: PassHookPoint, hook: impl PassDrawHook) {
        self.hooks.entry(point).or_default().push(Arc::new(hook));
    }

    /// Retrieves cloned handles to all registered hooks for the hook point.
    pub fn get_hooks(&self, point: PassHookPoint) -> Vec<Arc<dyn PassDrawHook>> {
        self.hooks.get(&point).cloned().unwrap_or_default()
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
