//! Work done around the submission of the frame's command buffer.

use crate::tcs::world::World;

/// Callback trait for post-submit processing.
pub trait AfterSubmitCallback: 'static + Send + Sync {
    /// Executed after command encoder finishes and queue.submit completes.
    fn after_submit(&self, world: &World);
}

impl<F> AfterSubmitCallback for F
where
    F: Fn(&World) + Send + Sync + 'static,
{
    fn after_submit(&self, world: &World) {
        (self)(world);
    }
}

/// Container for post-submit callbacks stored in World resources.
#[derive(Default)]
pub struct AfterSubmitCallbacks {
    callbacks: Vec<Box<dyn AfterSubmitCallback>>,
}

impl AfterSubmitCallbacks {
    /// Creates an empty callback container.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a post-submit callback.
    pub fn register(&mut self, callback: impl AfterSubmitCallback) {
        self.callbacks.push(Box::new(callback));
    }

    /// Runs all registered callbacks.
    pub fn run(&self, world: &World) {
        for callback in &self.callbacks {
            callback.after_submit(world);
        }
    }
}

/// Commands recorded into the frame's encoder after every render pass and before the frame is
/// submitted, such as resolving timestamp queries the passes wrote.
#[derive(Default)]
pub struct BeforeSubmitHooks {
    hooks: Vec<fn(&World, &mut wgpu::CommandEncoder)>,
}

impl BeforeSubmitHooks {
    /// Registers a hook; hooks run in registration order.
    pub fn register(&mut self, hook: fn(&World, &mut wgpu::CommandEncoder)) {
        self.hooks.push(hook);
    }

    /// Runs every hook against the frame's encoder.
    pub fn run(&self, world: &World, encoder: &mut wgpu::CommandEncoder) {
        for hook in &self.hooks {
            hook(world, encoder);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };

    use super::*;

    #[test]
    fn test_after_submit_callbacks() {
        let mut callbacks = AfterSubmitCallbacks::new();
        let executed = Arc::new(AtomicBool::new(false));
        let flag = executed.clone();

        callbacks.register(move |_world: &World| {
            flag.store(true, Ordering::SeqCst);
        });

        let world = World::default();
        assert!(!executed.load(Ordering::SeqCst));
        callbacks.run(&world);
        assert!(executed.load(Ordering::SeqCst));
    }
}
