//! Deferred resource construction and replacement when resource criteria change.

#![deny(missing_docs)]

use std::mem;

use crate::{coords::WorldTileCoords, render::tile_view_pattern::HasTile, tcs::world::World};

/// Wrapper around a resource which can be initialized or uninitialized.
/// Uninitialized resources can be constructed once with [`Eventually::initialize`].
#[derive(Default)]
pub enum Eventually<T> {
    /// A resource is available for use and can be taken or replaced.
    Initialized(T),
    /// No resource is stored; initialization has not occurred or the value was taken.
    #[default]
    Uninitialized,
}

/// Determines whether a resource must be replaced to satisfy caller-supplied criteria.
pub trait HasChanged {
    /// Comparable criteria such as viewport dimensions or resource configuration.
    type Criteria: Eq;

    /// Whether the current resource no longer satisfies the supplied criteria.
    fn has_changed(&self, criteria: &Self::Criteria) -> bool;
}

impl<T> HasChanged for Option<T>
where
    T: HasChanged,
{
    type Criteria = T::Criteria;

    fn has_changed(&self, criteria: &Self::Criteria) -> bool {
        match self {
            None => true,
            Some(value) => value.has_changed(criteria),
        }
    }
}

impl<T> Eventually<T>
where
    T: HasChanged,
{
    #[tracing::instrument(name = "reinitialize", skip_all)]
    /// Constructs a replacement only when uninitialized or when the current resource has changed.
    pub fn reinitialize(&mut self, f: impl FnOnce() -> T, criteria: &T::Criteria) {
        let should_replace = match &self {
            Eventually::Initialized(current) => current.has_changed(criteria),
            Eventually::Uninitialized => true,
        };

        if should_replace {
            *self = Eventually::Initialized(f());
        }
    }
}
impl<T> Eventually<T> {
    #[tracing::instrument(name = "initialize", skip_all)]
    /// Initializes on first use and returns the stored resource; later calls do not invoke `f`.
    pub fn initialize(&mut self, f: impl FnOnce() -> T) -> &mut T {
        if let Eventually::Uninitialized = self {
            *self = Eventually::Initialized(f());
        }

        match self {
            Eventually::Initialized(data) => data,
            Eventually::Uninitialized => panic!("not initialized"),
        }
    }

    /// Moves out the stored state, leaving this wrapper uninitialized.
    pub fn take(&mut self) -> Eventually<T> {
        mem::replace(self, Eventually::Uninitialized)
    }

    /// Borrows the resource mutably when the caller has already established initialization.
    ///
    /// # Panics
    /// Panics with `message` if this wrapper is uninitialized.
    pub fn expect_initialized_mut(&mut self, message: &str) -> &mut T {
        match self {
            Eventually::Initialized(value) => value,
            Eventually::Uninitialized => panic!("{message}"),
        }
    }
}

impl<T> HasTile for Eventually<T>
where
    T: HasTile,
{
    fn has_tile(&self, coords: WorldTileCoords, world: &World) -> bool {
        match self {
            Eventually::Initialized(value) => value.has_tile(coords, world),
            Eventually::Uninitialized => false,
        }
    }
}
