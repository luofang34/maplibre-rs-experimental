use std::{any::type_name, borrow::Cow};

use crate::{
    context::MapContext,
    tcs::system::{System, SystemResult},
};

/// Converts a stateful map-update function into a [`System`].
pub trait IntoSystem: Sized {
    /// Scheduled system that owns this update function.
    type System: System;
    /// Turns this value into its corresponding [`System`].
    fn into_system(self) -> Self::System;
}

/// A stateful update function whose Rust type name identifies it in frame timings.
pub struct FunctionSystem<F> {
    func: F,
}

impl<F> System for FunctionSystem<F>
where
    F: FnMut(&mut MapContext) -> SystemResult + 'static,
{
    fn name(&self) -> Cow<'static, str> {
        type_name::<F>().into()
    }

    fn run(&mut self, context: &mut MapContext) -> SystemResult {
        (self.func)(context)
    }
}

impl<F> IntoSystem for F
where
    F: FnMut(&mut MapContext) -> SystemResult + 'static,
{
    type System = FunctionSystem<F>;

    fn into_system(self) -> Self::System {
        FunctionSystem { func: self }
    }
}
