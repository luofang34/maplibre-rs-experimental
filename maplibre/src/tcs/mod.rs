use std::{any::TypeId, collections::HashSet};

pub mod resources;
pub mod system;
pub mod tiles;
pub mod world;

#[derive(Default)]
pub struct GlobalQueryState {
    shared: HashSet<TypeId>,
    mutably_borrowed: HashSet<TypeId>,
    mutably_borrowed_components: HashSet<usize>,
}

pub trait QueryState<'s> {
    fn create(state: &'s mut GlobalQueryState) -> Self;
    fn clone_to<'a, S: QueryState<'a>>(&'a mut self) -> S;
}

pub struct EphemeralQueryState<'s> {
    state: &'s mut GlobalQueryState,
}

impl EphemeralQueryState<'_> {
    fn borrow_shared<T: 'static>(&mut self) -> Option<()> {
        let id = TypeId::of::<T>();
        if self.state.mutably_borrowed.contains(&id) {
            return None;
        }
        self.state.shared.insert(id);
        Some(())
    }

    fn borrow_mut<T: 'static>(&mut self) -> Option<()> {
        let id = TypeId::of::<T>();
        if self.state.shared.contains(&id) || !self.state.mutably_borrowed.insert(id) {
            return None;
        }
        Some(())
    }
}

impl<'s> QueryState<'s> for EphemeralQueryState<'s> {
    fn create(state: &'s mut GlobalQueryState) -> Self {
        Self { state }
    }

    fn clone_to<'a, S: QueryState<'a>>(&'a mut self) -> S {
        S::create(self.state)
    }
}
