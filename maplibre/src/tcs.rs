//! Typed resources, tile components and the systems that update a map's world.

#![deny(missing_docs)]

use std::{any::TypeId, collections::HashSet};

pub mod resources;
pub mod system;
pub mod tiles;
pub mod world;

/// Access ledger shared by the members of one resource or component query.
/// A fresh ledger belongs to each top-level query; it does not retain references to values.
#[derive(Default)]
pub struct GlobalQueryState {
    shared: HashSet<TypeId>,
    mutably_borrowed: HashSet<TypeId>,
    mutably_borrowed_components: HashSet<usize>,
}

/// A query's temporary access to the common borrow ledger.
pub trait QueryState<'s> {
    /// Binds a query to an existing ledger without clearing accesses made by other members.
    fn create(state: &'s mut GlobalQueryState) -> Self;
    /// Reborrows the same ledger for a member query; this does not create an independent copy.
    fn clone_to<'a, S: QueryState<'a>>(&'a mut self) -> S;
}

/// Borrow ledger handle used by the built-in reference and tuple queries.
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
